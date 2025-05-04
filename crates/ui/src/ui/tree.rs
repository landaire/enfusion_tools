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
    fn build_file_tree_node(&mut self, node: VfsPath, open: bool, ui: &mut Ui) {
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
                    self.build_file_tree_node(child, false, ui);
                }
            }
        });
    }

    pub(crate) fn show_file_tree(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("file_listing").show(ctx, |ui| {
            ui.vertical(|ui| {
                if !self.internal.pak_files.is_empty() {
                    ScrollArea::both().show(ui, |ui| {
                        if let Some(overlay_fs) = self.internal.overlay_fs.clone() {
                            self.build_file_tree_node(overlay_fs, true, ui);
                        }
                    });
                }
            })
        });
    }
}
