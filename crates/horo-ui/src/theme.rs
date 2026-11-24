//! The look: the WebView build's two themes (src/styles), each with four
//! accents, four shades and three contrasts, as the appearance settings
//! choose; plus the font and the message text size, and the window's zoom.
//!
//! - horo (themes/horo.css, the default): slate surfaces, cream text, a gold
//!   accent; Sora for everything read and pressed, Crimson Pro for
//!   headings; compact.
//! - classic (tokens/colors.css): the original neon rave flyer, near-black,
//!   Nunito with Baloo 2 headings; roomier.
//!
//! [`Palette`] is what horo's own views draw with; [`apply`] also points
//! gpui-component's theme at the same colours so its inputs and buttons
//! match. The theme's scales and the zoom are read by [`text`], [`space`]
//! and the rest, so a component writes the design's own pixels.

use std::{
    borrow::Cow,
    sync::atomic::{AtomicU32, Ordering},
};

use gpui::{App, Global, Hsla, Pixels, Rgba, SharedString, px, rgb};
use gpui_component::{Theme, ThemeMode};
use serde::{Deserialize, Serialize};

/// Converted from the WebView build's woff2 files (gpui loads TrueType);
/// the classic theme's and the mono face merged from fontsource's Latin
/// and Latin-extended subsets, at the weights the design uses.
const FONTS: [&[u8]; 15] = [
    include_bytes!("../assets/fonts/Sora-Variable.ttf"),
    include_bytes!("../assets/fonts/CrimsonPro-Variable.ttf"),
    include_bytes!("../assets/fonts/CrimsonPro-VariableItalic.ttf"),
    include_bytes!("../assets/fonts/Nunito-Regular.ttf"),
    include_bytes!("../assets/fonts/Nunito-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Nunito-Bold.ttf"),
    include_bytes!("../assets/fonts/Nunito-ExtraBold.ttf"),
    include_bytes!("../assets/fonts/Baloo2-Medium.ttf"),
    include_bytes!("../assets/fonts/Baloo2-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Baloo2-Bold.ttf"),
    include_bytes!("../assets/fonts/Baloo2-ExtraBold.ttf"),
    include_bytes!("../assets/fonts/Unbounded-Medium.ttf"),
    include_bytes!("../assets/fonts/Unbounded-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
];

/// The mono face, in both themes (`--font-mono`).
pub const MONO_FONT: &str = "JetBrains Mono";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    #[default]
    Horo,
    Classic,
}

/// Which ramp the primary accent comes from. The names are classic's;
/// horo fills the same ramps with gold, clay, heather and sage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    #[default]
    Acid,
    Pink,
    Violet,
    Cyan,
}

/// How far the surfaces are pushed toward black.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shade {
    Dim,
    #[default]
    Dark,
    Deep,
    Black,
}

/// How strongly quiet text and hairlines stand out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Contrast {
    Soft,
    #[default]
    Normal,
    High,
}

impl ThemeName {
    pub const ALL: [ThemeName; 2] = [ThemeName::Horo, ThemeName::Classic];
    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Horo => "horo",
            ThemeName::Classic => "classic",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            ThemeName::Horo => "grounded and compact, in the logo's colours",
            ThemeName::Classic => "the original neon rave flyer",
        }
    }
}

impl Accent {
    pub const ALL: [Accent; 4] = [Accent::Acid, Accent::Pink, Accent::Violet, Accent::Cyan];
    /// What it's called under `theme`.
    pub fn label(self, theme: ThemeName) -> &'static str {
        match (theme, self) {
            (ThemeName::Horo, Accent::Acid) => "gold",
            (ThemeName::Horo, Accent::Pink) => "clay",
            (ThemeName::Horo, Accent::Violet) => "heather",
            (ThemeName::Horo, Accent::Cyan) => "sage",
            (ThemeName::Classic, Accent::Acid) => "acid",
            (ThemeName::Classic, Accent::Pink) => "pink",
            (ThemeName::Classic, Accent::Violet) => "violet",
            (ThemeName::Classic, Accent::Cyan) => "cyan",
        }
    }
}

