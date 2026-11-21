//! The "horo" theme (src/styles/themes/horo.css, the WebView build's
//! default): slate surfaces, cream text, a gold accent; Sora for everything
//! read and pressed, Crimson Pro for headings and section labels.
//!
//! [`Palette`] is what horo's own views draw with; [`install`] also points
//! gpui-component's theme at the same colours so its inputs and buttons
//! match.

use std::borrow::Cow;

use gpui::{App, Global, Hsla, Pixels, SharedString, px, rgb};
use gpui_component::{Theme, ThemeMode};

pub const BODY_FONT: &str = "Sora";
pub const HEADING_FONT: &str = "Crimson Pro";

/// Converted from the WebView build's woff2 files (gpui loads TrueType).
const FONTS: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/Sora-Variable.ttf"),
    include_bytes!("../assets/fonts/CrimsonPro-Variable.ttf"),
    include_bytes!("../assets/fonts/CrimsonPro-VariableItalic.ttf"),
];

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// `color-mix(in srgb, tint X%, transparent)`, as tokens/tone.css does for
/// borders and quiet text: the tint at that opacity.
fn mix(tint: Hsla, percent: f32) -> Hsla {
    Hsla { a: percent / 100.0, ..tint }
}

/// Colours and sizes by role, as the CSS tokens name them.
#[derive(Debug, Clone)]
pub struct Palette {
    /// `--surface-app`: the window, behind everything.
    pub app: Hsla,
    /// `--surface-card`: panels (the room list).
    pub card: Hsla,
    /// `--surface-card-raised`: a hovered or selected row.
    pub raised: Hsla,
    /// `--surface-inset`: inputs, wells.
    pub inset: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_on_accent: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_secondary: Hsla,
    pub border_subtle: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub online: Hsla,
    pub idle: Hsla,
    /// `--status-offline` (ink-500).
    pub offline: Hsla,
    pub danger: Hsla,
    pub radius_sm: Pixels,
    pub radius_md: Pixels,
    pub radius_lg: Pixels,
    pub body_font: SharedString,
    pub heading_font: SharedString,
    /// Code: the platform's monospace face, as gpui-component finds it.
    pub mono_font: SharedString,
}

impl Global for Palette {}

impl Palette {
    pub fn horo() -> Palette {
        let fog_100 = hex(0xFBECDC);
        Palette {
            app: hex(0x2B302E),
            card: hex(0x313734),
            raised: hex(0x383F3B),
            inset: hex(0x414945),
            text: fog_100,
            text_secondary: mix(fog_100, 66.0),
            text_tertiary: mix(fog_100, 42.0),
            text_on_accent: hex(0x2B302E),
            accent: hex(0xF2BA25),
            accent_hover: hex(0xF5C94F),
            accent_secondary: hex(0xE2795B),
            border_subtle: mix(fog_100, 8.0),
            border: mix(fog_100, 14.0),
            border_strong: mix(fog_100, 22.0),
            online: hex(0x8CC97A),
            idle: hex(0xE8954A),
            offline: hex(0x6C7470),
            danger: hex(0xE46A5A),
            radius_sm: px(6.),
            radius_md: px(8.),
            radius_lg: px(10.),
            body_font: BODY_FONT.into(),
            heading_font: HEADING_FONT.into(),
            mono_font: "Menlo".into(),
        }
    }
}

/// Load the fonts, make the palette global, and dress gpui-component in it.
/// After `gpui_component::init`.
pub fn install(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect()) {
        tracing::warn!("couldn't load horo's fonts: {e}");
    }
    let mut palette = Palette::horo();

    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = BODY_FONT.into();
    theme.font_size = px(14.);
    theme.radius = palette.radius_md;
    theme.radius_lg = palette.radius_lg;
    let c = &mut theme.colors;
    c.background = palette.app;
    c.foreground = palette.text;
    c.border = palette.border;
    c.input = palette.border;
    c.ring = palette.accent;
    c.caret = palette.accent;
    c.selection = mix(palette.accent, 30.0);
    c.muted = palette.inset;
    c.muted_foreground = palette.text_secondary;
    c.accent = palette.raised;
    c.accent_foreground = palette.text;
    c.primary = palette.accent;
    c.primary_hover = palette.accent_hover;
    c.primary_active = hex(0xD9A114);
    c.primary_foreground = palette.text_on_accent;
    c.button_primary = palette.accent;
    c.button_primary_hover = palette.accent_hover;
    c.button_primary_active = hex(0xD9A114);
    c.button_primary_foreground = palette.text_on_accent;
    c.secondary = palette.raised;
    c.secondary_hover = palette.inset;
    c.secondary_foreground = palette.text;
    c.popover = palette.card;
    c.popover_foreground = palette.text;
    c.danger = palette.danger;
    c.list = palette.card;
    c.list_hover = palette.raised;
    c.list_active = palette.raised;
    c.scrollbar_thumb = palette.border_strong;
    c.overlay = Hsla { a: 0.78, ..hex(0x1F2322) };

    palette.mono_font = theme.mono_font_family.clone();
    cx.set_global(palette);
}
