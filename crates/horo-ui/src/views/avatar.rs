//! Avatars as ui.tsx's `Avatar` draws them: a rounded square with an ink
//! border, the picture once it has loaded, otherwise a two-letter monogram
//! on one of the theme's four accents, picked by the id (the same hash as
//! `accentFor`, so a room is the same colour in both apps).

use gpui::{AnyElement, App, Div, Hsla, Pixels, div, img, prelude::*, rgb};

use crate::{media::Media, presence::Shown, theme::Palette};

/// FNV-1a over UTF-16 code units, as `hash` in src/lib/display.ts.
fn hash(input: &str) -> u32 {
    input.encode_utf16().fold(2_166_136_261u32, |h, unit| (h ^ unit as u32).wrapping_mul(16_777_619))
}

pub fn accent_for(id: &str) -> Hsla {
    // `--accent-primary` … `--accent-quaternary` as the appearance
    // settings have them (NEON's order in src/lib/display.ts).
    let colours = crate::theme::name_colours();
    rgb(colours[hash(id) as usize % colours.len()]).into()
}

/// Two characters for a tile, as `initialsFor`: the first letters of the
/// first two words ("tekno flyers" → "tf"), else the first two of one word,
/// sigils and the server dropped, emoji and symbols ignored.
pub fn initials(name: &str) -> String {
    let cleaned = name.trim_start_matches(['@', '#', '!', '+']);
    let cleaned = cleaned.split(':').next().unwrap_or(cleaned).trim();
    if cleaned.is_empty() {
        return "??".into();
    }
    let words: Vec<Vec<char>> = cleaned
        .split(|c: char| c.is_whitespace() || matches!(c, '_' | '-' | '.'))
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
        .collect();
    match words.as_slice() {
        [first, second, ..] => [first[0], second[0]].iter().collect::<String>().to_lowercase(),
        [only] => only.iter().take(2).collect::<String>().to_lowercase(),
        [] => cleaned.chars().next().map(String::from).unwrap_or_else(|| "??".into()),
    }
}

/// The corner for a tile `size` across: `round(size * 0.36)`, scaled.
fn corner(size: Pixels) -> Pixels {
    gpui::px((f32::from(size) * 0.36).round() * crate::theme::radius_scale())
}

/// The monogram tile.
pub fn monogram(id: &str, name: &str, size: Pixels, ink: Hsla, border: Hsla) -> Div {
    div()
        .size(size)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(corner(size))
        .border_2()
        .border_color(border)
        .bg(accent_for(id))
        .text_color(ink)
        .text_size(gpui::px((f32::from(size) * 0.37).round().max(10.)))
        .font_weight(gpui::FontWeight::EXTRA_BOLD)
        .child(initials(name))
}

/// `size` across: `mxc`'s picture if it's loaded, the monogram until then
/// (and for good, when there's no picture or it won't load).
pub fn avatar(id: &str, name: &str, mxc: Option<&str>, size: Pixels, cx: &mut App) -> AnyElement {
    let palette = cx.global::<Palette>().clone();
    // Twice the size it's drawn at, for Retina screens.
    let pixels = (f32::from(size) * 2.0).round() as u32;
    match mxc.and_then(|mxc| Media::image(mxc, Some((pixels, pixels)), cx)) {
        Some(image) => div()
            .size(size)
            .flex_none()
            .rounded(corner(size))
            .border_2()
            .border_color(palette.app)
            .bg(palette.card)
            // gpui clips children to a rectangle, whatever the corners: the
            // picture rounds itself, inside the border.
            .child(img(image).size_full().rounded(corner(size) - gpui::px(2.)).object_fit(gpui::ObjectFit::Cover))
            .into_any_element(),
        None => monogram(id, name, size, palette.text_on_accent, palette.app).into_any_element(),
    }
}

/// The status dot, cut into an avatar's bottom-right corner (out of `ring`,
/// the colour behind the avatar). Goes in a `relative` parent.
pub fn presence_dot(shown: Shown, size: Pixels, ring: Hsla, palette: &Palette) -> Div {
    let colour = match shown {
        Shown::Online => palette.online,
        Shown::Away => palette.idle,
        Shown::Offline => palette.offline,
    };
    div().absolute().right(gpui::px(-2.)).bottom(gpui::px(-2.)).size(size).rounded_full().bg(colour).border_2().border_color(ring)
}

/// An avatar with its presence dot, when there's one to show.
pub fn with_presence(face: AnyElement, shown: Option<Shown>, ring: Hsla, palette: &Palette) -> AnyElement {
    div().relative().flex_none().child(face).children(shown.map(|s| presence_dot(s, gpui::px(12.), ring, palette))).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_as_the_webview_build_makes_them() {
        assert_eq!(initials("tekno flyers"), "tf");
        assert_eq!(initials("Empty Room"), "er");
        assert_eq!(initials("m.uwu.lt Admin Room"), "mu");
        assert_eq!(initials("@kii:uwu.gg"), "ki");
        assert_eq!(initials("skitchy 🍁"), "sk");
        assert_eq!(initials("tundra"), "tu");
        assert_eq!(initials("🍁"), "🍁");
    }

    #[test]
    fn the_hash_is_display_ts_fnv1a() {
        // FNV-1a of "a": 0xe40c292c.
        assert_eq!(hash("a"), 0xe40c292c);
        assert_eq!(hash(""), 2_166_136_261);
    }
}
