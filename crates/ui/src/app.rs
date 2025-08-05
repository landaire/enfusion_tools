use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;

use egui_dock::DockArea;
use egui_dock::DockState;
use egui_dock::Style;
use egui_ltreeview::TreeViewState;
use enfusion_pak::vfs::VfsPath;
use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
use log::debug;

use crate::task::BackgroundTask;
use crate::task::BackgroundTaskMessage;
use crate::task::FileName;
use crate::task::FileReference;
use crate::task::FullPath;
use crate::task::SearchId;
use crate::task::execute;
use crate::task::process_background_requests;
use crate::task::start_background_thread;
use crate::ui::tab::DiffData;
use crate::ui::tab::EditorData;
use crate::ui::tab::SearchData;
use crate::ui::tab::TabKind;
use crate::ui::tab::ToolsTabViewer;

#[derive(Debug)]
pub struct TreeNode {
    pub id: usize,
    pub is_dir: bool,
    pub title: String,
    pub close_count: usize,
    pub vfs_path: VfsPath,
}

pub(crate) type KnownPaths = HashMap<(FullPath, FileName), VfsPath>;

pub(crate) struct AppInternalData {
    pub(crate) inbox: egui_inbox::UiInbox<BackgroundTaskMessage>,

    pub(crate) task_queue: Option<mpsc::Sender<BackgroundTask>>,
    task_queue_rx: Option<mpsc::Receiver<BackgroundTask>>,

    pub(crate) overlay_fs: Option<VfsPath>,
    pub(crate) async_overlay_fs: Option<AsyncVfsPath>,
    pub(crate) known_file_paths: Arc<KnownPaths>,

    pub(crate) opened_file_text: String,
    pub(crate) file_filter: String,

    pub(crate) next_search_query_id: SearchId,
    pub(crate) tree_view_state: TreeViewState<usize>,
    pub(crate) tree: Vec<TreeNode>,
    pub(crate) filtered_tree: Option<Vec<TreeNode>>,
    pub(crate) open_nodes: Vec<bool>,
    pub(crate) dir_count: usize,
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct EnfusionToolsApp {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) file_paths: Vec<String>,

    #[serde(skip)]
    pub(crate) internal: AppInternalData,

    #[serde(skip)]
    pub(crate) dock_state: DockState<TabKind>,

    pub(crate) opened_file_path: Option<String>,

    pub(crate) search_query: String,
}

impl Default for EnfusionToolsApp {
    fn default() -> Self {
        let inbox = egui_inbox::UiInbox::new();

        Self {
            #[cfg(not(target_arch = "wasm32"))]
            file_paths: Default::default(),

            dock_state: DockState::new([].to_vec()),
            internal: AppInternalData {
                inbox,
                task_queue: None,
                task_queue_rx: None,
                overlay_fs: None,
                async_overlay_fs: None,
                opened_file_text: "".to_string(),
                file_filter: "".to_string(),
                known_file_paths: Default::default(),
                next_search_query_id: SearchId(0),
                tree_view_state: TreeViewState::default(),
                tree: Default::default(),
                dir_count: 0,
                filtered_tree: None,
                open_nodes: vec![],
            },
            opened_file_path: None,
            search_query: "".to_string(),
        }
    }
}

impl EnfusionToolsApp {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        let mut app: EnfusionToolsApp = if let Some(storage) = cc.storage {
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            Default::default()
        };

        let (task_queue, maybe_task_queue_receiver) =
            start_background_thread(app.internal.inbox.sender());

        #[cfg(not(target_arch = "wasm32"))]
        {
            if !app.file_paths.is_empty() {
                let mut pak_file_paths = Vec::new();
                for file in &app.file_paths {
                    let path = std::path::PathBuf::from(file);
                    if path.exists() {
                        pak_file_paths.push(FileReference(path));
                    }
                }

                task_queue
                    .send(BackgroundTask::LoadPakFiles(pak_file_paths))
                    .expect("failed to send background task");
            }
        }

        app.internal.task_queue = Some(task_queue);
        app.internal.task_queue_rx = maybe_task_queue_receiver;

