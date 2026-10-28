use super::panes::{badge, dot};
use super::*;

impl App {
    pub(super) fn top(&mut self, ctx: &egui::Context, theme: &Theme) {
        egui::TopBottomPanel::top("title")
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("chrome"))
                    .stroke(Stroke::new(1_f32, theme.ui("border")))
                    .inner_margin(egui::Margin::symmetric(14, 9)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.image((self.logo.id(), Vec2::splat(24.)));
                    ui.label(
                        RichText::new("Caelum")
                            .size(18.)
                            .strong()
                            .color(theme.ui("textStrong")),
                    );
                    badge(ui, theme, "RUST", theme.ui("accent"));
                    ui.add_space(6.);
                    ui.label(RichText::new("›").color(theme.ui("muted")));
                    ui.label(
                        RichText::new(self.workspace().name.clone())
                            .color(theme.ui("textSecondary")),
                    );
                    ui.label(RichText::new("›").color(theme.ui("muted")));
                    ui.label(RichText::new(self.tab().name.clone()).color(theme.ui("text")));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Appearance")
                            .on_hover_text("Themes, fonts and colors")
                            .clicked()
                        {
                            self.settings = true;
                        }
                        let palette = ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Search commands…   Ctrl+Shift+P")
                                        .color(theme.ui("muted")),
                                )
                                .fill(theme.ui("background"))
                                .stroke(Stroke::new(1_f32, theme.ui("border"))),
                            )
                            .on_hover_text("Every action and theme in one place");
                        if palette.clicked() {
                            self.action(Action::Palette, ctx);
                        }
                    });
                });
            });
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("chrome"))
                    .stroke(Stroke::new(1_f32, theme.ui("border")))
                    .inner_margin(egui::Margin::symmetric(14, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    dot(ui, theme.syn("string"));
                    ui.label(
                        RichText::new(format!("{} sessions", self.terminals.len()))
                            .small()
                            .color(theme.ui("textSecondary")),
                    );
                    ui.label(RichText::new("│").small().color(theme.ui("border")));
                    ui.label(
                        RichText::new(self.cwd().display().to_string())
                            .small()
                            .color(theme.ui("muted")),
                    );
                    if !self.message.is_empty() {
                        ui.label(RichText::new("│").small().color(theme.ui("border")));
                        ui.label(
                            RichText::new(self.message.clone())
                                .small()
                                .strong()
                                .color(theme.ui("accent")),
                        );
                        if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                            self.message.clear();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  v{}",
                                self.theme().name,
                                env!("CARGO_PKG_VERSION")
                            ))
                            .small()
                            .color(theme.ui("muted")),
                        );
                    });
                });
            });
    }
    pub(super) fn sidebar(&mut self, ctx: &egui::Context, theme: &Theme) {
        egui::SidePanel::left("workspaces")
            .default_width(220.)
            .width_range(180. ..=320.)
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("panel"))
                    .stroke(Stroke::new(1_f32, theme.ui("border")))
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 3.;
                section(ui, theme, "WORKSPACES");
                let workspaces: Vec<_> = self
                    .state
                    .workspaces
                    .iter()
                    .map(|w| (w.id.clone(), w.name.clone()))
                    .collect();
                for (id, name) in workspaces {
                    if nav_row(ui, theme, &name, "", self.state.active_workspace == id).clicked() {
                        self.state.active_workspace = id;
                        self.zoom = None;
                        self.ensure_focus();
                    }
                }
                if nav_row(ui, theme, "+  New workspace", "", false).clicked() {
                    self.action(Action::NewWorkspace, ctx);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("Rename").small()).frame(false))
                        .clicked()
                    {
                        self.dialog = Some("Rename workspace".into());
                        self.dialog_input = self.workspace().name.clone();
                    }
                    if self.state.workspaces.len() > 1
                        && ui
                            .add(egui::Button::new(RichText::new("Close").small()).frame(false))
                            .clicked()
                    {
                        self.pending = Some(PendingClose::Workspace);
                    }
                });
                ui.add_space(14.);
                section(ui, theme, "TOOLS");
                for (name, hint, a) in [
                    ("Files", "browse · edit", Action::Files),
                    ("System", "load · memory", Action::System),
                    ("Processes", "live list", Action::Processes),
                    ("SSH", "remote shell", Action::Connect),
                    ("Codex", "assistant", Action::Assistant("codex".into())),
                    ("Claude", "assistant", Action::Assistant("claude".into())),
                ] {
                    if nav_row(ui, theme, name, hint, false).clicked() {
                        self.action(a, ctx);
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("Native Rust · OpenGL")
                            .small()
                            .color(theme.ui("muted")),
                    );
                    ui.label(
                        RichText::new("A little room for big ideas.")
                            .small()
                            .color(theme.ui("textSecondary")),
                    );
                });
            });
    }
    pub(super) fn tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let theme = self.theme();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            let tabs: Vec<_> = self
                .workspace()
                .tabs
                .iter()
                .map(|t| (t.id.clone(), t.name.clone()))
                .collect();
            let count = tabs.len();
            for (id, name) in tabs {
                let active = self.workspace().active_tab == id;
                let (response, close) = tab_chip(ui, &theme, &name, active, count > 1);
                if response.clicked() {
                    self.workspace_mut().active_tab = id.clone();
                    self.zoom = None;
                    self.ensure_focus();
                }
                if close {
                    self.workspace_mut().active_tab = id;
                    self.pending = Some(PendingClose::Tab);
                }
                if response.double_clicked() {
                    self.dialog = Some("Rename tab".into());
                    self.dialog_input = self.tab().name.clone();
                }
            }
            if ui
                .button("+")
                .on_hover_text("New tab (Ctrl+Shift+T)")
                .clicked()
            {
                self.action(Action::NewTab, ctx);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if self.zoom.is_some() {
                        "Restore"
                    } else {
                        "Zoom"
                    })
                    .on_hover_text("Focus this pane (Ctrl+Shift+Enter)")
                    .clicked()
                {
                    self.action(Action::Zoom, ctx);
                }
                if ui
                    .button("Split below")
                    .on_hover_text("Ctrl+Shift+E")
                    .clicked()
                {
                    self.action(Action::Split(Axis::Vertical), ctx);
                }
                if ui
                    .button("Split right")
                    .on_hover_text("Ctrl+Shift+D")
                    .clicked()
                {
                    self.action(Action::Split(Axis::Horizontal), ctx);
                }
            });
        });
        ui.add_space(4.);
    }
}
pub(super) fn section(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.add_space(4.);
    ui.label(
        RichText::new(text)
            .size(10.5)
            .strong()
            .color(theme.ui("muted")),
    );
    ui.add_space(2.);
}
/// A full-width sidebar row with an accent bar when active and an optional right-hand hint.
pub(super) fn nav_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    hint: &str,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let fill = if active {
            theme.ui("tabActive")
        } else if response.hovered() {
            theme.ui("surface")
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 7, fill);
        if active {
            let bar = Rect::from_min_size(
                rect.left_top() + Vec2::new(0., 6.),
                Vec2::new(3., rect.height() - 12.),
            );
            ui.painter().rect_filled(bar, 2, theme.ui("accent"));
        }
        let color = if active || response.hovered() {
            theme.ui("textStrong")
        } else {
            theme.ui("text")
        };
        ui.painter().text(
            rect.left_center() + Vec2::new(12., 0.),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.),
            color,
        );
        if !hint.is_empty() {
            ui.painter().text(
                rect.right_center() - Vec2::new(10., 0.),
                egui::Align2::RIGHT_CENTER,
                hint,
                FontId::proportional(11.),
                theme.ui("muted"),
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// A tab chip; returns the chip response and whether its close button was clicked.
pub(super) fn tab_chip(
    ui: &mut egui::Ui,
    theme: &Theme,
    name: &str,
    active: bool,
    closable: bool,
) -> (egui::Response, bool) {
    let font = FontId::proportional(14.);
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_string(), font, Color32::WHITE);
    let close_w = if closable { 22. } else { 0. };
    let size = Vec2::new(galley.size().x + 28. + close_w, 32.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    let mut closed = false;
    if ui.is_rect_visible(rect) {
        let fill = if active {
            theme.term("background")
        } else if response.hovered() {
            theme.ui("surface")
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 8, fill);
        if active {
            let bar = Rect::from_min_size(
                rect.left_bottom() - Vec2::new(-8., 3.),
                Vec2::new(rect.width() - 16., 3.),
            );
            ui.painter().rect_filled(bar, 2, theme.ui("accent"));
        }
        let color = if active {
            theme.ui("textStrong")
        } else {
            theme.ui("textSecondary")
        };
        ui.painter().text(
            rect.left_center() + Vec2::new(14., -1.),
            egui::Align2::LEFT_CENTER,
            name,
            FontId::proportional(14.),
            color,
        );
        if closable && (active || response.hovered()) {
            let x =
                Rect::from_center_size(rect.right_center() - Vec2::new(16., 1.), Vec2::splat(18.));
            let hit = ui.interact(x, response.id.with("close"), Sense::click());
            if hit.hovered() {
                ui.painter().rect_filled(x, 5, theme.ui("border"));
            }
            ui.painter().text(
                x.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                FontId::proportional(14.),
                theme.ui("muted"),
            );
            closed = hit.on_hover_text("Close tab").clicked();
        }
    }
    (
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
        closed,
    )
}
