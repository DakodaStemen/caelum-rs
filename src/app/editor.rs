use super::panes::badge;
use super::*;

impl App {
    pub(super) fn editor_pane(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        id: &str,
        path: &Path,
    ) -> bool {
        let mut focus_requested = false;
        if !self.documents.contains_key(id) {
            match Document::open(path) {
                Ok(doc) => {
                    self.documents.insert(id.to_string(), doc);
                }
                Err(e) => {
                    ui.colored_label(theme.ui("danger"), e.to_string());
                    return false;
                }
            }
        }
        let language = self
            .syntax
            .language(path)
            .unwrap_or_else(|| "Plain text".into());
        let doc = self.documents.get_mut(id).unwrap();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!doc.read_only && doc.dirty(), egui::Button::new("Save"))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.pending_save = Some(id.to_string());
            }
            if doc.read_only {
                ui.colored_label(theme.ui("danger"), "Unknown encoding · read only");
            }
            badge(ui, theme, &language, theme.syn("function"));
            let lines = doc.text.lines().count().max(1);
            ui.label(
                RichText::new(format!("{lines} lines"))
                    .small()
                    .color(theme.ui("muted")),
            );
            ui.label(
                RichText::new(doc.path.display().to_string())
                    .small()
                    .color(theme.ui("muted")),
            );
        });
        let font = FontId::monospace(self.state.preferences.font_size);
        let doc_path = doc.path.clone();
        let syntax = &mut self.syntax;
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            let job = syntax.job(&doc_path, text.as_str(), theme, font.clone(), wrap);
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let edit = egui::TextEdit::multiline(&mut doc.text)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .interactive(!doc.read_only)
            .code_editor()
            .layouter(&mut layouter);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let size = ui.available_size().max(Vec2::new(1., 100.));
                if ui.add_sized(size, edit).has_focus() {
                    focus_requested = true;
                }
            });
        focus_requested
    }
    pub(super) fn open_file(&mut self, path: PathBuf) {
        match Document::open(&path) {
            Ok(doc) => {
                let id = model::id();
                let path = doc.path.clone();
                self.documents.insert(id.clone(), doc);
                self.insert(Pane::Editor { id, path });
            }
            Err(e) => self.message = e.to_string(),
        }
    }
}
