//! What a message shows: formatted text as styled runs, and attachments
//! (pictures inline, sized from their metadata before they load so the
//! timeline doesn't jump; video, audio and files as cards that open or save
//! them).

use std::path::PathBuf;

use gpui::AppContext as _;

use gpui::{
    AnyElement, App, ElementId, Entity, FontStyle, FontWeight, Hsla, InteractiveElement, InteractiveText, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, StrikethroughStyle, StyledImage, Styled, StyledText, TextRun, UnderlineStyle, div, font, img, prelude::FluentBuilder,
    px,
};
use gpui_component::{h_flex, v_flex};
use horo_core::{dto::MediaInfoDto, matrix::media as core_media};

use crate::{
    backend::Backend,
    media::{Media, fit},
    rich::{Block, Inline, Marks},
    store::{Lightbox, Store},
    theme::Palette,
};

/// The largest a picture is drawn in the timeline.
const PICTURE_MAX: (f32, f32) = (400., 320.);
const STICKER_MAX: (f32, f32) = (176., 176.);

/// Formatted text, block by block.
pub fn rich(id: &str, blocks: &[Block], colour: Hsla, palette: &Palette, cx: &mut App) -> AnyElement {
    // A message that's only emotes draws them big, as one emoji alone is.
    let jumbo = !blocks.is_empty() && blocks.iter().all(|b| matches!(b, Block::Paragraph(i) if i.only_images()));
    let size = if jumbo { EMOTE_JUMBO } else { EMOTE };
    rich_sized(id, blocks, colour, size, palette, cx)
}

fn rich_sized(id: &str, blocks: &[Block], colour: Hsla, emote: f32, palette: &Palette, cx: &mut App) -> AnyElement {
    let mut column = v_flex().w_full().gap_1();
    for (i, b) in blocks.iter().enumerate() {
        column = column.child(block(&format!("{id}/{i}"), b, colour, emote, palette, cx));
    }
    column.into_any_element()
}

/// Emotes inline: 1.45 times the text, as the WebView build draws them.
const EMOTE: f32 = 21.;
const EMOTE_JUMBO: f32 = 48.;

fn block(id: &str, block: &Block, colour: Hsla, emote: f32, palette: &Palette, cx: &mut App) -> AnyElement {
    match block {
        Block::Paragraph(inline) => text(id, inline, colour, emote, palette, cx),
        Block::Heading(level, inline) => {
            let size = match level {
                1 => 22.,
                2 => 19.,
                3 => 17.,
                _ => 15.,
            };
            div().pt_1().text_size(px(size)).child(text(id, inline, colour, emote, palette, cx)).into_any_element()
        }
        Block::Quote(inner) => div()
            .pl_3()
            .border_l_2()
            .border_color(palette.border_strong)
            .child(rich_sized(id, inner, palette.text_secondary, emote, palette, cx))
            .into_any_element(),
        Block::Code(code) => div()
            .w_full()
            .p_2()
            .rounded(palette.radius_md)
            .bg(palette.inset)
            .font_family(palette.mono_font.clone())
            .text_sm()
            .text_color(colour)
            .child(code.clone())
            .into_any_element(),
        Block::List { ordered, start, items } => {
            let mut list = v_flex().gap(px(2.));
            for (n, item) in items.iter().enumerate() {
                let marker = if *ordered { format!("{}.", start + n as u64) } else { "•".into() };
                list = list.child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().flex_none().min_w(px(14.)).text_color(palette.text_tertiary).child(marker))
                        .child(div().flex_1().min_w_0().child(rich_sized(&format!("{id}/{n}"), item, colour, emote, palette, cx))),
                );
            }
            list.into_any_element()
        }
        Block::Rule => div().w_full().my_1().h(px(1.)).bg(palette.border).into_any_element(),
    }
}