impl Shade {
    pub const ALL: [Shade; 4] = [Shade::Dim, Shade::Dark, Shade::Deep, Shade::Black];
    pub fn label(self) -> &'static str {
        match self {
            Shade::Dim => "dim",
            Shade::Dark => "dark",
            Shade::Deep => "deep",
            Shade::Black => "near black",
        }
    }
    /// The share of black mixed into the surfaces (tokens/tone.css).
    fn mix(self) -> f32 {
        match self {
            Shade::Dim => 0.,
            Shade::Dark => 0.18,
            Shade::Deep => 0.38,
            Shade::Black => 0.70,
        }
    }
}

impl Contrast {
    pub const ALL: [Contrast; 3] = [Contrast::Soft, Contrast::Normal, Contrast::High];
    pub fn label(self) -> &'static str {
        match self {
            Contrast::Soft => "soft",
            Contrast::Normal => "normal",
            Contrast::High => "high",
        }
    }
    /// Points of opacity quiet text gains (borders get half).
    fn lift(self) -> f32 {
        match self {
            Contrast::Soft => 0.,
            Contrast::Normal => 8.,
            Contrast::High => 20.,
        }
    }
}

/// Everything the appearance settings choose.
#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    pub theme: ThemeName,
    pub accent: Accent,
    pub shade: Shade,
    pub contrast: Contrast,
    /// A family to read in, in front of the theme's own; empty for the
    /// theme's.
    pub font_family: String,
    /// Message text, in px.
    pub message_font_size: f32,
}

/// The range the message text size covers.
pub const MESSAGE_SIZE_MIN: f32 = 12.;
pub const MESSAGE_SIZE_MAX: f32 = 22.;
pub const MESSAGE_SIZE_DEFAULT: f32 = 14.5;

/// The font picker's one-click choices: label, and the family as stored.
pub const FONT_PRESETS: [(&str, &str); 4] = [("default", ""), ("system", "system"), ("baloo", "Baloo 2"), ("mono", MONO_FONT)];

/// One theme's tokens, before shade and contrast.
struct Base {
    /// Each accent ramp's 500, 400 and 600 steps: base, hover, pressed.
    acid: [u32; 3],
    pink: [u32; 3],
    violet: [u32; 3],
    cyan: [u32; 3],
    /// `--ink-950-base` … `--ink-600-base`.
    ink: [u32; 5],
    ink_500: u32,
    text: u32,
    border_tint: u32,
    /// Border opacities: subtle, default, strong.
    border_mix: [f32; 3],
    /// Quiet text opacities: secondary, tertiary.
    text_mix: [f32; 2],
    red: u32,
    amber: u32,
    green: u32,
    overlay: (u32, f32),
    body_font: &'static str,
    heading_font: &'static str,
    scales: Scales,
    radii: [f32; 3],
}

/// How a theme stretches the design's sizes (`--text-scale` and friends).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Scales {
    text: f32,
    heading: f32,
    label: f32,
    density: f32,
    radius: f32,
}

