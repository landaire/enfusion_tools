use std::sync::Arc;

use egui::Color32;
use egui::TextFormat;
use egui::Ui;
use egui::text::LayoutJob;
use egui_code_editor::CodeEditor;
use egui_code_editor::ColorTheme;
use egui_code_editor::Syntax;
use enfusion_pak::vfs::VfsPath;

use crate::app::AppInternalData;
use crate::diff;
use crate::diff::DiffResult;
use crate::task;
use crate::task::LineNumber;
use crate::task::SearchId;
use crate::task::SearchResult;
use crate::task::execute;

#[derive(Clone)]
pub enum TabKind {
    Editor(EditorData),
    SearchResults(SearchData),
    Diff(DiffData),
}

#[derive(Clone)]
#[allow(unused)]
pub struct EditorData {
    pub opened_file: VfsPath,
    pub title: String,
    pub contents: String,
}

#[derive(Clone)]
#[allow(unused)]
pub struct SearchData {
    pub query: String,
    pub tab_title: String,
    pub id: SearchId,
    pub results: Vec<SearchResult>,
}

#[derive(Clone)]
pub struct DiffData {
    pub modified: Vec<diff::DiffResult>,
    pub modified_filtered: Option<Vec<diff::DiffResult>>,
    pub path_filter: String,
}

impl TabKind {
    pub fn title(&self) -> &str {
        match self {
            TabKind::Editor(data) => data.title.as_str(),
            TabKind::SearchResults(data) => data.tab_title.as_str(),
            TabKind::Diff(_results) => "Diff",
        }
    }
}
pub struct ToolsTabViewer<'a> {
    pub app_internal_data: &'a mut AppInternalData,
}

impl ToolsTabViewer<'_> {
    fn build_editor_tab(&self, editor: &mut EditorData, ui: &mut Ui) {
        CodeEditor::default()
            .id_source(format!("{}_code_editor", &editor.title))
            .with_rows(12)
            .with_fontsize(14.0)
            .with_theme(ColorTheme::GRUVBOX)
            .with_syntax(Syntax::rust())
            .with_numlines(true)
            .vscroll(true)
            .auto_shrink(false)
            .show(ui, &mut &*editor.contents);
    }

    fn build_search_results_tab(&self, search_data: &SearchData, ui: &mut Ui) {
        ui.vertical(|ui| {
            for file_result in &search_data.results {
                let id = ui.make_persistent_id(file_result.file.as_str());

                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                )
                .show_header(ui, |ui| {
                    ui.label(file_result.file.as_str());
                    if ui.button("Open").clicked()
                        && let Some(overlay_fs) = self.app_internal_data.overlay_fs.as_ref()
                    {
                        let _ = self.app_internal_data.inbox.sender().send(
                            crate::task::BackgroundTaskMessage::RequestOpenFile(
                                overlay_fs
                                    .join(file_result.file.as_str())
                                    .expect("failed to map async file to sync file"),
                            ),
                        );
                    }
                })
                .body(|ui| {
                    for (num, (LineNumber(line_num), file_match)) in
                        file_result.matches.iter().enumerate()
                    {
                        CodeEditor::default()
                            .id_source(format!("search_{}_result_{}", search_data.id.0, num))
                            .with_rows(file_match.lines().count())
                            .with_fontsize(14.0)
                            .with_theme(ColorTheme::GRUVBOX)
                            .with_syntax(Syntax::rust())
                            .with_numlines(true)
                            .with_numlines_shift(
                                (line_num - 1).try_into().expect("invalid line num shift"),
                            )
                            .vscroll(false)
                            .auto_shrink(false)
                            .show(ui, &mut file_match.as_str());

                        ui.separator();
                    }
                });
            }
        });
    }

    fn build_diff_tab(&self, diff_data: &mut DiffData, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Path Filter:");
                if ui.text_edit_singleline(&mut diff_data.path_filter).changed() {
                    diff_data.modified_filtered = Some(
                        diff_data
                            .modified
                            .iter()
                            .filter(|diff| {
                                task::ascii_icontains(
                                    &diff_data.path_filter,
                                    diff.comparison_path(),
                                )
                            })
                            .cloned()
                            .collect(),
                    );
                }
            });
            let modified = if let Some(filtered) = &diff_data.modified_filtered {
                filtered
            } else {
                &diff_data.modified
            };
            for result in modified {
                let mut heading = LayoutJob::default();
                match result {
                    DiffResult::Added { path, overlay, data } => {
                        heading.append(
                            path.as_str(),
                            0.0,
                            TextFormat { color: Color32::LIGHT_GREEN, ..Default::default() },
                        );

                        ui.collapsing(heading, |ui| {
                            let data_inner = data.lock().unwrap();
                            if let Some(data_inner) = &*data_inner {
                                ui.label(Arc::clone(data_inner));
                            } else {
                                let added_file = overlay.join(path.as_str()).unwrap();
                                let output = Arc::clone(data);
                                execute(async move {
                                    if let Some(data) = task::read_file_data(added_file)
                                        .await
                                        .and_then(|data| String::from_utf8(data).ok())
                                    {
                                        let mut job = LayoutJob::default();
                                        job.append(data.as_str(), 0.0, Default::default());
                                        *output.lock().unwrap() = Some(job.into());
                                    } else {
                                        *output.lock().unwrap() = Some(LayoutJob::default().into());
                                    }
                                });
                            }
                        });
                    }
                    DiffResult::Changed {
                        base_path,
                        base_overlay,
                        modified_path,
                        modified_overlay,
                        data,
                    } => {
                        heading.append(
                            base_path.as_str(),
                            0.0,
                            TextFormat { color: Color32::ORANGE, ..Default::default() },
                        );

                        ui.collapsing(heading, |ui| {
                            let data_inner = data.lock().unwrap();
                            if let Some(data_inner) = &*data_inner {
                                ui.label(Arc::clone(data_inner));
                            } else {
                                let base = base_overlay.join(base_path.as_str()).unwrap();
                                let modified =
                                    modified_overlay.join(modified_path.as_str()).unwrap();
                                let output = Arc::clone(data);
                                execute(async move {
                                    diff::build_file_diff(base, modified, output).await;
                                });
                            }
                        });
                    }
                }
            }
        });
    }
}

impl egui_dock::TabViewer for ToolsTabViewer<'_> {
    type Tab = TabKind;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            TabKind::Editor(editor_data) => {
                self.build_editor_tab(editor_data, ui);
            }
            TabKind::SearchResults(search_data) => {
                self.build_search_results_tab(search_data, ui);
            }
            TabKind::Diff(diff_data) => {
                self.build_diff_tab(diff_data, ui);
            }
        }
    }
}
