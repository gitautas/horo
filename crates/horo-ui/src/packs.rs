//! Custom emotes and stickers, as src/lib/packs.ts turns whole packs into
//! what the UI asks for: what `:blobcat:` looks like, what can be typed,
//! what the picker shows, and what a reaction key draws as.
//!
//! Packs come from `horo_core::matrix::packs::available`, your own first,
//! so when two packs claim one shortcode the first keeps it: yours wins.

use std::collections::HashMap;

use horo_core::{dto::EmoteRef, matrix::packs::{ImagePackDto, PackImageDto}};

/// What a pick from the emoji picker is.
#[derive(Debug, Clone)]
pub enum Picked {
    Unicode(String),
    Emote(PackImageDto),
    Sticker(PackImageDto),
}

/// Shortcode → image, for everything usable as an inline emote.
pub fn emote_lookup(packs: &[ImagePackDto]) -> HashMap<String, PackImageDto> {
    lookup(packs, |i| i.is_emoticon)
}

/// Shortcode → image across every image, for what a shortcode someone sent
/// stands for (a sticker-only pack still has names).
pub fn image_lookup(packs: &[ImagePackDto]) -> HashMap<String, PackImageDto> {
    lookup(packs, |_| true)
}

fn lookup(packs: &[ImagePackDto], keep: impl Fn(&PackImageDto) -> bool) -> HashMap<String, PackImageDto> {
    let mut out = HashMap::new();
    for image in packs.iter().flat_map(|p| p.images.iter()).filter(|i| keep(i)) {
        out.entry(image.shortcode.clone()).or_insert_with(|| image.clone());
    }
    out
}

/// The emotes to hand the backend when sending, so it can put them in.
pub fn emote_refs(packs: &[ImagePackDto]) -> Vec<EmoteRef> {
    let mut refs: Vec<EmoteRef> = emote_lookup(packs).into_iter().map(|(shortcode, image)| EmoteRef { shortcode, url: image.url }).collect();
    refs.sort_by(|a, b| a.shortcode.cmp(&b.shortcode));
    refs
}

/// Every sticker on offer, in pack order.
pub fn stickers_of(packs: &[ImagePackDto]) -> Vec<PackImageDto> {
    packs.iter().flat_map(|p| p.images.iter()).filter(|i| i.is_sticker).cloned().collect()
}

/// A reaction key that is exactly one `:shortcode:`.
pub fn reaction_shortcode(key: &str) -> Option<&str> {
    let code = key.trim().strip_prefix(':')?.strip_suffix(':')?;
    (!code.is_empty() && !code.contains([':', ' ', '\t', '\n'])).then_some(code)
}

/// The key a pick reacts with: the character, or a custom emote's own
/// address (what the clients this account's friends use send; reactions
/// pile up on exact string equality).
pub fn reaction_key(picked: &Picked) -> String {
    match picked {
        Picked::Unicode(emoji) => emoji.clone(),
        Picked::Emote(image) | Picked::Sticker(image) => image.url.clone(),
    }
}

/// The picture a reaction key stands for, and what to call it: a
/// `:shortcode:` we have the pack for, or an `mxc://` address (named from
/// the pack when we have that image).
pub fn reaction_image(key: &str, lookup: &HashMap<String, PackImageDto>) -> Option<(String, String)> {
    let key = key.trim();
    if let Some(code) = reaction_shortcode(key) {
        return lookup.get(code).map(|i| (i.url.clone(), format!(":{code}:")));
    }
    if !key.starts_with("mxc://") || key.contains(char::is_whitespace) {
        return None;
    }
    let name = lookup.iter().find(|(_, i)| i.url == key).map(|(code, _)| format!(":{code}:")).unwrap_or_else(|| key.to_owned());
    Some((key.to_owned(), name))
}

/// Emotes whose shortcode contains the query, those starting with it
/// first, then alphabetical.
pub fn match_emotes(lookup: &HashMap<String, PackImageDto>, query: &str, limit: usize) -> Vec<PackImageDto> {
    let needle = query.to_lowercase();
    let mut found: Vec<(&String, &PackImageDto)> = lookup.iter().filter(|(code, _)| code.to_lowercase().contains(&needle)).collect();
    found.sort_by(|(a, _), (b, _)| {
        let lead = |s: &str| !s.to_lowercase().starts_with(&needle);
        lead(a).cmp(&lead(b)).then_with(|| a.cmp(b))
    });
    found.into_iter().take(limit).map(|(_, i)| i.clone()).collect()
}

