use egui::{CollapsingHeader, Label, ScrollArea, Sense, Ui};
use enfusion_pak::vfs::{OverlayFS, VfsPath};
use itertools::Itertools;

use crate::EnfusionToolsApp;

pub fn show_file_tree(ui: &mut Ui) {}

impl EnfusionToolsApp {
    fn build_file_tree_node(&mut self, node: VfsPath, open: bool, ui: &mut Ui) {
        let header = CollapsingHeader::new(node.filename())
            .default_open(open)
            .show(ui, |ui| {
                for child in node
                    .read_dir()
                    .expect("??")
                    .sorted_by_key(|path| path.filename())
                {
                    if child.is_file().unwrap_or_default() {
                        let file_label = ui.add(Label::new(child.filename()).sense(Sense::click()));
                        // self.add_view_file_menu(&file_label, node);
                        if file_label.double_clicked() {
                            let file_result = child
                                .open_file()
                                .ok()
                                .map(|mut file| std::io::read_to_string(&mut file));

                            println!("{:?}", file_result);

                            if let Some(Ok(opened_file)) = file_result {
                                self.opened_file = opened_file;
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
                if !self.pak_files.is_empty() {
                    let overlay_fs = VfsPath::new(OverlayFS::new(&self.pak_files));

                    ScrollArea::both().show(ui, |ui| {
                        self.build_file_tree_node(overlay_fs, true, ui);
                    });
                }
            })
        });
    }
}
