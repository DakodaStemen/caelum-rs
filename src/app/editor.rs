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
            // Files load on the document IO thread; show a placeholder until the
            // result arrives (or the error, which also reaches the status line).
            if let Some(error) = self.docio.open_error(id) {
                ui.colored_label(theme.ui("danger"), error);
                return false;
            }
            if !self.docio.loading(id) {
                self.docio.open(id, path, false);
            }
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(theme.ui("muted")));
            });
            return false;
        }
        let language = self
            .syntax
            .language(path)
            .unwrap_or_else(|| "Plain text".into());
        let Some(doc) = self.documents.get_mut(id) else {
            // Unreachable (checked above); render a placeholder rather than panic.
            ui.colored_label(
                theme.ui("danger"),
                "Document is not loaded; reopen the file",
            );
            return false;
        };
        ui.horizontal(|ui| {
            if self.docio.saving(id) {
                ui.spinner();
            }
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
    /// Opens `path` in a new editor pane; the file loads in the background. If it
    /// cannot be opened the pane is removed again and the error is shown.
    pub(super) fn open_file(&mut self, path: PathBuf) {
        let id = model::id();
        self.docio.open(&id, &path, true);
        self.insert(Pane::Editor { id, path });
    }
    /// Saves document `id` on the IO thread (coalescing with a save already running).
    pub(super) fn save_document(&mut self, id: &str) {
        let Some(doc) = self.documents.get(id) else {
            if self.docio.loading(id) {
                self.message = "File is still loading".into();
            }
            return;
        };
        self.message = match self.docio.save(id, doc) {
            files::SaveRequest::Started => "Saving…".into(),
            files::SaveRequest::Queued => "Saving… latest edits follow".into(),
        };
    }
    /// Applies finished document opens and saves.
    pub(super) fn poll_documents(&mut self) {
        let events = self.docio.poll(&mut self.documents);
        self.document_events(events);
    }
    /// Waits for every pending save to land (exit, window close, smoke checks).
    pub(super) fn flush_documents(&mut self) {
        let events = self.docio.flush(&mut self.documents);
        self.document_events(events);
    }
    fn document_events(&mut self, events: Vec<files::DocEvent>) {
        let mut remove = vec![];
        for event in events {
            match event {
                files::DocEvent::Opened { .. } => {}
                files::DocEvent::OpenFailed {
                    id,
                    error,
                    remove_pane,
                } => {
                    eprintln!("caelum-rs: {error}");
                    self.message = error;
                    if remove_pane {
                        remove.push(id);
                    }
                }
                files::DocEvent::Saved { .. } => self.message = "File saved".into(),
                files::DocEvent::SaveFailed { error, .. } => {
                    eprintln!("caelum-rs: {error}");
                    self.message = error;
                }
            }
        }
        if remove.is_empty() {
            return;
        }
        // A file that failed to open from "open file" gets no pane, as before.
        let cwd = self.cwd();
        for w in &mut self.state.workspaces {
            for t in &mut w.tabs {
                for id in &remove {
                    if t.layout.find(id).is_some() && !t.layout.remove(id) {
                        t.layout = Pane::terminal(cwd.clone());
                    }
                }
            }
        }
        if remove.iter().any(|id| self.zoom.as_ref() == Some(id)) {
            self.zoom = None;
        }
        self.ensure_focus();
        self.prune();
    }
}
