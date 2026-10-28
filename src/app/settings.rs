use super::*;

impl App {
    /// Applies a resolved system font when one is ready and starts resolving a changed
    /// selection on a worker thread so `fc-match` never blocks the UI.
    pub(super) fn apply_font(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.font_rx {
            match rx.try_recv() {
                Ok(bytes) => {
                    self.font_rx = None;
                    if let Some(bytes) = bytes {
                        let mut fonts = egui::FontDefinitions::default();
                        fonts.font_data.insert(
                            "system-mono".into(),
                            egui::FontData::from_owned(bytes).into(),
                        );
                        fonts
                            .families
                            .entry(egui::FontFamily::Monospace)
                            .or_default()
                            .insert(0, "system-mono".into());
                        ctx.set_fonts(fonts);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.font_rx = None,
            }
        }
        let name = self.state.preferences.terminal_font.clone();
        if name == self.applied_font {
            return;
        }
        self.applied_font = name.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.font_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(resolve_font(&name));
            ctx.request_repaint();
        });
    }
    pub(super) fn appearance(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut draft = self.theme_draft.take();
        let mut save_draft = false;
        let mut cancel_draft = false;
        egui::Window::new("Appearance")
            .open(&mut open)
            .default_width(650.)
            .max_height(650.)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(t) = &mut draft {
                        ui.label(
                            "Customize every interface and terminal color. Changes preview live.",
                        );
                        ui.text_edit_singleline(&mut t.name);
                        egui::ComboBox::from_label("Kind")
                            .selected_text(&t.kind)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut t.kind, "dark".into(), "Dark");
                                ui.selectable_value(&mut t.kind, "light".into(), "Light");
                            });
                        ui.separator();
                        for (name, values) in
                            [("Interface", &mut t.ui), ("Terminal", &mut t.terminal)]
                        {
                            ui.strong(name);
                            egui::Grid::new(name).num_columns(2).show(ui, |ui| {
                                for (key, value) in values {
                                    ui.label(key);
                                    ui.horizontal(|ui| {
                                        if let Some(c) = theme::color(value) {
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::splat(18.),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(rect, 3., c);
                                        }
                                        ui.add(
                                            egui::TextEdit::singleline(value).desired_width(110.),
                                        );
                                    });
                                    ui.end_row();
                                }
                            });
                        }
                        if let Err(e) = t.validate() {
                            ui.colored_label(Color32::LIGHT_RED, e.to_string());
                        }
                        ui.horizontal(|ui| {
                            save_draft = ui
                                .add_enabled(t.validate().is_ok(), egui::Button::new("Save theme"))
                                .clicked();
                            cancel_draft = ui.button("Cancel preview").clicked();
                        });
                    } else {
                        ui.label("TEN PRESETS · YOUR OWN COLORS");
                        ui.add_space(8.);
                        egui::Grid::new("themes")
                            .num_columns(5)
                            .spacing([8., 12.])
                            .show(ui, |ui| {
                                for (i, (id, t)) in self.presets.iter().enumerate() {
                                    let b = egui::Button::new(
                                        RichText::new(&t.name).color(t.ui("text")),
                                    )
                                    .fill(t.ui("surface"))
                                    .stroke(Stroke::new(
                                        if self.state.preferences.theme == *id {
                                            2_f32
                                        } else {
                                            1_f32
                                        },
                                        t.ui("accent"),
                                    ));
                                    if ui.add_sized([110., 45.], b).clicked() {
                                        self.state.preferences.theme = id.clone();
                                    }
                                    if i % 5 == 4 {
                                        ui.end_row();
                                    }
                                }
                            });
                        for t in &self.state.preferences.custom_themes {
                            if ui
                                .selectable_label(self.state.preferences.theme == t.id, &t.name)
                                .clicked()
                            {
                                self.state.preferences.theme = t.id.clone();
                            }
                        }
                        if ui.button("Customize current theme").clicked() {
                            let mut t = self.theme();
                            t.id = model::id();
                            t.name = format!("{} custom", t.name);
                            draft = Some(t);
                        }
                        ui.separator();
                        ui.strong("Type and cursor");
                        egui::ComboBox::from_label("Terminal font")
                            .selected_text(if self.state.preferences.terminal_font.is_empty() {
                                "System monospace"
                            } else {
                                &self.state.preferences.terminal_font
                            })
                            .show_ui(ui, |ui| {
                                for name in ["monospace", "DejaVu Sans Mono", "Liberation Mono"] {
                                    ui.selectable_value(
                                        &mut self.state.preferences.terminal_font,
                                        name.into(),
                                        name,
                                    );
                                }
                            });
                        ui.add(
                            egui::Slider::new(&mut self.state.preferences.font_size, 10. ..=28.)
                                .text("Terminal size"),
                        );
                        ui.add(
                            egui::Slider::new(&mut self.state.preferences.line_height, 1. ..=2.)
                                .text("Line height"),
                        );
                        egui::ComboBox::from_label("Cursor")
                            .selected_text(&self.state.preferences.cursor_style)
                            .show_ui(ui, |ui| {
                                for style in ["block", "bar", "underline"] {
                                    ui.selectable_value(
                                        &mut self.state.preferences.cursor_style,
                                        style.into(),
                                        style,
                                    );
                                }
                            });
                        ui.checkbox(&mut self.state.preferences.cursor_blink, "Blink cursor");
                        ui.separator();
                        ui.strong("Share themes");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.theme_path)
                                .hint_text("Absolute path to a theme JSON file")
                                .desired_width(f32::INFINITY),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Export current theme").clicked() {
                                let result = (|| -> Result<()> {
                                    let path = PathBuf::from(&self.theme_path);
                                    if !path.is_absolute() {
                                        bail!("Choose an absolute file path");
                                    }
                                    let mut file = fs::OpenOptions::new()
                                        .write(true)
                                        .create_new(true)
                                        .open(path)?;
                                    file.write_all(&serde_json::to_vec_pretty(&self.theme())?)?;
                                    Ok(())
                                })();
                                self.message = match result {
                                    Ok(()) => "Theme exported".into(),
                                    Err(e) => e.to_string(),
                                };
                            }
                            if ui.button("Import theme").clicked() {
                                let result = (|| -> Result<Theme> {
                                    let mut t: Theme =
                                        serde_json::from_slice(&fs::read(&self.theme_path)?)?;
                                    t.validate()?;
                                    t.id = model::id();
                                    Ok(t)
                                })();
                                match result {
                                    Ok(t) => {
                                        self.state.preferences.theme = t.id.clone();
                                        self.state.preferences.custom_themes.push(t);
                                    }
                                    Err(e) => self.message = e.to_string(),
                                }
                            }
                        });
                        ui.separator();
                        ui.label(format!("App data: {}", self.profile.display()));
                        ui.label("Escape closes this window.");
                    }
                });
            });
        if save_draft && let Some(t) = draft.take() {
            self.state.preferences.theme = t.id.clone();
            self.state.preferences.custom_themes.push(t);
        }
        if cancel_draft || !open {
            draft = None;
        }
        self.theme_draft = draft;
        self.settings = open;
    }
}

/// Resolves a font family through fontconfig and returns its bytes if egui can parse them.
fn resolve_font(name: &str) -> Option<Vec<u8>> {
    let output = std::process::Command::new("fc-match")
        .args([
            "-f",
            "%{file}",
            if name.is_empty() { "monospace" } else { name },
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout);
    let bytes = fs::read(path.trim()).ok()?;
    ab_glyph::FontArc::try_from_vec(bytes.clone()).ok()?;
    Some(bytes)
}