fn base(theme: ThemeName) -> Base {
    match theme {
        ThemeName::Horo => Base {
            acid: [0xF2BA25, 0xF5C94F, 0xD9A114],
            pink: [0xE2795B, 0xEA957C, 0xC9603F],
            violet: [0xA98FC0, 0xBEA8D1, 0x8F72A8],
            cyan: [0x7FBFA9, 0x9DCFBD, 0x5FA48C],
            ink: [0x2B302E, 0x313734, 0x383F3B, 0x414945, 0x525A56],
            ink_500: 0x6C7470,
            text: 0xFBECDC,
            border_tint: 0xFBECDC,
            border_mix: [8., 14., 22.],
            text_mix: [66., 42.],
            red: 0xE46A5A,
            amber: 0xE8954A,
            green: 0x8CC97A,
            overlay: (0x1F2322, 0.78),
            body_font: "Sora Variable",
            heading_font: "Crimson Pro",
            scales: Scales { text: 0.94, heading: 1.12, label: 1.45, density: 0.78, radius: 0.45 },
            radii: [6., 8., 10.],
        },
        ThemeName::Classic => Base {
            acid: [0xC8FF4D, 0xD9FF7A, 0xA6E020],
            pink: [0xFF6187, 0xFF85A2, 0xE8456C],
            violet: [0xB14EFF, 0xC97DFF, 0x8F2FE0],
            cyan: [0x4DE8FF, 0x7EEFFF, 0x22C4DE],
            ink: [0x0B0B0F, 0x111117, 0x191922, 0x22222E, 0x33333F],
            ink_500: 0x54545F,
            text: 0xF4F1F7,
            border_tint: 0xFFFFFF,
            border_mix: [8., 14., 24.],
            text_mix: [64., 40.],
            red: 0xFF5470,
            amber: 0xFFC24D,
            green: 0x5CFFA0,
            overlay: (0x0B0B0F, 0.72),
            body_font: "Nunito",
            heading_font: "Baloo 2",
            scales: Scales { text: 1., heading: 1., label: 1., density: 1., radius: 1. },
            radii: [12., 16., 20.],
        },
    }
}

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// `color-mix(in srgb, tint X%, transparent)`: the tint at that opacity.
fn mix(tint: Hsla, percent: f32) -> Hsla {
    Hsla { a: (percent / 100.0).clamp(0., 1.), ..tint }
}

/// `color-mix(in oklab, colour, #000 share)`, as tokens/tone.css darkens
/// the surfaces: in OKLab black is the origin, so this scales L, a and b.
fn darken(value: u32, share: f32) -> Hsla {
    let to_linear = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let to_srgb = |c: f32| if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1. / 2.4) - 0.055 };
    let c = rgb(value);
    let (r, g, b) = (to_linear(c.r), to_linear(c.g), to_linear(c.b));
    // Linear sRGB to OKLab (Björn Ottosson's matrices).
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let k = 1. - share;
    let (lab_l, lab_a, lab_b) = (
        k * (0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s),
        k * (1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s),
        k * (0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s),
    );
    let l = (lab_l + 0.396_337_78 * lab_a + 0.215_803_76 * lab_b).powi(3);
    let m = (lab_l - 0.105_561_346 * lab_a - 0.063_854_17 * lab_b).powi(3);
    let s = (lab_l - 0.089_484_18 * lab_a - 1.291_485_5 * lab_b).powi(3);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    Rgba { r: to_srgb(r).clamp(0., 1.), g: to_srgb(g).clamp(0., 1.), b: to_srgb(b).clamp(0., 1.), a: 1. }.into()
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
    pub accent_press: Hsla,
    pub accent_secondary: Hsla,
    /// `--accent-tertiary` and `--accent-quaternary`.
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
    pub overlay: Hsla,
    pub radius_sm: Pixels,
    pub radius_md: Pixels,
    pub radius_lg: Pixels,
    pub body_font: SharedString,
    pub heading_font: SharedString,
    pub mono_font: SharedString,
}

impl Global for Palette {}

