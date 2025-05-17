use std::sync::Arc;

use egui::ScrollArea;
use egui::TextEdit;
use egui::Widget;
use egui_ltreeview::NodeBuilder;
use egui_ltreeview::TreeView;

use crate::EnfusionToolsApp;

impl EnfusionToolsApp {
    pub(crate) fn show_file_tree(&mut self, ctx: &egui::Context) {
        // static FILE_TREE_WIDTH_KEY: &str = "file_tree_desired_width";
        // static FILE_TREE_FIRST_LOAD_KEY: &str = "file_tree_first_load";

        // let (tree_info, first_load) = ctx.data_mut(|writer| {
        //     if writer.get_temp::<bool>(FILE_TREE_FIRST_LOAD_KEY.into()).is_some() {
        //         (writer.get_persisted(FILE_TREE_WIDTH_KEY.into()), false)
        //     } else {
        //         writer.insert_temp(FILE_TREE_FIRST_LOAD_KEY.into(), false);
        //         (writer.get_persisted(FILE_TREE_WIDTH_KEY.into()), true)
        //     }
        // });

        let left_panel = egui::SidePanel::left("file_listing");

        // if let Some((width, changed)) = tree_info {
        //     if changed || first_load {
        //         left_panel = left_panel.exact_width(width);
        //     }
        // }

        left_panel.show(ctx, |ui| {
            ui.vertical(|ui| {
                let response =
                    TextEdit::singleline(&mut self.internal.file_filter).hint_text("Filter").ui(ui);

                if response.lost_focus()
                    && response.ctx.input(|input| input.key_pressed(egui::Key::Enter))
                {
                    if self.internal.file_filter.is_empty() {
                        self.internal.filtered_tree = None;
                    } else if self.internal.file_filter.len() >= 2 {
                        if let Some(overlay_fs) = self.internal.overlay_fs.clone() {
                            if let Some(task_queue) = self.internal.task_queue.as_ref() {
                                let _ = task_queue.send(crate::task::BackgroundTask::FilterPaths(
                                    Arc::clone(&self.internal.known_file_paths),
                                    overlay_fs,
                                    self.internal.file_filter.clone(),
                                ));
                            }
                        }
                    }
                }
                if self.internal.overlay_fs.is_some() {
                    // let mut open_state_changed = false;
                    ScrollArea::both().show(ui, |ui| {
                        let tree =
                            self.internal.filtered_tree.as_ref().unwrap_or(&self.internal.tree);

                        let (_response, actions) =
                            TreeView::new(ui.make_persistent_id("main_fs_tree_view"))
                                .allow_multi_selection(false)
                                .tree_size_hint(self.internal.tree.len())
                                .dir_count_hint(self.internal.dir_count)
                                .show_state(ui, &mut self.internal.tree_view_state, |builder| {
                                    for node in tree {
                                        if node.is_dir {
                                            builder.node(
                                                NodeBuilder::dir(node.id)
                                                    .default_open(node.id == 0)
                                                    .label(&node.title),
                                            );
                                        } else {
                                            builder.leaf(node.id, &node.title);
                                        }

                                        for _ in 0..node.close_count {
                                            builder.close_dir();
                                        }
                                    }
                                });

                        for action in actions {
                            match action {
                                egui_ltreeview::Action::SetSelected(_items) => {
                                    // open_state_changed = true;
                                }
                                // egui_ltreeview::Action::Move(_drag_and_drop) => todo!(),
                                // egui_ltreeview::Action::Drag(_drag_and_drop) => todo!(),
                                egui_ltreeview::Action::Activate(activate) => {
                                    for activated in activate.selected {
                                        self.open_file(tree[activated].vfs_path.clone());
                                    }
                                }
                                _ => {
                                    // do nothing,
                                }
                            }
                        }
                    });

                    // if open_state_changed {
                    //     ctx.data_mut(|writer| {
                    //         let new_width = response.content_size.x;
                    //         writer.insert_persisted(
                    //             FILE_TREE_WIDTH_KEY.into(),
                    //             (new_width, open_state_changed),
                    //         );
                    //     });
                    // }
                }
            })
        });
    }
}