        app
    }

    pub fn process_message_from_background(&mut self, message: BackgroundTaskMessage) {
        match message {
            BackgroundTaskMessage::LoadedPakFiles(files) => match files {
                #[allow(unused_mut)]
                Ok((mut loaded_files, file_tree)) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.file_paths = loaded_files
                            .disk_files_parsed
                            .drain(..)
                            .filter_map(|handle| handle.0.to_str().map(|s| s.to_string()))
                            .collect();
                    }

                    self.internal.known_file_paths = Arc::new(loaded_files.known_paths);

                    self.internal.overlay_fs = Some(loaded_files.overlay_fs);
                    self.internal.async_overlay_fs = Some(loaded_files.async_overlay_fs);

                    self.internal.tree = file_tree;
                    self.internal.dir_count = self
                        .internal
                        .tree
                        .iter()
                        .fold(0, |accum, node| if node.is_dir { accum + 1 } else { accum });
                    self.internal.open_nodes.clear();
                    self.internal.open_nodes.push(true);
                }
                Err(e) => {
                    eprintln!("failed to load pak files: {e:?}");
                }
            },
            BackgroundTaskMessage::SearchResult(search_id, search_result) => {
                for tab in self.dock_state.iter_all_tabs_mut() {
                    let TabKind::SearchResults(data) = tab.1 else {
                        continue;
                    };

                    if data.id == search_id {
                        data.results.push(search_result);
                        break;
                    }
                }
            }
            BackgroundTaskMessage::FileDataLoaded(file, items) => {
                // Try reading this as text
                let Ok(str_data) = String::from_utf8(items) else {
                    return;
                };

                let surface = self.dock_state.main_surface_mut();
                surface.push_to_first_leaf(TabKind::Editor(EditorData {
                    title: file.filename(),
                    opened_file: file,
                    contents: str_data,
                }));
            }
            BackgroundTaskMessage::FilesFiltered(filtered_tree) => {
                self.internal.filtered_tree = Some(filtered_tree);
            }
            BackgroundTaskMessage::RequestOpenFile(vfs_path) => {
                self.open_file(vfs_path);
            }
            BackgroundTaskMessage::FilesDiffed(diff_results) => match diff_results {
                Ok(results) => {
                    let surface = self.dock_state.main_surface_mut();
                    surface.push_to_first_leaf(TabKind::Diff(DiffData {
                        modified: results,
                        modified_filtered: Default::default(),
                        path_filter: Default::default(),
                    }));
                }
                Err(e) => {
                    eprintln!("failed to load pak files: {e:?}");
                }
            },
        }
    }

    pub(crate) fn open_file(&self, file: VfsPath) {
        if !file.is_file().unwrap_or_default() {
            return;
        }

        if let Some(task_queue) = self.internal.task_queue.as_ref() {
            debug!("sending task");
            // Get the async version of this file
            let _ = task_queue.send(crate::task::BackgroundTask::LoadFileData(
                file,
                self.internal.async_overlay_fs.clone().expect("no async overlay FS?"),
            ));
        }
    }
}

impl eframe::App for EnfusionToolsApp {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.internal.inbox.set_ctx(ctx);

        // Process any background messages
        if let Some(task_queue_rx) = self.internal.task_queue_rx.as_ref() {
            process_background_requests(self.internal.inbox.sender(), task_queue_rx);
        }

        while let Some(message) = self.internal.inbox.read_without_ctx().next() {
            self.process_message_from_background(message);
        }

        // Put your widgets into a `SidePanel`, `TopBottomPanel`, `CentralPanel`, `Window` or `Area`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:

            egui::MenuBar::new().ui(ui, |ui| {
                // NOTE: no File->Quit on web pages!
                let is_web = cfg!(target_arch = "wasm32");
                if !is_web {
                    ui.menu_button("File", |ui| {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.add_space(16.0);
                }

                egui::widgets::global_theme_preference_buttons(ui);
            });
        });

