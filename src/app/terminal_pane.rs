use super::*;

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
            let test_command: Vec<String> = [
                "/usr/bin/env",
                "HISTFILE=/dev/null",
                "PS1=$ ",
                "/bin/bash",
                "--noprofile",
                "--norc",
                "-i",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
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
        let session = self.terminals.get_mut(id).unwrap();
        let allow_input = selected
            && !self.palette
            && !self.settings
            && self.dialog.is_none()
            && self.pending.is_none();
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
        *cwd = session.cwd.clone();
        if let Some(error) = session.error.take() {
            self.message = error;
        }
        focused
    }
}
