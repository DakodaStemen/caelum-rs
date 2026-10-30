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
            .default_pos(ctx.content_rect().center())
            .pivot(egui::Align2::CENTER_CENTER)
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
                        for (name, values) in [
                            ("Code", &mut t.syntax),
                            ("Interface", &mut t.ui),
                            ("Terminal", &mut t.terminal),
                        ] {
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
                        ui.label("ELEVEN PRESETS · YOUR OWN COLORS");
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

/// Resolves a font family (fontconfig first, then standard font directories) and
/// returns its bytes if egui can parse them.
fn resolve_font(name: &str) -> Option<Vec<u8>> {
    // A leading '-' would be parsed by fc-match as an option, not a font pattern.
    if name.starts_with('-') {
        return None;
    }
    let parse = |bytes: Vec<u8>| {
        ab_glyph::FontArc::try_from_vec(bytes.clone())
            .ok()
            .map(|_| bytes)
    };
    if let Ok(output) = std::process::Command::new("fc-match")
        .args([
            "-f",
            "%{file}",
            "--",
            if name.is_empty() { "monospace" } else { name },
        ])
        .output()
        && output.status.success()
        && let Some(bytes) = fs::read(String::from_utf8_lossy(&output.stdout).trim())
            .ok()
            .and_then(parse)
    {
        return Some(bytes);
    }
    // macOS/Windows (or Linux without fontconfig): scan the standard font directories.
    let wanted: Vec<&str> = if name.is_empty() {
        vec!["Menlo", "Consolas", "Courier New", "DejaVuSansMono"]
    } else {
        vec![name]
    };
    let dirs = font_dirs();
    wanted.into_iter().find_map(|n| {
        find_font_file(&dirs, n)
            .and_then(|p| fs::read(p).ok())
            .and_then(parse)
    })
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
        caelum_rs::platform::home().join("Library/Fonts"),
    ];
    if let Some(w) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")) {
        dirs.push(PathBuf::from(w).join("Fonts"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join("Microsoft/Windows/Fonts"));
    }
    dirs
}

/// Finds a font file whose name matches `family` (case, spaces and dashes ignored):
/// an exact stem match wins, otherwise the shortest stem starting with the family.
fn find_font_file(dirs: &[PathBuf], family: &str) -> Option<PathBuf> {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let want = norm(family);
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(usize, PathBuf)> = None;
    let mut stack: Vec<(PathBuf, u8)> = dirs.iter().cloned().map(|d| (d, 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                if depth < 2 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let ext = path
                .extension()
                .and_then(|x| x.to_str())
                .map(str::to_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc")) {
                continue;
            }
            let stem = norm(path.file_stem().and_then(|x| x.to_str()).unwrap_or(""));
            if stem == want {
                return Some(path);
            }
            if stem.starts_with(&want) && best.as_ref().is_none_or(|(l, _)| stem.len() < *l) {
                best = Some((stem.len(), path));
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod font_tests {
    use super::*;
    #[test]
    fn finds_font_by_family_name() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("Menlo-Bold.ttf"), b"x").unwrap();
        fs::write(sub.join("Menlo.ttc"), b"x").unwrap();
        fs::write(d.path().join("notes.txt"), b"x").unwrap();
        let dirs = [d.path().to_path_buf()];
        assert_eq!(find_font_file(&dirs, "menlo"), Some(sub.join("Menlo.ttc")));
        assert_eq!(find_font_file(&dirs, "Nope"), None);
        assert_eq!(find_font_file(&dirs, ""), None);
    }
}