/// One paragraph. With pictures in it, a wrapping row of words and
/// pictures (gpui's text can't hold an image), each word keeping its style
/// and link.
fn text(id: &str, inline: &Inline, colour: Hsla, emote: f32, palette: &Palette, cx: &mut App) -> AnyElement {
    if inline.images.is_empty() {
        return words(id, inline, colour, palette);
    }
    let mut row = h_flex().w_full().flex_wrap().items_center();
    let mut from = 0;
    let mut piece = 0;
    let add_words = |row: gpui::Div, range: std::ops::Range<usize>, piece: &mut usize| -> gpui::Div {
        let mut row = row;
        for token in tokens(&inline.text[range.clone()]) {
            let start = range.start + token.start;
            let slice = inline.slice(start..range.start + token.end);
            row = row.child(div().flex_none().child(words(&format!("{id}/w{piece}"), &slice, colour, palette)));
            *piece += 1;
        }
        row
    };
    for image in &inline.images {
        row = add_words(row, from..image.at, &mut piece);
        from = image.at;
        row = row.child(match Media::image(&image.mxc, Some((64, 64)), cx) {
            Some(picture) => img(picture).flex_none().size(px(emote)).mx(px(1.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
            // Its shortcode while it loads, or if it can't be fetched.
            None => div().flex_none().text_color(palette.text_tertiary).child(image.alt.clone()).into_any_element(),
        });
    }
    row = add_words(row, from..inline.text.len(), &mut piece);
    row.into_any_element()
}

/// Each word with the whitespace after it, so a row of them wraps where
/// text would.
fn tokens(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (i, c) in text.char_indices() {
        let space = c.is_whitespace();
        if in_space && !space {
            out.push(start..i);
            start = i;
        }
        in_space = space;
    }
    if start < text.len() {
        out.push(start..text.len());
    }
    out
}

/// Styled runs, with links that open when clicked.
fn words(id: &str, inline: &Inline, colour: Hsla, palette: &Palette) -> AnyElement {
    let runs = runs(inline, colour, palette);
    let styled = StyledText::new(inline.text.clone()).with_runs(runs);
    if inline.links.is_empty() {
        return div().child(styled).into_any_element();
    }
    let ranges = inline.links.iter().map(|(r, _)| r.clone()).collect();
    let targets: Vec<String> = inline.links.iter().map(|(_, href)| href.clone()).collect();
    div()
        .child(InteractiveText::new(ElementId::Name(SharedString::from(id.to_owned())), styled).on_click(ranges, move |ix, _, cx| {
            if let Some(href) = targets.get(ix)
                && crate::rich::mentioned_user(href).is_none()
                && (href.starts_with("https://") || href.starts_with("http://") || href.starts_with("mailto:"))
            {
                cx.open_url(href);
            }
        }))
        .into_any_element()
}

/// Runs covering all of `inline.text`, one per change of style.
fn runs(inline: &Inline, colour: Hsla, palette: &Palette) -> Vec<TextRun> {
    let plain = |len: usize, marks: Marks| {
        let mut f = font(if marks.code { palette.mono_font.clone() } else { palette.body_font.clone() });
        if marks.bold || marks.mention {
            f.weight = FontWeight::SEMIBOLD;
        }
        if marks.italic {
            f.style = FontStyle::Italic;
        }
        let ink = if marks.mention {
            palette.accent_secondary
        } else if marks.link {
            palette.accent
        } else {
            colour
        };
        TextRun {
            len,
            font: f,
            // A spoiler is a bar of its own colour until it can be
            // revealed on hover.
            color: ink,
            background_color: if marks.spoiler {
                Some(ink)
            } else if marks.code {
                Some(palette.inset)
            } else if marks.mention {
                Some(Hsla { a: 0.16, ..palette.accent_secondary })
            } else {
                None
            },
            underline: (marks.underline || marks.link).then(|| UnderlineStyle { thickness: px(1.), color: Some(ink), wavy: false }),
            strikethrough: marks.strike.then(|| StrikethroughStyle { thickness: px(1.), color: Some(ink) }),
        }
    };
    let mut out = Vec::new();
    let mut at = 0;
    for (range, marks) in &inline.spans {
        if range.start > at {
            out.push(plain(range.start - at, Marks::default()));
        }
        out.push(plain(range.end - range.start, *marks));
        at = range.end;
    }
    if inline.text.len() > at {
        out.push(plain(inline.text.len() - at, Marks::default()));
    }
    out
}

/// A picture attachment (or sticker): the thumbnail at its own shape, the
/// full picture in the lightbox when clicked.
pub fn picture(id: &str, body: &str, media: &MediaInfoDto, sticker: bool, store: &Entity<Store>, palette: &Palette, cx: &mut App) -> AnyElement {
    let (w, h) = fit(media.width, media.height, if sticker { STICKER_MAX } else { PICTURE_MAX }, 48.);
    let source = media.thumbnail_mxc.clone().or_else(|| media.mxc.clone());
    // Twice the drawn size, for Retina screens.
    let size = Some(((w * 2.) as u32, (h * 2.) as u32));
    let image = source.as_deref().and_then(|mxc| Media::image(mxc, size, cx));
    let frame = div()
        .id(ElementId::Name(SharedString::from(format!("{id}/picture"))))
        .w(px(w))
        .h(px(h))
        .rounded(px(12.))
        .overflow_hidden()
        .when(!sticker, |d| d.bg(palette.inset))
        .cursor_pointer();
    let frame = match image {
        Some(image) => frame.child(img(image).size_full().object_fit(gpui::ObjectFit::Contain)),
        None => frame.flex().items_center().justify_center().child(div().text_xs().text_color(palette.text_tertiary).child("loading…")),
    };
    let lightbox = Lightbox { mxc: media.mxc.clone(), thumbnail: source, width: media.width, height: media.height, name: body.to_owned() };
    let store = store.clone();
    frame
        .on_click(move |_, _, cx| {
            let lightbox = lightbox.clone();
            store.update(cx, |s, cx| s.open_lightbox(lightbox, cx))
        })
        .into_any_element()
}

/// What a card's click does.
#[derive(Clone)]
pub enum Open {
    /// Play it in the system's player (video, audio).
    Play,
    /// Ask where, then save it.
    Save,
}

/// Video, audio or a file: a card with its name and size. Video shows its
/// poster frame when it has one.
pub fn card(id: &str, glyph: &str, body: &str, media: &MediaInfoDto, open: Open, palette: &Palette, cx: &mut App) -> AnyElement {
    let poster = media
        .thumbnail_mxc
        .as_deref()
        .and_then(|mxc| {
            let (w, h) = fit(media.width, media.height, (320., 180.), 48.);
            Media::image(mxc, Some(((w * 2.) as u32, (h * 2.) as u32)), cx).map(|image| (image, w, h))
        });
    let details = [media.duration_ms.map(duration), media.size.map(bytes)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let action = match open {
        Open::Play => "play",
        Open::Save => "save",
    };
    let (mxc, name, mime) = (media.mxc.clone(), body.to_owned(), media.mimetype.clone());
    v_flex()
        .id(ElementId::Name(SharedString::from(format!("{id}/card"))))
        .max_w(px(360.))
        .rounded(px(12.))
        .overflow_hidden()
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border_subtle)
        .cursor_pointer()
        .hover(|d| d.border_color(palette.border_strong))
        .children(poster.map(|(image, w, h)| {
            div().relative().w(px(w)).h(px(h)).child(img(image).size_full().object_fit(gpui::ObjectFit::Cover)).child(
                div().absolute().inset_0().flex().items_center().justify_center().child(
                    div().size(px(44.)).rounded_full().bg(Hsla { a: 0.7, ..palette.app }).flex().items_center().justify_center().child("▶"),
                ),
            )
        }))
        .child(
            h_flex()
                .gap_3()
                .px_3()
                .py_2()
                .child(div().flex_none().text_size(px(20.)).child(glyph.to_owned()))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_sm().text_color(palette.text).child(body.to_owned()))
                        .when(!details.is_empty(), |c| c.child(div().text_xs().text_color(palette.text_tertiary).child(details))),
                )
                .child(div().flex_none().text_xs().text_color(palette.accent).child(action)),
        )
        .on_click(move |_, _, cx| {
            let Some(mxc) = mxc.clone() else { return };
            match &open {
                Open::Play => play(mxc, name.clone(), mime.clone(), cx),
                Open::Save => save(mxc, name.clone(), cx),
            }
        })
        .into_any_element()
}

