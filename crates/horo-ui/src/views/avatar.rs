//! A round monogram, until avatars load images: the first letter of a name
//! on one of the theme's accents, picked by the id so it's stable.

use gpui::{Div, Hsla, Pixels, div, prelude::*, rgb};

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
