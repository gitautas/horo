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

/// The family name inside Sora-Variable.ttf (not "Sora": the woff2 the
/// WebView loads is named by CSS, this file names itself).
pub const BODY_FONT: &str = "Sora Variable";
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
    /// `--accent-tertiary` (heather) and `--accent-quaternary` (sage).
    pub accent_tertiary: Hsla,
    pub accent_quaternary: Hsla,
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
            accent_tertiary: hex(0xA98FC0),
            accent_quaternary: hex(0x7FBFA9),
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

/// The horo theme's scales (src/styles/themes/horo.css), so sizes can be
/// written as the React components write them: `text(13.)` is
/// `textSize(13)`, `space(10.)` is `space(10)`, and so on.
pub const TEXT_SCALE: f32 = 0.94;
pub const HEADING_SCALE: f32 = 1.12;
pub const LABEL_SCALE: f32 = 1.45;
pub const DENSITY: f32 = 0.78;
pub const RADIUS_SCALE: f32 = 0.45;

/// The whole window's zoom, as a factor (1 is the design's own size), as
/// the WebView build's page zoom: every size below is multiplied by it,
/// and so is the rem gpui-component sizes its widgets in.
static ZOOM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000); // 1.0

/// The range zoom covers, and one step of it (⌘= / ⌘-).
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 2.;
pub const ZOOM_STEP: f32 = 0.1;

pub fn zoom() -> f32 {
    f32::from_bits(ZOOM.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn set_zoom(zoom: f32) {
    ZOOM.store(zoom.clamp(ZOOM_MIN, ZOOM_MAX).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The zoom one shortcut press away, on the step grid, clamped.
pub fn step_zoom(zoom: f32, up: bool) -> f32 {
    let next = ((zoom + if up { ZOOM_STEP } else { -ZOOM_STEP }) / ZOOM_STEP).round() * ZOOM_STEP;
    next.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// gpui's rem at this zoom (its default is 16 px).
pub fn rem() -> Pixels {
    px(16. * zoom())
}

/// A size in the design's pixels, zoomed.
pub fn zpx(size: f32) -> Pixels {
    px(size * zoom())
}

pub fn text(size: f32) -> Pixels {
    zpx(size * TEXT_SCALE)
}
pub fn heading(size: f32) -> Pixels {
    zpx(size * TEXT_SCALE * HEADING_SCALE)
}
/// Section labels: the heading face, a size up.
pub fn label(size: f32) -> Pixels {
    zpx(size * TEXT_SCALE * LABEL_SCALE)
}
pub fn space(size: f32) -> Pixels {
    zpx(size * DENSITY)
}
pub fn corner(size: f32) -> Pixels {
    zpx(size * RADIUS_SCALE)
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

#[cfg(test)]
mod zoom_tests {
    use super::*;

    #[test]
    fn zoom_steps_on_a_grid_and_stops_at_the_ends() {
        let mut zoom = 1.;
        for _ in 0..3 {
            zoom = step_zoom(zoom, true);
        }
        assert!((zoom - 1.3).abs() < 1e-5);
        assert_eq!(step_zoom(2., true), 2.);
        assert_eq!(step_zoom(0.5, false), 0.5);
        // Off the grid (a slider's 1.04) snaps onto it.
        assert!((step_zoom(1.04, true) - 1.1).abs() < 1e-5);
    }
}
