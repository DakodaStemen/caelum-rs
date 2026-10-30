use super::*;

impl App {
    pub(super) fn layout(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        node: &mut Pane,
        ctx: &egui::Context,
        theme: &Theme,
    ) {
        if let Pane::Split {
            id,
            axis,
            ratio,
            first,
            second,
        } = node
        {
            let horizontal = *axis == Axis::Horizontal;
            let length = if horizontal {
                rect.width()
            } else {
                rect.height()
            };
            let split = length * (*ratio);
            let (a, handle, b) = if horizontal {
                (
                    Rect::from_min_max(
                        rect.min,
                        Pos2::new(rect.left() + split - 3., rect.bottom()),
                    ),
                    Rect::from_min_max(
                        Pos2::new(rect.left() + split - 3., rect.top()),
                        Pos2::new(rect.left() + split + 3., rect.bottom()),
                    ),
                    Rect::from_min_max(Pos2::new(rect.left() + split + 3., rect.top()), rect.max),
                )
            } else {
                (
                    Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + split - 3.)),
                    Rect::from_min_max(
                        Pos2::new(rect.left(), rect.top() + split - 3.),
                        Pos2::new(rect.right(), rect.top() + split + 3.),
                    ),
                    Rect::from_min_max(Pos2::new(rect.left(), rect.top() + split + 3.), rect.max),
                )
            };
            let response = ui.interact(handle, egui::Id::new(id.clone()), Sense::drag());
            if response.dragged() {
                let delta = response.drag_delta();
                *ratio = (*ratio
                    + if horizontal {
                        delta.x / length
                    } else {
                        delta.y / length
                    })
                .clamp(0.1, 0.9);
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(if horizontal {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
                ui.painter().rect_filled(handle, 0., theme.ui("accent"));
            }
            self.layout(ui, a, first, ctx, theme);
            self.layout(ui, b, second, ctx, theme);
            return;
        }
        let id = node.id().to_string();
        let selected = id == self.focused;
        let mut focus_requested = false;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(&id)
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect);
        let border = if selected {
            theme.ui("accent")
        } else {
            theme.ui("border")
        };
        egui::Frame::new()
            .fill(theme.term("background"))
            .stroke(Stroke::new(1_f32, border))
            .show(&mut child, |ui| {
                let inner = (rect.size() - Vec2::splat(4.)).max(Vec2::splat(1.));
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                if self.pane_header(ui, node, &id, selected, theme) {
                    focus_requested = true;
                }
                ui.separator();
                match node {
                    Pane::Terminal { cwd, command, .. } => {
                        focus_requested |=
                            self.terminal_pane(ui, ctx, theme, &id, selected, cwd, command);
                    }
                    Pane::Files { path, .. } => self.files_pane(ui, theme, &id, path),
                    Pane::Editor { path, .. } => {
                        focus_requested |= self.editor_pane(ui, theme, &id, path);
                    }
                    Pane::System { .. } => self.system_pane(ui, theme),
                    Pane::Processes { .. } => self.processes_pane(ui, &id),
                    Pane::Browser { url, .. } => Self::browser_pane(ui, ctx, url),
                    _ => {}
                }
            });
        if focus_requested {
            self.focused = id;
        }
    }
    pub(super) fn pane_header(
        &mut self,
        ui: &mut egui::Ui,
        node: &Pane,
        id: &str,
        selected: bool,
        theme: &Theme,
    ) -> bool {
        let mut focus_requested = false;
        ui.horizontal(|ui| {
            let label = match node {
                Pane::Terminal { .. } => self
                    .terminals
                    .get(id)
                    .map(|s| s.title.clone())
                    .unwrap_or_else(|| "Terminal".into()),
                Pane::Files { .. } => "Files".into(),
                Pane::Editor { path, .. } => path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                Pane::System { .. } => "System".into(),
                Pane::Processes { .. } => "Processes".into(),
                Pane::Browser { .. } => "Browser link".into(),
                _ => String::new(),
            };
            let kind = match node {
                Pane::Terminal { .. } => "shell",
                Pane::Files { .. } => "files",
                Pane::Editor { .. } => "editor",
                Pane::System { .. } => "system",
                Pane::Processes { .. } => "processes",
                Pane::Browser { .. } => "link",
                _ => "",
            };
            dot(
                ui,
                if selected {
                    theme.ui("accent")
                } else {
                    theme.ui("borderStrong")
                },
            );
            let title = RichText::new(label).small().color(if selected {
                theme.ui("textStrong")
            } else {
                theme.ui("textSecondary")
            });
            let title = if selected { title.strong() } else { title };
            if ui
                .add(egui::Label::new(title).truncate().sense(Sense::click()))
                .clicked()
            {
                focus_requested = true;
            }
            if self.documents.get(id).is_some_and(Document::dirty)
                && matches!(node, Pane::Editor { .. })
            {
                ui.colored_label(theme.syn("number"), "● unsaved");
            }
            let roomy = ui.available_width() > 190.;
            if roomy {
                ui.label(RichText::new(kind).size(10.).color(theme.ui("muted")));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("×").frame(false).small())
                    .on_hover_text("Close pane (Ctrl+Shift+W)")
                    .clicked()
                {
                    self.focused = id.to_string();
                    self.pending = Some(PendingClose::Pane);
                }
                if roomy
                    && ui
                        .add(egui::Button::new("Zoom").frame(false).small())
                        .on_hover_text("Zoom or restore (Ctrl+Shift+Enter)")
                        .clicked()
                {
                    self.focused = id.to_string();
                    self.zoom = if self.zoom.is_some() {
                        None
                    } else {
                        Some(id.to_string())
                    };
                }
            });
        });
        focus_requested
    }
    pub(super) fn files_pane(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        id: &str,
        path: &mut PathBuf,
    ) {
        let view = self.files.entry(id.to_string()).or_default();
        if view.input_for.as_ref() != Some(&*path) {
            view.path_input = path.display().to_string();
            view.input_for = Some(path.clone());
        }
        // Shows the last listing until the background one for this path arrives.
        view.poll(path, ui.ctx());
        ui.horizontal(|ui| {
            if ui.small_button("Up").clicked()
                && let Some(parent) = path.parent()
            {
                *path = parent.to_path_buf();
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut view.path_input)
                    .desired_width(ui.available_width()),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let requested = PathBuf::from(&view.path_input);
                *path = if requested.is_absolute() {
                    requested
                } else {
                    path.join(requested)
                };
            }
        });
        ui.add(egui::TextEdit::singleline(&mut view.filter).hint_text("Filter files…"));
        if !view.error.is_empty() {
            ui.colored_label(theme.ui("danger"), &view.error);
        }
        let mut open = None;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &view.entries {
                    let marker = if entry.directory { "▸" } else { "·" };
                    let color = if entry.directory {
                        theme.syn("function")
                    } else {
                        theme.ui("text")
                    };
                    let label = RichText::new(format!("{marker}  {}", entry.name)).color(color);
                    let response = ui.selectable_label(false, label);
                    if response.double_clicked() {
                        if entry.directory {
                            *path = entry.path.clone();
                        } else {
                            open = Some(entry.path.clone());
                        }
                    }
                }
            });
        if let Some(path) = open {
            self.pending_open = Some(path);
        }
    }
    pub(super) fn system_pane(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        self.system.refresh(ui.ctx());
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(16.);
            for (name, value) in [
                ("LOAD AVERAGE", &self.system.load),
                ("MEMORY", &self.system.memory),
                ("UPTIME", &self.system.uptime),
            ] {
                ui.label(RichText::new(name).small().color(theme.ui("muted")));
                ui.label(RichText::new(value).size(23.).color(theme.ui("accent")));
                ui.add_space(20.);
            }
            let cpus = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1);
            ui.label(format!("{cpus} logical CPUs"));
            ui.label(std::env::consts::OS);
        });
    }
    pub(super) fn processes_pane(&mut self, ui: &mut egui::Ui, id: &str) {
        self.system.refresh(ui.ctx());
        ui.add(
            egui::TextEdit::singleline(&mut self.process_filter).hint_text("Filter name or PID…"),
        );
        let filter = self.process_filter.to_lowercase();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new(("processes", id))
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("PID");
                        ui.strong("Process");
                        ui.strong("RSS");
                        ui.end_row();
                        for (pid, name, rss) in &self.system.processes {
                            if name.to_lowercase().contains(&filter) || pid.contains(&filter) {
                                ui.monospace(pid);
                                ui.label(name);
                                ui.label(rss);
                                ui.end_row();
                            }
                        }
                    });
            });
    }
    pub(super) fn browser_pane(ui: &mut egui::Ui, ctx: &egui::Context, url: &mut String) {
        ui.label("Imported browser link");
        ui.text_edit_singleline(url);
        if ui.button("Open in system browser").clicked()
            && (url.starts_with("https://") || url.starts_with("http://"))
        {
            ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
        }
        ui.label("Embedded browsing is pending. The link is preserved in this workspace.");
    }
}
/// Three-bar menu toggle drawn directly so it needs no icon font.
pub(super) fn menu_button(ui: &mut egui::Ui, theme: &Theme, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28_f32), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Toggle sidebar")
    });
    let fill = if open {
        theme.ui("tabActive")
    } else if response.hovered() {
        theme.ui("surface")
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 7, fill);
    let color = if open || response.hovered() {
        theme.ui("accent")
    } else {
        theme.ui("textSecondary")
    };
    for dy in [-5_f32, 0., 5.] {
        let y = rect.center().y + dy;
        ui.painter().line_segment(
            [
                Pos2::new(rect.center().x - 7., y),
                Pos2::new(rect.center().x + 7., y),
            ],
            Stroke::new(1.8_f32, color),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
pub(super) fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(9.), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}
pub(super) fn badge(ui: &mut egui::Ui, theme: &Theme, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(Stroke::new(1_f32, color.gamma_multiply(0.5)))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.).strong().color(color));
        });
    let _ = theme;
}