/// Fetch it (decrypted, for an encrypted room) into a temporary file.
pub fn download_temp(mxc: String, name: &str, mime: Option<&str>, cx: &mut App) -> gpui::Task<Result<PathBuf, String>> {
    let dir = std::env::temp_dir().join("horo-media");
    let path = dir.join(safe_name(name, mime));
    let backend = Backend::global(cx);
    let task = backend.run({
        let (state, path) = (backend.state.clone(), path.clone());
        async move {
            std::fs::create_dir_all(&dir).map_err(|e| horo_core::Error::Other(e.to_string()))?;
            let core = state.core().await?;
            core_media::save_to(&core.client, &mxc, &path.to_string_lossy()).await
        }
    });
    cx.background_spawn(async move {
        match task.await {
            Ok(Ok(())) => Ok(path),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    })
}

/// Fetch it into a temporary file and hand it to the system's player.
pub fn play_externally(mxc: String, name: String, mime: Option<String>, cx: &mut App) {
    play(mxc, name, mime, cx)
}

fn play(mxc: String, name: String, mime: Option<String>, cx: &mut App) {
    let download = download_temp(mxc, &name, mime.as_deref(), cx);
    cx.spawn(async move |cx| match download.await {
        Ok(path) => cx.update(|cx| cx.open_with_system(&path)),
        Err(e) => tracing::warn!("couldn't open {name}: {e}"),
    })
    .detach();
}

/// Ask where to put it, then download it there and show it.
fn save(mxc: String, name: String, cx: &mut App) {
    let downloads = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(std::env::temp_dir);
    let chosen = cx.prompt_for_new_path(&downloads, Some(&safe_name(&name, None)));
    let state = Backend::global(cx).state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = chosen.await else { return };
        let task = cx.update(|cx| {
            Backend::global(cx).run({
                let (state, path, mxc) = (state.clone(), path.clone(), mxc.clone());
                async move { core_media::save_to(&state.core().await?.client, &mxc, &path.to_string_lossy()).await }
            })
        });
        match task.await {
            Ok(Ok(())) => {
                cx.update(|cx| cx.reveal_path(&path));
            }
            Ok(Err(e)) => tracing::warn!("couldn't save {name}: {e}"),
            Err(e) => tracing::warn!("couldn't save {name}: {e}"),
        }
    })
    .detach();
}

