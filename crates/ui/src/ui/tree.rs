use egui::CollapsingHeader;
use egui::Label;
use egui::ScrollArea;
use egui::Sense;
use egui::Ui;
use enfusion_pak::vfs::OverlayFS;
use enfusion_pak::vfs::VfsPath;
use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
use itertools::Itertools;
use log::debug;

use crate::EnfusionToolsApp;

impl EnfusionToolsApp {
    fn build_file_tree_node(&mut self, node: VfsPath, open: bool, ui: &mut Ui) -> bool {
        let mut open_state_changed = false;
        let header = CollapsingHeader::new(node.filename()).default_open(open).show(ui, |ui| {
            for child in node.read_dir().expect("??").sorted_by_key(|path| path.filename()) {
                if child.is_file().unwrap_or_default() {
                    let file_label = ui.add(Label::new(child.filename()).sense(Sense::click()));
                    // self.add_view_file_menu(&file_label, node);
                    if file_label.double_clicked() {
                        debug!("file double-clicked");
                        if let Some(task_queue) = self.internal.task_queue.as_ref() {
                            debug!("sending task");
                            // Get the async version of this file
                            let _ = task_queue.send(crate::task::BackgroundTask::LoadFileData(
                                child.clone(),
                                self.internal
                                    .async_overlay_fs
                                    .clone()
                                    .expect("no async overlay FS?"),
                            ));
                        }
                    }
                } else {
                    open_state_changed |= self.build_file_tree_node(child, false, ui);
                }
            }
        });

        if header.header_response.clicked() {
            open_state_changed = true;
        }

        open_state_changed
    }

    pub(crate) fn show_file_tree(&mut self, ctx: &egui::Context) {
        static FILE_TREE_WIDTH_KEY: &str = "file_tree_desired_width";
        static FILE_TREE_FIRST_LOAD_KEY: &str = "file_tree_first_load";

        let (tree_info, first_load) = ctx.data_mut(|writer| {
            if writer.get_temp::<bool>(FILE_TREE_FIRST_LOAD_KEY.into()).is_some() {
                (writer.get_persisted(FILE_TREE_WIDTH_KEY.into()), false)
            } else {
                writer.insert_temp(FILE_TREE_FIRST_LOAD_KEY.into(), false);
                (writer.get_persisted(FILE_TREE_WIDTH_KEY.into()), true)
            }
        });

        let mut left_panel = egui::SidePanel::left("file_listing");

        if let Some((width, changed)) = tree_info {
            if changed || first_load {
                left_panel = left_panel.exact_width(width);
            }
        }

        left_panel.show(ctx, |ui| {
            ui.vertical(|ui| {
                if !self.internal.pak_files.is_empty() {
                    let mut open_state_changed = false;
                    let response = ScrollArea::both().show(ui, |ui| {
                        if let Some(overlay_fs) = self.internal.overlay_fs.clone() {
                            open_state_changed = self.build_file_tree_node(overlay_fs, true, ui);
                        }
                    });

                    if open_state_changed {
                        ctx.data_mut(|writer| {
                            let new_width = response.content_size.x;
                            writer.insert_persisted(
                                FILE_TREE_WIDTH_KEY.into(),
                                (new_width, open_state_changed),
                            );
                        });
                    }
                }
            })
        });
    }
}
