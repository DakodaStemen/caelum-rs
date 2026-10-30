use super::*;

/// A multi-line paste held back from a terminal until the user confirms it.
pub(super) struct PendingPaste {
    terminal: String,
    text: String,
    lines: usize,
    preview: String,
    dont_ask: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn terminal_pane(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        theme: &Theme,
        id: &str,
        selected: bool,
        cwd: &mut PathBuf,
        command: &Option<Vec<String>>,
    ) -> bool {
        if !self.terminals.contains_key(id) {
            // Defense in depth: State::validate already drops unsupported commands.
            if !model::allowed_terminal_command(command.as_deref()) {
                ui.colored_label(
                    theme.ui("danger"),
                    "Refusing to launch an unsupported terminal command",
                );
                return false;
            }
            let test_command = caelum_rs::platform::test_shell_command();
            let chosen = if self.smoke.is_some() {
                Some(test_command.as_slice())
            } else {
                command.as_deref()
            };
            match Session::spawn(cwd, chosen, ctx.clone()) {
                Ok(s) => {
                    self.terminals.insert(id.to_string(), s);
                }
                Err(e) => {
                    ui.colored_label(theme.ui("danger"), format!("Cannot open terminal: {e}"));
                    return false;
                }
            }
        }
        let Some(session) = self.terminals.get_mut(id) else {
            // Unreachable today (inserted above); the next frame respawns the session.
            ui.colored_label(theme.ui("muted"), "Starting terminal…");
            ctx.request_repaint();
            return false;
        };
        let allow_input = selected
            && !self.palette
            && !self.settings
            && self.dialog.is_none()
            && self.pending.is_none()
            && self.pending_paste.is_none();
        let search = if self.dialog.as_deref() == Some("Search terminal") {
            self.dialog_input.as_str()
        } else {
            ""
        };
        let focused = terminal::show(
            ui,
            session,
            theme,
            &self.state.preferences,
            allow_input,
            search,
        );
        if let Some(text) = session.take_pending_paste() {
            let (lines, preview) = terminal::paste_preview(&text);
            self.pending_paste = Some(PendingPaste {
                terminal: id.to_string(),
                text,
                lines,
                preview,
                dont_ask: false,
            });
        }
        *cwd = session.cwd.clone();
        if let Some(error) = session.error.take() {
            self.message = error;
        }
        focused
    }
    /// Modal asking whether a held multi-line paste should reach the terminal.
    /// Enter pastes, Escape cancels; "Don't ask again" is saved on Paste.
    pub(super) fn paste_confirm(&mut self, ctx: &egui::Context) {
        let Some(p) = self.pending_paste.as_mut() else {
            return;
        };
        let mut paste = ctx.input(|i| i.key_pressed(Key::Enter));
        let mut cancel = ctx.input(|i| i.key_pressed(Key::Escape));
        egui::Window::new("Paste multiple lines?")
            .collapsible(false)
            .resizable(false)
            .default_width(540.)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "This paste has {} lines. The program has not enabled bracketed paste, \
                     so each line break will act as Enter.",
                    p.lines
                ));
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(egui::RichText::new(&p.preview).monospace()).wrap());
                });
                ui.checkbox(&mut p.dont_ask, "Don't ask again");
                ui.horizontal(|ui| {
                    paste |= ui.button("Paste").clicked();
                    cancel |= ui.button("Cancel").clicked();
                });
            });
        if cancel {
            self.pending_paste = None;
        } else if paste && let Some(p) = self.pending_paste.take() {
            if p.dont_ask {
                self.state.preferences.confirm_multiline_paste = false;
            }
            if let Some(session) = self.terminals.get_mut(&p.terminal) {
                session.paste(&p.text);
            }
        }
    }
}
