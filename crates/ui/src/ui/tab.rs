use egui::{CollapsingHeader, TextBuffer, Ui};
use egui_code_editor::{CodeEditor, ColorTheme, Syntax};
use enfusion_pak::vfs::VfsPath;

use crate::{app::AppInternalData, task::SearchResult};

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
pub struct SearchData {
    pub query: String,
    pub id: usize,
    pub results: Vec<SearchResult>,
}

impl TabKind {
    pub fn title(&self) -> &str {
        match self {
            TabKind::Editor(data) => data.title.as_str(),
            TabKind::SearchResults(data) => data.query.as_str(),
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
            .show(ui, &mut editor.contents);
    }

    fn build_search_results_tab(&self, search_data: &SearchData, ui: &mut Ui) {
        ui.vertical(|ui| {
            for file_result in &search_data.results {
                if CollapsingHeader::new(file_result.file.as_str())
                    .default_open(true)
                    .show(ui, |ui| {
                        for (num, file_match) in file_result.matches.iter().enumerate() {
                            CodeEditor::default()
                                .id_source(format!("search_{}_result_{}", search_data.id, num))
                                .with_rows(file_match.lines().count())
                                .with_fontsize(14.0)
                                .with_theme(ColorTheme::GRUVBOX)
                                .with_syntax(Syntax::rust())
                                .with_numlines(true)
                                .vscroll(false)
                                .auto_shrink(false)
                                .show(ui, &mut file_match.as_str());

                            ui.separator();
                        }
                    })
                    .header_response
                    .double_clicked()
                {
                    // TODO: open file
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
        }
    }
}