/// A file name that can't climb out of its directory, with an extension
/// the player will recognise.
fn safe_name(name: &str, mime: Option<&str>) -> String {
    let base: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '_' } else { c }).collect();
    let base = base.trim_start_matches('.').trim();
    let base = if base.is_empty() { "attachment" } else { base };
    if base.contains('.') {
        return base.to_owned();
    }
    let extension = match mime {
        Some("video/mp4") => ".mp4",
        Some("video/webm") => ".webm",
        Some("video/quicktime") => ".mov",
        Some("audio/ogg") => ".ogg",
        Some("audio/mpeg") => ".mp3",
        Some("audio/mp4") => ".m4a",
        _ => "",
    };
    format!("{base}{extension}")
}

pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_keep_the_space_after_them() {
        let text = "hi  there friend";
        let parts: Vec<&str> = tokens(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(parts, ["hi  ", "there ", "friend"]);
        assert!(tokens("").is_empty());
        assert_eq!(tokens(" lead").into_iter().map(|r| &" lead"[r]).collect::<Vec<_>>(), [" ", "lead"]);
    }

    #[test]
    fn saved_names_stay_in_their_directory() {
        assert_eq!(safe_name("../../etc/passwd", None), "_.._etc_passwd");
        assert_eq!(safe_name(".hidden", None), "hidden");
        assert_eq!(safe_name("clip", Some("video/mp4")), "clip.mp4");
        assert_eq!(safe_name("", None), "attachment");
    }

    #[test]
    fn sizes_and_durations_read_naturally() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2 KB");
        assert_eq!(bytes(5 * 1024 * 1024 + 300_000), "5.3 MB");
        assert_eq!(duration(65_000), "1:05");
    }
}
