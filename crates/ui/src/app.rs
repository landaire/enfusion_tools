use std::sync::mpsc;

use egui_code_editor::CodeEditor;
use egui_code_editor::ColorTheme;
use egui_code_editor::Syntax;
use enfusion_pak::vfs::MemoryFS;
use enfusion_pak::vfs::OverlayFS;
use enfusion_pak::vfs::VfsPath;
use enfusion_pak::vfs::async_vfs::AsyncMemoryFS;
use enfusion_pak::vfs::async_vfs::AsyncOverlayFS;
use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
use log::debug;

use crate::task::BackgroundTask;
use crate::task::BackgroundTaskMessage;
use crate::task::FileReference;
use crate::task::execute;
use crate::task::process_background_messages;
use crate::task::start_background_thread;

pub(crate) struct Internal {
    inbox: egui_inbox::UiInbox<BackgroundTaskMessage>,

    pub(crate) task_queue: Option<mpsc::Sender<BackgroundTask>>,
    task_queue_rx: Option<mpsc::Receiver<BackgroundTask>>,

    pub(crate) pak_files: Vec<VfsPath>,
    pub(crate) async_pak_files: Vec<AsyncVfsPath>,

    pub(crate) overlay_fs: Option<VfsPath>,
    pub(crate) async_overlay_fs: Option<AsyncVfsPath>,

    pub(crate) opened_file_text: String,
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct EnfusionToolsApp {
    pub(crate) data_path: String,

    #[serde(skip)]
    pub(crate) internal: Internal,

    pub(crate) opened_file_path: Option<String>,

    pub(crate) search_query: String,
}

impl Default for EnfusionToolsApp {
    fn default() -> Self {
        let data_dir = r#"D:\SteamLibrary\steamapps\common\Arma Reforger\addons\data"#.to_string();
        let inbox = egui_inbox::UiInbox::new();

        Self {
            data_path: data_dir,

            internal: Internal {
                inbox,
                task_queue: None,
                task_queue_rx: None,
                pak_files: Vec::new(),
                async_pak_files: Vec::new(),
                overlay_fs: None,
                async_overlay_fs: None,
                opened_file_text: "".to_string(),
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
            let mut pak_files = Vec::new();
            for entry in std::fs::read_dir(&app.data_path).expect("failed to read dir") {
                let entry = entry.expect("could not get entry");
                let path = entry.path();
                if let Some("pak") = path.extension().and_then(std::ffi::OsStr::to_str) {
                    pak_files.push(FileReference(path));
                }
            }

            task_queue
                .send(BackgroundTask::LoadPakFiles(pak_files))
                .expect("failed to send background task");
        }

        app.internal.task_queue = Some(task_queue);
        app.internal.task_queue_rx = maybe_task_queue_receiver;

        app
    }

    pub fn process_background_message(&mut self, message: BackgroundTaskMessage) {
        match message {
            BackgroundTaskMessage::LoadedPakFiles(files) => match files {
                Ok(files) => {
                    self.internal.pak_files.clear();
                    self.internal.pak_files.push(VfsPath::new(MemoryFS::new()));

                    self.internal.async_pak_files.clear();
                    self.internal.async_pak_files.push(AsyncVfsPath::new(AsyncMemoryFS::new()));

                    for file in files {
                        self.internal.pak_files.push(VfsPath::new(file.clone()));
                        self.internal.async_pak_files.push(AsyncVfsPath::new(file.clone()));
                    }

                    self.internal.overlay_fs =
                        Some(VfsPath::new(OverlayFS::new(self.internal.pak_files.as_slice())));
                    self.internal.async_overlay_fs = Some(AsyncVfsPath::new(AsyncOverlayFS::new(
                        self.internal.async_pak_files.as_slice(),
                    )));
                }
                Err(e) => {
                    eprintln!("failed to load pak files: {:?}", e);
                }
            },
            BackgroundTaskMessage::SearchResult(search_rx) => {
                self.internal.opened_file_text += search_rx.file.as_str();
                for m in search_rx.matches {
                    self.internal.opened_file_text += &m;
                    self.internal.opened_file_text += "\n...";
                }
                self.internal.opened_file_text += "\n";
            }
            BackgroundTaskMessage::FileDataLoaded(_, items) => {
                // Try reading this as text
                let Ok(str_data) = String::from_utf8(items) else {
                    return;
                };

                self.internal.opened_file_text = str_data;
            }
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
            process_background_messages(self.internal.inbox.sender(), task_queue_rx);
        }

        while let Some(message) = self.internal.inbox.read_without_ctx().next() {
            self.process_background_message(message);
        }

        // Put your widgets into a `SidePanel`, `TopBottomPanel`, `CentralPanel`, `Window` or `Area`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:

            egui::menu::bar(ui, |ui| {
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
                    if ui.button("Open").clicked() {
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
                    ui.label("Search");
                    let response = ui.text_edit_singleline(&mut self.search_query);

                    if response.lost_focus()
                        && response.ctx.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        debug!("Search requested");
                        if let Some(task_queue) = &self.internal.task_queue {
                            if let Some(vfs_root) = self.internal.async_overlay_fs.clone() {
                                debug!("Sending search task");
                                self.internal.opened_file_text.clear();
                                let _ = task_queue.send(BackgroundTask::PerformSearch(
                                    vfs_root,
                                    self.search_query.clone(),
                                ));
                            }
                        }
                    }
                });
                CodeEditor::default()
                    .id_source("code editor")
                    .with_rows(12)
                    .with_fontsize(14.0)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_syntax(Syntax::rust())
                    .with_numlines(true)
                    .vscroll(true)
                    .auto_shrink(false)
                    .show(ui, &mut self.internal.opened_file_text);
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