impl Palette {
    /// The palette `appearance` chooses.
    pub fn build(appearance: &Appearance) -> Palette {
        let b = base(appearance.theme);
        let (shade, lift) = (appearance.shade.mix(), appearance.contrast.lift());
        let ink = |i: usize| darken(b.ink[i], if i == 4 { shade / 2. } else { shade });
        let accent = match appearance.accent {
            Accent::Acid => b.acid,
            Accent::Pink => b.pink,
            Accent::Violet => b.violet,
            Accent::Cyan => b.cyan,
        };
        let (text, border_tint) = (hex(b.text), hex(b.border_tint));
        let body = match appearance.font_family.trim() {
            "" => b.body_font.to_owned(),
            "system" => ".SystemUIFont".to_owned(),
            family => family.to_owned(),
        };
        Palette {
            app: ink(0),
            card: ink(1),
            raised: ink(2),
            inset: ink(3),
            text,
            text_secondary: mix(text, b.text_mix[0] + lift),
            text_tertiary: mix(text, b.text_mix[1] + lift),
            text_on_accent: ink(0),
            accent: hex(accent[0]),
            accent_hover: hex(accent[1]),
            accent_press: hex(accent[2]),
            accent_secondary: hex(b.pink[0]),
            accent_tertiary: hex(b.violet[0]),
            accent_quaternary: hex(b.cyan[0]),
            border_subtle: mix(border_tint, b.border_mix[0] + lift / 2.),
            border: mix(border_tint, b.border_mix[1] + lift / 2.),
            border_strong: mix(border_tint, b.border_mix[2] + lift / 2.),
            online: hex(b.green),
            idle: hex(b.amber),
            offline: hex(b.ink_500),
            danger: hex(b.red),
            overlay: Hsla { a: b.overlay.1, ..hex(b.overlay.0) },
            radius_sm: px(b.radii[0]),
            radius_md: px(b.radii[1]),
            radius_lg: px(b.radii[2]),
            body_font: body.into(),
            heading_font: b.heading_font.into(),
            mono_font: MONO_FONT.into(),
        }
    }
}

/// The theme's scales and the message size, read by the size helpers
/// below on every draw (a change redraws the window).
static TEXT_SCALE: AtomicU32 = AtomicU32::new(0x3f70_a3d7); // 0.94
static HEADING_SCALE: AtomicU32 = AtomicU32::new(0x3f8f_5c29); // 1.12
static LABEL_SCALE: AtomicU32 = AtomicU32::new(0x3fb9_999a); // 1.45
static DENSITY: AtomicU32 = AtomicU32::new(0x3f47_ae14); // 0.78
static RADIUS_SCALE: AtomicU32 = AtomicU32::new(0x3ee6_6666); // 0.45
static MESSAGE_SIZE: AtomicU32 = AtomicU32::new(0x4168_0000); // 14.5

fn read(value: &AtomicU32) -> f32 {
    f32::from_bits(value.load(Ordering::Relaxed))
}

fn write(value: &AtomicU32, to: f32) {
    value.store(to.to_bits(), Ordering::Relaxed);
}

/// `--accent-primary` … `--accent-quaternary` now, as hex: the colours
/// people's names and monograms are drawn in.
static NAME_COLOURS: [AtomicU32; 4] = [AtomicU32::new(0xF2BA25), AtomicU32::new(0xE2795B), AtomicU32::new(0xA98FC0), AtomicU32::new(0x7FBFA9)];

pub fn name_colours() -> [u32; 4] {
    NAME_COLOURS.each_ref().map(|c| c.load(Ordering::Relaxed))
}

/// The current theme's corner scale (for shapes drawn outside [`corner`]).
pub fn radius_scale() -> f32 {
    read(&RADIUS_SCALE)
}

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
    zpx(size * read(&TEXT_SCALE))
}
pub fn heading(size: f32) -> Pixels {
    zpx(size * read(&TEXT_SCALE) * read(&HEADING_SCALE))
}
/// Section labels: the heading face, a size up.
pub fn label(size: f32) -> Pixels {
    zpx(size * read(&TEXT_SCALE) * read(&LABEL_SCALE))
}
pub fn space(size: f32) -> Pixels {
    zpx(size * read(&DENSITY))
}
pub fn corner(size: f32) -> Pixels {
    zpx(size * read(&RADIUS_SCALE))
}
/// Message text, as the appearance settings size it (`--text-message`:
/// the setting's px as they are, not through the theme's text scale).
pub fn message_text() -> Pixels {
    zpx(read(&MESSAGE_SIZE))
}
/// A message of just emoji: big, in proportion to the message size.
pub fn emoji_message_text() -> Pixels {
    zpx(read(&MESSAGE_SIZE) * 32. / MESSAGE_SIZE_DEFAULT)
}

