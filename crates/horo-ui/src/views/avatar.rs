//! Avatars: the picture when there is one and it has loaded, otherwise a
//! monogram, the first letter of a name on one of the theme's accents,
//! picked by the id so it's stable.

use gpui::{AnyElement, App, Div, Hsla, Pixels, div, img, prelude::*, rgb};

use crate::media::Media;

/// The four accents the horo theme recolours (gold, clay, heather, sage).
const COLOURS: [u32; 4] = [0xF2BA25, 0xE2795B, 0xA98FC0, 0x7FBFA9];

fn colour_for(id: &str) -> Hsla {
    let hash = id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    rgb(COLOURS[hash as usize % COLOURS.len()]).into()
}

/// The letter to show for `name`: its first alphanumeric character, so
/// `#rust` and `@alice:example.org` read as R and A.
pub fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

/// `size` across, a circle (or a rounded square with `square`).
pub fn monogram(id: &str, name: &str, size: Pixels, square: bool, ink: Hsla) -> Div {
    let tile = div()
        .size(size)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .bg(colour_for(id))
        .text_color(ink)
        .text_size(size * 0.45)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(initial(name));
    if square { tile.rounded(size * 0.3) } else { tile.rounded_full() }
}

/// `size` across: `mxc`'s picture if it's loaded, the monogram until then
/// (and for good, when there's no picture or it won't load).
pub fn avatar(id: &str, name: &str, mxc: Option<&str>, size: Pixels, square: bool, ink: Hsla, cx: &mut App) -> AnyElement {
    // Twice the size it's drawn at, for Retina screens.
    let pixels = (f32::from(size) * 2.0).round() as u32;
    match mxc.and_then(|mxc| Media::image(mxc, Some((pixels, pixels)), cx)) {
        Some(image) => {
            let picture = img(image).size(size).flex_none().object_fit(gpui::ObjectFit::Cover);
            if square { picture.rounded(size * 0.3).into_any_element() } else { picture.rounded_full().into_any_element() }
        }
        None => monogram(id, name, size, square, ink).into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_skip_sigils() {
        assert_eq!(initial("#rust"), "R");
        assert_eq!(initial("@alice:example.org"), "A");
        assert_eq!(initial("ąžuolas"), "Ą");
        assert_eq!(initial("…"), "?");
    }

    #[test]
    fn the_colour_is_stable_per_id() {
        assert_eq!(colour_for("!a:x"), colour_for("!a:x"));
    }
}
