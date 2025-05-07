use egui::TextBuffer;
use egui::Ui;
use egui_code_editor::CodeEditor;
use egui_code_editor::ColorTheme;
use egui_code_editor::Syntax;
use enfusion_pak::vfs::VfsPath;

use crate::app::AppInternalData;
use crate::task::SearchResult;

#[derive(Clone)]
pub enum TabKind {
    Editor(EditorData),
    SearchResults(SearchData),
}

#[derive(Clone)]
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
    pub id: usize,
    pub results: Vec<SearchResult>,
}

impl TabKind {
    pub fn title(&self) -> &str {
        match self {
            TabKind::Editor(data) => data.title.as_str(),
            TabKind::SearchResults(data) => data.tab_title.as_str(),
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
            for (idx, file_result) in search_data.results.iter().enumerate() {
                let id = ui.make_persistent_id(file_result.file.as_str());

                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                )
                .show_header(ui, |ui| {
                    ui.label(file_result.file.as_str());
                    if ui.button("Open").clicked() {
                        if let Some(overlay_fs) = self.app_internal_data.overlay_fs.as_ref() {
                            let _ = self.app_internal_data.inbox.sender().send(
                                crate::task::BackgroundTaskMessage::RequestOpenFile(
                                    overlay_fs
                                        .join(file_result.file.as_str())
                                        .expect("failed to map async file to sync file"),
                                ),
                            );
                        }
                    }
                })
                .body(|ui| {
                    for (num, (line_num, file_match)) in file_result.matches.iter().enumerate() {
                        CodeEditor::default()
                            .id_source(format!("search_{}_result_{}", search_data.id, num))
                            .with_rows(file_match.lines().count())
                            .with_fontsize(14.0)
                            .with_theme(ColorTheme::GRUVBOX)
                            .with_syntax(Syntax::rust())
                            .with_numlines(true)
                            .with_numlines_shift(
                                (*line_num - 1).try_into().expect("invalid line num shift"),
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
        }
    }
}
