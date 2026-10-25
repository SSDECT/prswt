//! DSK-07: the look of the desktop app (design "B · Modern workspace",
//! canvas `PRSWT desktop redesign`): colour tokens for light and dark,
//! the Inter typeface, and egui visuals built from those tokens.
//!
//! Views never hard-code colours: they ask [`Tokens::for_ui`]. Fonts are
//! Inter 4.1 (SIL Open Font License, `assets/fonts/Inter-LICENSE.txt`),
//! embedded so the app looks the same on Windows, Linux and macOS.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    Theme, Ui, Vec2, Visuals,
};

/// Font family name of the semibold face (titles, selected items, numbers).
pub const SEMIBOLD: &str = "semibold";

/// Inter Regular, the body face.
const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
/// Inter SemiBold, the emphasis face.
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

/// Semantic colours (theme tokens) for the active theme.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    /// Window ground behind the cards.
    pub bg: Color32,
    /// Cards, tables and the header.
    pub surface: Color32,
    /// The left navigation panel.
    pub sidebar: Color32,
    /// Card borders and separators.
    pub line: Color32,
    /// Main text.
    pub ink: Color32,
    /// Secondary text and labels.
    pub muted: Color32,
    /// Brand accent: primary buttons, the selected view, the data date.
    pub accent: Color32,
    /// Accent wash: the selected row and navigation item.
    pub accent_soft: Color32,
    /// Activities on the longest (critical) path. Orange, so it differs from
    /// [`Tokens::normal`] in lightness as well as hue.
    pub critical: Color32,
    /// Other activity bars.
    pub normal: Color32,
    /// Total-float extension lines.
    pub float: Color32,
    /// Milestone diamonds.
    pub milestone: Color32,
    /// WBS summary bars.
    pub summary: Color32,
    /// Light grid lines inside charts.
    pub grid: Color32,
    /// Data-date marker.
    pub data_date: Color32,
    /// Good news (float left, nothing overallocated).
    pub ok: Color32,
    /// Destructive actions.
    pub danger: Color32,
}

impl Tokens {
    /// The light palette.
    pub const LIGHT: Self = Self {
        bg: Color32::from_rgb(0xF4, 0xF4, 0xF1),
        surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        sidebar: Color32::from_rgb(0xFB, 0xFB, 0xFA),
        line: Color32::from_rgb(0xE4, 0xE5, 0xE2),
        ink: Color32::from_rgb(0x16, 0x18, 0x1D),
        muted: Color32::from_rgb(0x5E, 0x64, 0x70),
        accent: Color32::from_rgb(0x43, 0x52, 0xC9),
        accent_soft: Color32::from_rgb(0xEC, 0xEE, 0xFA),
        critical: Color32::from_rgb(0xE0, 0x66, 0x2F),
        normal: Color32::from_rgb(0x4C, 0x78, 0xE0),
        float: Color32::from_rgb(0xA3, 0xA8, 0xB0),
        milestone: Color32::from_rgb(0x16, 0x18, 0x1D),
        summary: Color32::from_rgb(0x9A, 0xA0, 0xAA),
        grid: Color32::from_rgb(0xEF, 0xEF, 0xEC),
        data_date: Color32::from_rgb(0x43, 0x52, 0xC9),
        ok: Color32::from_rgb(0x1F, 0x7A, 0x4D),
        danger: Color32::from_rgb(0xA3, 0x26, 0x1A),
    };

    /// The dark palette.
    pub const DARK: Self = Self {
        bg: Color32::from_rgb(0x14, 0x16, 0x1A),
        surface: Color32::from_rgb(0x1C, 0x1F, 0x24),
        sidebar: Color32::from_rgb(0x18, 0x1A, 0x1E),
        line: Color32::from_rgb(0x2D, 0x31, 0x38),
        ink: Color32::from_rgb(0xE8, 0xE9, 0xEC),
        muted: Color32::from_rgb(0x9B, 0xA1, 0xAB),
        accent: Color32::from_rgb(0x8A, 0x95, 0xF2),
        accent_soft: Color32::from_rgb(0x27, 0x2C, 0x4A),
        critical: Color32::from_rgb(0xF0, 0x86, 0x4E),
        normal: Color32::from_rgb(0x6A, 0x93, 0xF0),
        float: Color32::from_rgb(0x6B, 0x71, 0x7C),
        milestone: Color32::from_rgb(0xE8, 0xE9, 0xEC),
        summary: Color32::from_rgb(0x7A, 0x80, 0x8A),
        grid: Color32::from_rgb(0x24, 0x27, 0x2D),
        data_date: Color32::from_rgb(0x8A, 0x95, 0xF2),
        ok: Color32::from_rgb(0x5C, 0xC2, 0x8C),
        danger: Color32::from_rgb(0xF2, 0x8B, 0x80),
    };

    /// Tokens for the theme `ui` is drawn in.
    pub fn for_ui(ui: &Ui) -> Self {
        if ui.visuals().dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// The semibold font at `size` points.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Installs the fonts, text sizes, spacing and both themes' visuals.
/// The desktop shell calls it once at start-up.
pub fn apply_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("inter".into(), FontData::from_static(INTER_REGULAR).into());
    fonts.font_data.insert(
        "inter-semibold".into(),
        FontData::from_static(INTER_SEMIBOLD).into(),
    );
    // egui's own fonts stay as fallbacks (symbols, emoji).
    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    let mut bold = vec!["inter-semibold".to_owned()];
    bold.extend(fallbacks);
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), bold);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Heading, semibold(18.0)),
            (TextStyle::Body, FontId::proportional(13.5)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.window_margin = Margin::same(16);
    });
    ctx.set_visuals_of(Theme::Light, visuals(&Tokens::LIGHT, false));
    ctx.set_visuals_of(Theme::Dark, visuals(&Tokens::DARK, true));
}

/// egui visuals built from a palette: flat cards, 8-point corners, thin
/// borders and an accent-coloured selection.
fn visuals(t: &Tokens, dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r = CornerRadius::same(7);
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.window_stroke = Stroke::new(1.0, t.line);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 28 }),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.faint_bg_color = if dark {
        Color32::from_rgb(0x20, 0x23, 0x29)
    } else {
        Color32::from_rgb(0xF7, 0xF7, 0xF5)
    };
    v.extreme_bg_color = if dark {
        Color32::from_rgb(0x15, 0x17, 0x1B)
    } else {
        Color32::from_rgb(0xF7, 0xF7, 0xF5)
    };
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.override_text_color = None;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.surface;
    w.noninteractive.weak_bg_fill = t.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.ink);
    w.noninteractive.corner_radius = r;
    for (state, fill, stroke) in [
        (&mut w.inactive, t.surface, t.line),
        (&mut w.hovered, v.faint_bg_color, t.float),
        (&mut w.active, t.accent_soft, t.accent),
        (&mut w.open, t.accent_soft, t.line),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, t.ink);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    v
}