/// The shortcode being typed at `caret` (a byte offset), for the
/// composer's suggestions: the partial code and where its colon is. Only
/// a colon that starts a word counts, so `https://` and `3:15` don't.
pub fn typing_shortcode(text: &str, caret: usize) -> Option<(String, usize)> {
    let before = text.get(..caret)?;
    let colon = before.rfind(':')?;
    let query = &before[colon + 1..];
    if query.is_empty() || query.contains(|c: char| c.is_whitespace() || c == ':') {
        return None;
    }
    if before[..colon].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    Some((query.to_owned(), colon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use horo_core::matrix::packs::PackSource;

    fn image(code: &str, emote: bool, sticker: bool) -> PackImageDto {
        PackImageDto {
            shortcode: code.into(),
            url: format!("mxc://x/{code}"),
            body: code.into(),
            is_emoticon: emote,
            is_sticker: sticker,
            width: None,
            height: None,
            size: None,
            mimetype: None,
        }
    }

    fn pack(id: &str, images: Vec<PackImageDto>) -> ImagePackDto {
        ImagePackDto {
            id: id.into(),
            source: PackSource::User,
            room_id: None,
            state_key: None,
            display_name: id.into(),
            avatar_url: None,
            attribution: None,
            images,
            everywhere: true,
            can_edit: false,
        }
    }

    #[test]
    fn the_first_pack_keeps_a_shared_shortcode_and_stickers_only_stay_out_of_emotes() {
        let mut mine = image("blob", true, false);
        mine.url = "mxc://mine/blob".into();
        let packs = [pack("user", vec![mine, image("big", false, true)]), pack("room", vec![image("blob", true, true)])];
        let emotes = emote_lookup(&packs);
        assert_eq!(emotes["blob"].url, "mxc://mine/blob");
        assert!(!emotes.contains_key("big"));
        assert!(image_lookup(&packs).contains_key("big"));
        assert_eq!(stickers_of(&packs).iter().map(|i| i.shortcode.as_str()).collect::<Vec<_>>(), ["big", "blob"]);
        assert_eq!(emote_refs(&packs).len(), 1);
    }

    #[test]
    fn reaction_keys_round_trip_and_both_spellings_draw() {
        let packs = [pack("user", vec![image("cat", true, false)])];
        let lookup = image_lookup(&packs);
        let key = reaction_key(&Picked::Emote(lookup["cat"].clone()));
        assert_eq!(key, "mxc://x/cat");
        assert_eq!(reaction_image(&key, &lookup), Some(("mxc://x/cat".into(), ":cat:".into())));
        assert_eq!(reaction_image(":cat:", &lookup), Some(("mxc://x/cat".into(), ":cat:".into())));
        assert_eq!(reaction_image(":dog:", &lookup), None);
        assert_eq!(reaction_image("mxc://y/z", &lookup), Some(("mxc://y/z".into(), "mxc://y/z".into())));
        assert_eq!(reaction_image("👍", &lookup), None);
        assert_eq!(reaction_key(&Picked::Unicode("👍".into())), "👍");
        assert_eq!(reaction_shortcode("a :cat: b"), None);
    }

    #[test]
    fn emote_search_puts_leading_matches_first() {
        let packs = [pack("user", vec![image("happycat", true, false), image("cat", true, false), image("catjam", true, false), image("dog", true, false)])];
        let found: Vec<_> = match_emotes(&emote_lookup(&packs), "cat", 8).into_iter().map(|i| i.shortcode).collect();
        assert_eq!(found, ["cat", "catjam", "happycat"]);
    }

    #[test]
    fn a_shortcode_is_caught_only_at_the_start_of_a_word() {
        let at = |text: &str| typing_shortcode(text, text.len());
        assert_eq!(at(":blob"), Some(("blob".into(), 0)));
        assert_eq!(at("hello :blob"), Some(("blob".into(), 6)));
        assert_eq!(at(":"), None);
        assert_eq!(at("https://example"), None);
        assert_eq!(at("3:15"), None);
        assert_eq!(at(":blobcat: "), None);
        assert_eq!(at(":blob cat"), None);
        assert_eq!(typing_shortcode(":blob and more", 5), Some(("blob".into(), 0)));
        assert_eq!(at("ąčę :žu"), Some(("žu".into(), 7)));
    }
}