/// Load the fonts. Once, at startup, before [`apply`].
pub fn install(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect()) {
        tracing::warn!("couldn't load horo's fonts: {e}");
    }
    apply(&Appearance::default(), cx);
}

impl Default for Appearance {
    fn default() -> Appearance {
        Appearance {
            theme: ThemeName::default(),
            accent: Accent::default(),
            shade: Shade::default(),
            contrast: Contrast::default(),
            font_family: String::new(),
            message_font_size: MESSAGE_SIZE_DEFAULT,
        }
    }
}

/// Make `appearance` the look: the palette global, the scales, and
/// gpui-component dressed to match. The caller redraws.
pub fn apply(appearance: &Appearance, cx: &mut App) {
    let palette = Palette::build(appearance);
    let b = base(appearance.theme);
    let primary = match appearance.accent {
        Accent::Acid => b.acid[0],
        Accent::Pink => b.pink[0],
        Accent::Violet => b.violet[0],
        Accent::Cyan => b.cyan[0],
    };
    for (slot, colour) in NAME_COLOURS.iter().zip([primary, b.pink[0], b.violet[0], b.cyan[0]]) {
        slot.store(colour, Ordering::Relaxed);
    }
    let scales = b.scales;
    write(&TEXT_SCALE, scales.text);
    write(&HEADING_SCALE, scales.heading);
    write(&LABEL_SCALE, scales.label);
    write(&DENSITY, scales.density);
    write(&RADIUS_SCALE, scales.radius);
    write(&MESSAGE_SIZE, appearance.message_font_size.clamp(MESSAGE_SIZE_MIN, MESSAGE_SIZE_MAX));

    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = palette.body_font.clone();
    theme.mono_font_family = palette.mono_font.clone();
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
    c.primary_active = palette.accent_press;
    c.primary_foreground = palette.text_on_accent;
    c.button_primary = palette.accent;
    c.button_primary_hover = palette.accent_hover;
    c.button_primary_active = palette.accent_press;
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
    c.overlay = palette.overlay;
    cx.set_global(palette);
}

#[cfg(test)]
mod palette_tests {
    use super::*;

    fn close(a: Hsla, rgb_hex: u32) -> bool {
        let (a, b) = (Rgba::from(a), rgb(rgb_hex));
        (a.r - b.r).abs() < 0.01 && (a.g - b.g).abs() < 0.01 && (a.b - b.b).abs() < 0.01
    }

    #[test]
    fn dim_and_soft_are_the_theme_as_drawn() {
        let p = Palette::build(&Appearance { shade: Shade::Dim, contrast: Contrast::Soft, ..Appearance::default() });
        assert!(close(p.app, 0x2B302E));
        assert!((p.text_secondary.a - 0.66).abs() < 1e-4);
        assert!((p.border_strong.a - 0.22).abs() < 1e-4);
    }

    #[test]
    fn shade_darkens_and_contrast_lifts() {
        let dim = Palette::build(&Appearance { shade: Shade::Dim, ..Appearance::default() });
        let black = Palette::build(&Appearance { shade: Shade::Black, contrast: Contrast::High, ..Appearance::default() });
        assert!(black.app.l < dim.app.l);
        assert!(black.app.l > 0.);
        assert!((black.text_secondary.a - 0.86).abs() < 1e-4);
        assert!((black.border_subtle.a - 0.18).abs() < 1e-4);
        // Darkening black's own colour is a no-op; white stays white at 0.
        assert!(close(darken(0xFFFFFF, 0.), 0xFFFFFF));
    }

    #[test]
    fn accents_and_fonts_follow_the_choice() {
        let p = Palette::build(&Appearance { theme: ThemeName::Classic, accent: Accent::Cyan, font_family: "system".into(), ..Appearance::default() });
        assert!(close(p.accent, 0x4DE8FF));
        assert_eq!(p.body_font.as_ref(), ".SystemUIFont");
        assert_eq!(Palette::build(&Appearance::default()).body_font.as_ref(), "Sora Variable");
    }
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
