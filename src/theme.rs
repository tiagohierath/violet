//! Theme tokens (ARCHITECTURE §15): a closed set. There is no muted text
//! token, so gray text cannot be asked for; hierarchy comes from size,
//! weight and the accent. Square corners everywhere, no shadows. Light by
//! default (decided 2026-10-05); the Lua config will be able to supply
//! others. Neutral grays, because the surround changes how color is judged.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, Style, TextStyle, Visuals, vec2,
};

/// The closed token set. Elements take colors only from here.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Around the canvas.
    pub workspace: Color32,
    /// Panel background.
    pub surface: Color32,
    /// Buttons and fields at rest.
    pub raised: Color32,
    /// Buttons and fields under the pointer.
    pub hover: Color32,
    /// The amount part of a value field.
    pub fill: Color32,
    pub border: Color32,
    pub text: Color32,
    /// The selected state, the value marker, focus.
    pub accent: Color32,
    /// Text and icons drawn on the accent.
    pub on_accent: Color32,
    /// Errors and warnings.
    pub alert: Color32,
    /// The brush outline is drawn twice, light under dark, so it shows on
    /// any paint.
    pub cursor_light: Color32,
    pub cursor_dark: Color32,
}

/// The bold family. Weight, not gray, makes hierarchy.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

impl Theme {
    pub const LIGHT: Theme = Theme {
        workspace: Color32::from_rgb(0xa8, 0xa8, 0xa8),
        surface: Color32::from_rgb(0xf4, 0xf4, 0xf3),
        raised: Color32::from_rgb(0xe6, 0xe6, 0xe4),
        hover: Color32::from_rgb(0xd9, 0xd9, 0xd6),
        fill: Color32::from_rgb(0xc8, 0xc8, 0xc4),
        border: Color32::from_rgb(0xd0, 0xd0, 0xcd),
        text: Color32::from_rgb(0x17, 0x17, 0x17),
        accent: Color32::from_rgb(0x1c, 0x3a, 0x8c),
        on_accent: Color32::WHITE,
        alert: Color32::from_rgb(0xb0, 0x24, 0x1c),
        cursor_light: Color32::WHITE,
        cursor_dark: Color32::BLACK,
    };

    /// Installs the theme as egui's style, replacing its defaults outright.
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_theme(egui::Theme::Light);
        ctx.set_fonts(fonts());
        let theme = *self;
        ctx.all_styles_mut(move |style| theme.style(style));
    }

    fn style(&self, style: &mut Style) {
        let mut visuals = Visuals::light();
        // Every text color is the one text token, or on_accent on the
        // accent. No override, so selected rows can still invert.
        visuals.override_text_color = None;
        visuals.weak_text_color = Some(self.text);
        visuals.hyperlink_color = self.accent;
        visuals.panel_fill = self.surface;
        visuals.window_fill = self.surface;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        visuals.window_corner_radius = CornerRadius::ZERO;
        visuals.menu_corner_radius = CornerRadius::ZERO;
        visuals.window_shadow = Shadow::NONE;
        visuals.popup_shadow = Shadow::NONE;
        visuals.faint_bg_color = self.raised;
        visuals.extreme_bg_color = Color32::WHITE;
        visuals.code_bg_color = self.raised;
        visuals.text_edit_bg_color = Some(Color32::WHITE);
        visuals.selection.bg_fill = self.accent;
        visuals.selection.stroke = Stroke::new(1.0, self.on_accent);
        visuals.text_cursor.stroke = Stroke::new(2.0, self.accent);
        visuals.warn_fg_color = self.alert;
        visuals.error_fg_color = self.alert;
        visuals.window_highlight_topmost = false;
        let widgets = &mut visuals.widgets;
        for (widget, bg, fg) in [
            (&mut widgets.noninteractive, self.surface, self.text),
            (&mut widgets.inactive, self.raised, self.text),
            (&mut widgets.hovered, self.hover, self.text),
            (&mut widgets.active, self.accent, self.on_accent),
            (&mut widgets.open, self.hover, self.text),
        ] {
            widget.bg_fill = bg;
            widget.weak_bg_fill = bg;
            widget.fg_stroke = Stroke::new(1.5, fg);
            widget.corner_radius = CornerRadius::ZERO;
            widget.expansion = 0.0;
        }
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        widgets.hovered.bg_stroke = Stroke::new(1.0, self.accent);
        widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        widgets.open.bg_stroke = Stroke::new(1.0, self.border);
        style.visuals = visuals;

        // Pictogrep's blueprint: spacing from 4/8/12/16/24/32, controls 28
        // or 36 tall, type 14 and 12 in two weights.
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(12.0, 8.0);
        style.spacing.interact_size = vec2(36.0, 28.0);
        style.spacing.slider_width = 160.0;
        style.spacing.window_margin = Margin::same(16);
        style.spacing.menu_margin = Margin::same(4);
        style.spacing.icon_width = 16.0;
        style.spacing.icon_width_inner = 10.0;
        style.spacing.icon_spacing = 8.0;
        style.text_styles = [
            (TextStyle::Heading, bold(14.0)),
            (TextStyle::Body, regular(14.0)),
            (TextStyle::Button, regular(14.0)),
            (TextStyle::Small, regular(14.0)),
            (TextStyle::Monospace, FontId::monospace(14.0)),
        ]
        .into();
    }
}

/// Geist Mono for the whole UI, embedded so startup never asks fontconfig
/// (ARCHITECTURE §15). egui's own fonts stay behind it as fallbacks.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let embed = |bytes: &'static [u8]| Arc::new(FontData::from_static(bytes));
    fonts.font_data.insert(
        "mono".into(),
        embed(include_bytes!("../assets/fonts/GeistMono-Regular.ttf")),
    );
    fonts.font_data.insert(
        "mono-bold".into(),
        embed(include_bytes!("../assets/fonts/GeistMono-Bold.ttf")),
    );
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |first: &str| {
        let mut list = vec![first.to_owned()];
        list.extend(fallback.iter().cloned());
        list
    };
    fonts.families.insert(FontFamily::Proportional, family("mono"));
    fonts.families.insert(FontFamily::Monospace, family("mono"));
    fonts.families.insert(FontFamily::Name("bold".into()), family("mono-bold"));
    fonts
}
