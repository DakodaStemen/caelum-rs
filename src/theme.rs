use anyhow::{Result, bail};
use eframe::egui::{Color32, Context, CornerRadius, Stroke, Vec2, Visuals, epaint::Shadow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: String,
    pub ui: BTreeMap<String, String>,
    pub terminal: BTreeMap<String, String>,
    #[serde(default)]
    pub syntax: BTreeMap<String, String>,
}
static BUILTIN: std::sync::LazyLock<BTreeMap<String, Theme>> = std::sync::LazyLock::new(|| {
    let mut themes: BTreeMap<String, Theme> =
        serde_json::from_str(include_str!("../assets/themes.json")).expect("bundled themes");
    for (id, t) in &mut themes {
        t.id = id.clone();
    }
    themes
});
pub fn presets() -> BTreeMap<String, Theme> {
    BUILTIN.clone()
}
pub fn color(hex: &str) -> Option<Color32> {
    let h = hex.strip_prefix('#')?;
    let h = if h.len() == 3 {
        h.chars().flat_map(|c| [c, c]).collect()
    } else {
        h.to_string()
    };
    if ![6, 8].contains(&h.len()) || !h.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(&h, 16).ok()?;
    Some(if h.len() == 6 {
        Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
    } else {
        Color32::from_rgba_unmultiplied((n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8)
    })
}
impl Theme {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || !["light", "dark"].contains(&self.kind.as_str()) {
            bail!("Theme needs a name and a light/dark kind");
        }
        let reference = &BUILTIN["starlight"];
        for (values, keys) in [
            (&self.ui, &reference.ui),
            (&self.terminal, &reference.terminal),
        ] {
            for key in keys.keys() {
                if values.get(key).and_then(|v| color(v)).is_none() {
                    bail!("Invalid or missing theme color: {key}");
                }
            }
        }
        Ok(())
    }
    pub fn ui(&self, key: &str) -> Color32 {
        self.ui
            .get(key)
            .and_then(|s| color(s))
            .unwrap_or(Color32::WHITE)
    }
    pub fn term(&self, key: &str) -> Color32 {
        self.terminal
            .get(key)
            .and_then(|s| color(s))
            .unwrap_or(Color32::WHITE)
    }
    /// Code color for a token kind; themes without a `syntax` table borrow terminal colors.
    pub fn syn(&self, key: &str) -> Color32 {
        if let Some(c) = self.syntax.get(key).and_then(|s| color(s)) {
            return c;
        }
        match key {
            "comment" => self.ui("muted"),
            "keyword" => self.term("brightMagenta"),
            "string" => self.term("green"),
            "number" | "constant" | "parameter" => self.term("yellow"),
            "function" => self.term("blue"),
            "type" | "operator" | "escape" => self.term("cyan"),
            "tag" => self.term("red"),
            "attribute" => self.term("brightYellow"),
            "punctuation" => self.ui("textSecondary"),
            _ => self.term("foreground"),
        }
    }
    pub fn apply(&self, ctx: &Context) {
        let mut v = if self.kind == "light" {
            Visuals::light()
        } else {
            Visuals::dark()
        };
        v.override_text_color = Some(self.ui("text"));
        v.panel_fill = self.ui("panel");
        v.window_fill = self.ui("surface");
        v.extreme_bg_color = self.ui("background");
        v.selection.bg_fill = self.ui("button");
        v.selection.stroke.color = self.ui("textStrong");
        v.widgets.inactive.bg_fill = self.ui("borderStrong");
        v.widgets.inactive.weak_bg_fill = self.ui("surface");
        v.widgets.inactive.bg_stroke.color = self.ui("border");
        v.widgets.hovered.bg_fill = self.ui("tabActive");
        v.widgets.hovered.weak_bg_fill = self.ui("tabActive");
        v.widgets.active.bg_fill = self.ui("button");
        v.widgets.active.weak_bg_fill = self.ui("button");
        v.hyperlink_color = self.ui("accent");
        let round = CornerRadius::same(7);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = round;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1_f32, self.ui("border"));
        v.widgets.hovered.bg_stroke = Stroke::new(1_f32, self.ui("accent"));
        v.widgets.active.bg_stroke = Stroke::new(1_f32, self.ui("accent"));
        v.widgets.hovered.fg_stroke.color = self.ui("textStrong");
        v.widgets.active.fg_stroke.color = self.ui("textStrong");
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1_f32, self.ui("borderStrong"));
        v.window_shadow = Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(if self.kind == "light" { 50 } else { 140 }),
        };
        v.selection.bg_fill = self.ui("accent").gamma_multiply(0.35);
        v.text_cursor.stroke = Stroke::new(2_f32, self.ui("accent"));
        ctx.set_visuals(v);
        ctx.style_mut(|st| {
            st.spacing.item_spacing = Vec2::new(8., 7.);
            st.spacing.button_padding = Vec2::new(11., 6.);
            st.spacing.interact_size.y = 28.;
            st.spacing.scroll.bar_width = 8.;
            st.spacing.slider_width = 180.;
            st.spacing.slider_rail_height = 6.;
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_themes_complete() {
        let p = presets();
        assert_eq!(p.len(), 11);
        for t in p.values() {
            t.validate().unwrap();
            for key in crate::syntax::KINDS {
                assert!(t.syntax.contains_key(*key), "{} lacks {key}", t.name);
            }
        }
    }
    #[test]
    fn invalid_colors_rejected() {
        for s in ["red", "#xxyyzz", "#12345", "#😀abc"] {
            assert!(color(s).is_none());
        }
    }
}
