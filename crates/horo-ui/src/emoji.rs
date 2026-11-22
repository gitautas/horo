//! The Unicode emoji set, prepared for the picker, as src/lib/emoji.ts
//! prepares it: one entry per emoji with skin tones folded into a flag,
//! tones applied on the way out, and a word-start search.
//!
//! The data is unicode-emoji-json 0.9.0 (the WebView build's copy),
//! squeezed to `assets/emoji.tsv`: a `<TAB>group` line (a tab, since
//! an emoji can start with `#`), then `emoji<TAB>name<TAB>tones` per emoji. Parsed once, on first use.

use std::{collections::HashMap, sync::OnceLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emoji {
    /// The character, without a skin tone.
    pub emoji: &'static str,
    /// "grinning face with big eyes"; also what search matches.
    pub name: &'static str,
    pub skin_tones: bool,
}

#[derive(Debug)]
pub struct Group {
    /// Lower-cased, as the picker titles it.
    pub name: String,
    /// An `icons/…` path for the category rail.
    pub icon: &'static str,
    pub emojis: Vec<Emoji>,
}

const DATA: &str = include_str!("../assets/emoji.tsv");

/// Icons for the dataset's nine groups; a group the data adds later gets a
/// neutral one rather than nothing.
fn icon_for(group: &str) -> &'static str {
    match group {
        "Smileys & Emotion" => "icons/smile.svg",
        "People & Body" => "icons/hand.svg",
        "Animals & Nature" => "icons/cat.svg",
        "Food & Drink" => "icons/hamburger.svg",
        "Travel & Places" => "icons/plane.svg",
        "Activities" => "icons/trophy.svg",
        "Objects" => "icons/lightbulb.svg",
        "Symbols" => "icons/heart.svg",
        "Flags" => "icons/flag.svg",
        _ => "icons/ellipsis.svg",
    }
}

pub fn groups() -> &'static [Group] {
    static GROUPS: OnceLock<Vec<Group>> = OnceLock::new();
    GROUPS.get_or_init(|| {
        let mut out: Vec<Group> = Vec::new();
        for line in DATA.lines() {
            if let Some(name) = line.strip_prefix('\t') {
                out.push(Group { name: name.to_lowercase(), icon: icon_for(name), emojis: Vec::new() });
                continue;
            }
            let mut fields = line.split('\t');
            let (Some(emoji), Some(name), Some(tones), Some(group)) = (fields.next(), fields.next(), fields.next(), out.last_mut()) else {
                continue;
            };
            group.emojis.push(Emoji { emoji, name, skin_tones: tones == "1" });
        }
        out
    })
}

fn by_char() -> &'static HashMap<&'static str, &'static Emoji> {
    static BY_CHAR: OnceLock<HashMap<&'static str, &'static Emoji>> = OnceLock::new();
    BY_CHAR.get_or_init(|| groups().iter().flat_map(|g| g.emojis.iter()).map(|e| (e.emoji, e)).collect())
}

/// An emoji's name, for the picker's preview line.
pub fn name_of(emoji: &str) -> Option<&'static str> {
    by_char().get(emoji).map(|e| e.name)
}

/// 0 is the default yellow; 1 to 5 are the Fitzpatrick modifiers.
pub type SkinTone = u8;

const TONE_MODIFIERS: [char; 5] = ['🏻', '🏼', '🏽', '🏾', '🏿'];
const VARIATION_SELECTOR: char = '\u{FE0F}';

/// A swatch each, for the tone buttons; index 0 is the default.
pub const TONE_SAMPLES: [&str; 6] = ["✋", "✋🏻", "✋🏼", "✋🏽", "✋🏾", "✋🏿"];

/// The tone goes straight after the base character: in 👨‍💻 it belongs to
/// the man, not the end, and a base with a variation selector (☝️) drops
/// it, since the modifier already forces emoji presentation.
pub fn with_skin_tone(emoji: &str, tone: SkinTone) -> String {
    let Some(modifier) = tone.checked_sub(1).and_then(|i| TONE_MODIFIERS.get(i as usize)) else {
        return emoji.to_owned();
    };
    let mut chars = emoji.chars();
    let Some(first) = chars.next() else { return String::new() };
    let rest: String = chars.as_str().strip_prefix(VARIATION_SELECTOR).unwrap_or(chars.as_str()).to_owned();
    format!("{first}{modifier}{rest}")
}

/// What picking `emoji` sends, with your tone where it takes one.
pub fn toned(emoji: &Emoji, tone: SkinTone) -> String {
    if emoji.skin_tones { with_skin_tone(emoji.emoji, tone) } else { emoji.emoji.to_owned() }
}

/// Emoji whose name has every word of the query at a word start: "red
/// heart" narrows rather than widens, and "car" finds "racing car" but not
/// "scarf". Names that start with the first word come first.
pub fn search(query: &str, limit: usize) -> Vec<&'static Emoji> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<(&'static Emoji, bool)> = Vec::new();
    for emoji in groups().iter().flat_map(|g| g.emojis.iter()) {
        let name = emoji.name.to_lowercase();
        if !words.iter().all(|w| word_starts_with(&name, w)) {
            continue;
        }
        matches.push((emoji, name.starts_with(&words[0])));
        if matches.len() >= limit * 2 {
            break;
        }
    }
    // Stable, so the dataset's order holds within each half.
    matches.sort_by_key(|(_, leading)| !leading);
    matches.into_iter().take(limit).map(|(e, _)| e).collect()
}

fn word_starts_with(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(at, _)| at == 0 || haystack[..at].ends_with([' ', '-', '_', '&']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_tones_attach_to_the_base_character() {
        assert_eq!(with_skin_tone("✋", 0), "✋");
        assert_eq!(with_skin_tone("✋", 3), "✋🏽");
        assert_eq!(with_skin_tone("👨‍💻", 5), "👨🏿‍💻");
        assert_eq!(with_skin_tone("☝️", 1), "☝🏻");
    }

    #[test]
    fn search_matches_word_starts_and_narrows() {
        let names: Vec<_> = search("cat", 90).iter().map(|e| e.name).collect();
        assert!(names.contains(&"cat") && names.contains(&"cat face"));
        assert!(!search("car", 90).iter().any(|e| e.name == "scarf"));
        let one = search("face", 500);
        let two = search("face heart", 500);
        assert!(two.len() < one.len());
        assert!(two.iter().all(|e| e.name.contains("face") && e.name.contains("heart")));
        assert!(search("   ", 90).is_empty());
    }

    #[test]
    fn names_and_groups_come_from_the_data() {
        assert_eq!(name_of("😀"), Some("grinning face"));
        assert_eq!(name_of("not an emoji"), None);
        assert_eq!(name_of("#️⃣"), Some("keycap #"));
        assert_eq!(groups().len(), 9);
        assert_eq!(groups()[0].name, "smileys & emotion");
        assert!(groups().iter().all(|g| !g.icon.ends_with("ellipsis.svg")));
    }
}