        self.show_file_tree(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open PAK Files").clicked() {
                        let task = rfd::AsyncFileDialog::new().pick_files();
                        if let Some(background_task_sender) = self.internal.task_queue.clone() {
                            execute(async move {
                                let file = task.await;
                                if let Some(mut files) = file {
                                    #[cfg(target_arch = "wasm32")]
                                    let _ =
                                        background_task_sender.send(BackgroundTask::LoadPakFiles(
                                            files.drain(..).map(FileReference).collect(),
                                        ));

                                    #[cfg(not(target_arch = "wasm32"))]
                                    let _ =
                                        background_task_sender.send(BackgroundTask::LoadPakFiles(
                                            files
                                                .drain(..)
                                                .map(|handle| {
                                                    FileReference(handle.path().to_owned())
                                                })
                                                .collect(),
                                        ));
                                }
                            });
                        }
                    }
                    if ui.button("Diff Builds").clicked() {
                        if let Some(background_task_sender) = self.internal.task_queue.clone() {
                            execute(async move {
                                let base_files = rfd::AsyncFileDialog::new().pick_files().await;
                                if let Some(mut base_files) = base_files {
                                    let modified_files =
                                        rfd::AsyncFileDialog::new().pick_files().await;
                                    if let Some(mut modified_files) = modified_files {
                                        #[cfg(target_arch = "wasm32")]
                                        let _ = background_task_sender.send(
                                            BackgroundTask::DiffBuilds {
                                                base: base_files
                                                    .drain(..)
                                                    .map(FileReference)
                                                    .collect(),
                                                modified: modified_files
                                                    .drain(..)
                                                    .map(FileReference)
                                                    .collect(),
                                            },
                                        );

                                        #[cfg(not(target_arch = "wasm32"))]
                                        let _ = background_task_sender.send(
                                            BackgroundTask::DiffBuilds {
                                                base: base_files
                                                    .drain(..)
                                                    .map(|handle| {
                                                        FileReference(handle.path().to_owned())
                                                    })
                                                    .collect(),
                                                modified: modified_files
                                                    .drain(..)
                                                    .map(|handle| {
                                                        FileReference(handle.path().to_owned())
                                                    })
                                                    .collect(),
                                            },
                                        );
                                    }
                                }
                            });
                        }
                    }
                    ui.label("Search");
                    let response = ui.text_edit_singleline(&mut self.search_query);

                    if response.lost_focus()
                        && response.ctx.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        debug!("Search requested");
                        if let Some(task_queue) = &self.internal.task_queue {
                            if let Some(vfs_root) = self.internal.async_overlay_fs.clone() {
                                debug!("Sending earch task");
                                self.internal.opened_file_text.clear();
                                let search_id = self.internal.next_search_query_id;
                                self.internal.next_search_query_id.0 += 1;

                                let _ = task_queue.send(BackgroundTask::PerformSearch(
                                    search_id,
                                    vfs_root,
                                    self.search_query.clone(),
                                ));

                                let query = self.search_query.clone();
                                self.dock_state.main_surface_mut().push_to_first_leaf(
                                    TabKind::SearchResults(SearchData {
                                        tab_title: format!("{query} - Search Results"),
                                        query: self.search_query.clone(),
                                        id: search_id,
                                        results: Default::default(),
                                    }),
                                );
                            }
                        }
                    }
                });

                DockArea::new(&mut self.dock_state)
                    .style(Style::from_egui(ui.style().as_ref()))
                    .allowed_splits(egui_dock::AllowedSplits::All)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_close_buttons(true)
                    .show_inside(ui, &mut ToolsTabViewer { app_internal_data: &mut self.internal });
            });

            // ui.add_sized(ui.available_size(), widget)
            // ui.text_edit_multiline(&mut self.internal.opened_file_text);
        });
    }
}

// fn powered_by_egui_and_eframe(ui: &mut egui::Ui) {
//     ui.horizontal(|ui| {
//         ui.spacing_mut().item_spacing.x = 0.0;
//         ui.label("Powered by ");
//         ui.hyperlink_to("egui", "https://github.com/emilk/egui");
//         ui.label(" and ");
//         ui.hyperlink_to("eframe", "https://github.com/emilk/egui/tree/master/crates/eframe");
//         ui.label(".");
//     });
// }
