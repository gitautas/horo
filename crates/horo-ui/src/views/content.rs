//! What a message shows: formatted text as styled runs, and attachments
//! (pictures inline, sized from their metadata before they load so the
//! timeline doesn't jump; video, audio and files as cards that open or save
//! them).

use std::{path::PathBuf, rc::Rc};

use gpui::AppContext as _;

use gpui::{
    AnyElement, App, ElementId, Entity, FontStyle, FontWeight, Hsla, InteractiveElement, InteractiveText, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, StrikethroughStyle, StyledImage, Styled, StyledText, TextRun, UnderlineStyle, div, font, img, prelude::FluentBuilder,
};
use gpui_component::{h_flex, v_flex};
use horo_core::{dto::MediaInfoDto, matrix::media as core_media};

use crate::theme::zpx;
use crate::{
    backend::Backend,
    media::{Media, fit},
    rich::{Block, Inline, Marks},
    store::Store,
    theme::Palette,
};

/// The largest a picture is drawn in the timeline.
const PICTURE_MAX: (f32, f32) = (400., 320.);
const STICKER_MAX: (f32, f32) = (176., 176.);

/// Interaction hooks for formatted text: clicking a `@mention` opens their
/// profile card, and clicking a spoiler reveals it.
#[derive(Clone)]
pub struct RichContext {
    pub store: Entity<Store>,
    pub spoilers_revealed: bool,
    pub on_reveal_spoiler: Rc<dyn Fn(&mut App)>,
}

/// Formatted text, block by block.
pub fn rich(id: &str, blocks: &[Block], colour: Hsla, rich_ctx: &RichContext, palette: &Palette, cx: &mut App) -> AnyElement {
    // A message that's only emotes draws them big, as one emoji alone is.
    let jumbo = !blocks.is_empty() && blocks.iter().all(|b| matches!(b, Block::Paragraph(i) if i.only_images()));
    let size = if jumbo { EMOTE_JUMBO } else { EMOTE };
    rich_sized(id, blocks, colour, size, rich_ctx, palette, cx)
}

/// Plain text with bare URLs turned into clickable links.
pub fn linked_plain(id: &str, body: &str, colour: Hsla, rich_ctx: &RichContext, palette: &Palette) -> AnyElement {
    let inline = crate::rich::linkify(body);
    words(id, &inline, colour, rich_ctx, palette)
}

fn rich_sized(id: &str, blocks: &[Block], colour: Hsla, emote: f32, rich_ctx: &RichContext, palette: &Palette, cx: &mut App) -> AnyElement {
    let mut column = v_flex().w_full().gap_1();
    for (i, b) in blocks.iter().enumerate() {
        column = column.child(block(&format!("{id}/{i}"), b, colour, emote, rich_ctx, palette, cx));
    }
    column.into_any_element()
}

/// Emotes inline: 1.45 times the text, as the WebView build draws them.
const EMOTE: f32 = 21.;
const EMOTE_JUMBO: f32 = 48.;

fn block(id: &str, block: &Block, colour: Hsla, emote: f32, rich_ctx: &RichContext, palette: &Palette, cx: &mut App) -> AnyElement {
    match block {
        Block::Paragraph(inline) => text(id, inline, colour, emote, rich_ctx, palette, cx),
        Block::Heading(level, inline) => {
            let size = match level {
                1 => 22.,
                2 => 19.,
                3 => 17.,
                _ => 15.,
            };
            div().pt_1().text_size(zpx(size)).child(text(id, inline, colour, emote, rich_ctx, palette, cx)).into_any_element()
        }
        Block::Quote(inner) => div()
            .pl_3()
            .border_l_2()
            .border_color(palette.border_strong)
            .child(rich_sized(id, inner, palette.text_secondary, emote, rich_ctx, palette, cx))
            .into_any_element(),
        Block::Code(code) => {
            let copy_code = code.clone();
            div()
                .relative()
                .w_full()
                .p_2()
                .pr(zpx(56.))
                .rounded(palette.radius_md)
                .bg(palette.inset)
                .font_family(palette.mono_font.clone())
                .text_sm()
                .text_color(colour)
                .child(plain(format!("select/{id}"), code.clone(), palette))
                .child(
                    div()
                        .id(SharedString::from(format!("copy-code-{id}")))
                        .absolute()
                        .top(zpx(6.))
                        .right(zpx(6.))
                        .px(zpx(7.))
                        .py(zpx(2.))
                        .rounded(zpx(6.))
                        .bg(palette.card)
                        .border_1()
                        .border_color(palette.border)
                        .font_family(palette.body_font.clone())
                        .text_xs()
                        .text_color(palette.text_secondary)
                        .opacity(0.75)
                        .cursor_pointer()
                        .hover(|d| d.opacity(1.0).bg(palette.raised).text_color(palette.text))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_code.clone()));
                        })
                        .child("copy"),
                )
                .into_any_element()
        }
        Block::List { ordered, start, items } => {
            let mut list = v_flex().gap(zpx(2.));
            for (n, item) in items.iter().enumerate() {
                let marker = if *ordered { format!("{}.", start + n as u64) } else { "•".into() };
                list = list.child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().flex_none().min_w(zpx(14.)).text_color(palette.text_tertiary).child(marker))
                        .child(div().flex_1().min_w_0().child(rich_sized(&format!("{id}/{n}"), item, colour, emote, rich_ctx, palette, cx))),
                );
            }
            list.into_any_element()
        }
        Block::Rule => div().w_full().my_1().h(zpx(1.)).bg(palette.border).into_any_element(),
    }
}

/// One paragraph. With pictures in it, a wrapping row of words and
/// pictures (gpui's text can't hold an image), each word keeping its style
/// and link.
fn text(id: &str, inline: &Inline, colour: Hsla, emote: f32, rich_ctx: &RichContext, palette: &Palette, cx: &mut App) -> AnyElement {
    if inline.images.is_empty() {
        return words(id, inline, colour, rich_ctx, palette);
    }
    let mut row = h_flex().w_full().flex_wrap().items_center();
    let mut from = 0;
    let mut piece = 0;
    let add_words = |row: gpui::Div, range: std::ops::Range<usize>, piece: &mut usize| -> gpui::Div {
        let mut row = row;
        for token in tokens(&inline.text[range.clone()]) {
            let start = range.start + token.start;
            let slice = inline.slice(start..range.start + token.end);
            row = row.child(div().flex_none().child(words(&format!("{id}/w{piece}"), &slice, colour, rich_ctx, palette)));
            *piece += 1;
        }
        row
    };
    for image in &inline.images {
        row = add_words(row, from..image.at, &mut piece);
        from = image.at;
        row = row.child(match Media::image(&image.mxc, Some((64, 64)), cx) {
            Some(picture) => img(picture).flex_none().size(zpx(emote)).mx(zpx(1.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
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

enum ClickTarget {
    Url(String),
    Mention(String),
    Spoiler,
}

/// Styled runs, selectable, with links, mentions and spoilers clickable.
fn words(id: &str, inline: &Inline, colour: Hsla, rich_ctx: &RichContext, palette: &Palette) -> AnyElement {
    let runs = runs(inline, colour, rich_ctx.spoilers_revealed, palette);
    let styled = StyledText::new(inline.text.clone()).with_runs(runs);
    let layout = styled.layout().clone();
    let select = |inner: AnyElement| {
        super::selectable::selectable(ElementId::Name(SharedString::from(format!("select/{id}"))), inline.text.clone(), layout.clone(), inner, selection(palette))
    };

    let mut ranges = Vec::new();
    let mut targets = Vec::new();
    // Unrevealed spoilers take precedence over links inside them so clicking a
    // hidden spoiler reveals it rather than opening an unseen URL.
    if !rich_ctx.spoilers_revealed {
        for (range, marks) in &inline.spans {
            if marks.spoiler {
                ranges.push(range.clone());
                targets.push(ClickTarget::Spoiler);
            }
        }
    }
    for (range, href) in &inline.links {
        if let Some(user_id) = crate::rich::mentioned_user_id(href) {
            ranges.push(range.clone());
            targets.push(ClickTarget::Mention(user_id));
        } else if href.starts_with("https://") || href.starts_with("http://") || href.starts_with("mailto:") {
            ranges.push(range.clone());
            targets.push(ClickTarget::Url(href.clone()));
        }
    }
    if ranges.is_empty() {
        return div().child(select(styled.into_any_element())).into_any_element();
    }
    let store = rich_ctx.store.clone();
    let on_reveal = rich_ctx.on_reveal_spoiler.clone();
    let linked = InteractiveText::new(ElementId::Name(SharedString::from(id.to_owned())), styled).on_click(ranges, move |ix, window, cx| {
        match targets.get(ix) {
            Some(ClickTarget::Url(href)) => cx.open_url(href),
            Some(ClickTarget::Mention(user_id)) => {
                let at = window.mouse_position();
                let user_id = user_id.clone();
                store.update(cx, |s, cx| s.open_profile(user_id, at, None, cx));
            }
            Some(ClickTarget::Spoiler) => on_reveal(cx),
            None => {}
        }
    });
    div().child(select(linked.into_any_element())).into_any_element()
}

/// Selected text's background.
pub fn selection(palette: &Palette) -> Hsla {
    Hsla { a: 0.3, ..palette.accent }
}

/// Plain text, selectable.
pub fn plain(id: impl Into<SharedString>, text: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    let text: SharedString = text.into();
    let styled = StyledText::new(text.clone());
    let layout = styled.layout().clone();
    super::selectable::selectable(ElementId::Name(id.into()), text, layout, styled, selection(palette)).into_any_element()
}

/// Runs covering all of `inline.text`, one per change of style.
fn runs(inline: &Inline, colour: Hsla, spoilers_revealed: bool, palette: &Palette) -> Vec<TextRun> {
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
            // An unrevealed spoiler is a bar of its own colour; clicking reveals it.
            color: ink,
            background_color: if marks.spoiler {
                Some(if spoilers_revealed { Hsla { a: 0.14, ..palette.text_tertiary } } else { ink })
            } else if marks.code {
                Some(palette.inset)
            } else if marks.mention {
                Some(Hsla { a: 0.16, ..palette.accent_secondary })
            } else {
                None
            },
            underline: (marks.underline || marks.link).then(|| UnderlineStyle { thickness: zpx(1.), color: Some(ink), wavy: false }),
            strikethrough: marks.strike.then(|| StrikethroughStyle { thickness: zpx(1.), color: Some(ink) }),
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
/// full picture in the lightbox when clicked. Transparent cut-outs render
/// frameless, the way stickers do.
pub fn picture(
    id: &str,
    media: &MediaInfoDto,
    sticker: bool,
    palette: &Palette,
    cx: &mut App,
    on_open: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let max = if sticker { STICKER_MAX } else { PICTURE_MAX };
    let (w, h) = fit(media.width, media.height, max, 48.);
    let source = media.thumbnail_mxc.clone().or_else(|| media.mxc.clone());
    // Twice the drawn size, for Retina screens.
    let size = Some(((w * 2.) as u32, (h * 2.) as u32));
    let (image, cutout) = source
        .as_deref()
        .and_then(|mxc| Media::image_with_cutout(mxc, size, cx))
        .map(|(img, c)| (Some(img), c))
        .unwrap_or((None, false));
    // An event without its size (plenty of clients leave it out) is drawn
    // at a placeholder shape while loading, then at the picture's own shape,
    // read from its header, so the frame hugs it instead of letterboxing it.
    let (w, h) = match (&image, media.width.zip(media.height)) {
        (Some(image), None) => imagesize::blob_size(image.bytes())
            .ok()
            .map(|s| fit(Some(s.width as u64), Some(s.height as u64), max, 48.))
            .unwrap_or((w, h)),
        _ => (w, h),
    };
    let frameless = sticker || cutout;
    let frame = div()
        .id(ElementId::Name(SharedString::from(format!("{id}/picture"))))
        .w(zpx(w))
        .h(zpx(h))
        .when(!frameless, |d| d.rounded(zpx(10.)).overflow_hidden())
        // The placeholder only while it loads: once in, the picture fills
        // the frame exactly, as Discord draws it.
        .when(image.is_none() && !frameless, |d| d.bg(palette.inset))
        .cursor_pointer();
    let frame = match image {
        // Rounded itself: gpui clips children to a rectangle, whatever the
        // frame's corners.
        Some(image) => frame.child(img(image).size_full().aspect_ratio(w / h).when(!frameless, |i| i.rounded(zpx(10.))).object_fit(gpui::ObjectFit::Contain)),
        None => frame.flex().items_center().justify_center().child(div().text_xs().text_color(palette.text_tertiary).child("loading…")),
    };
    frame.on_click(move |_, _, cx| on_open(cx)).into_any_element()
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
        .max_w(zpx(360.))
        .rounded(zpx(12.))
        .overflow_hidden()
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border_subtle)
        .cursor_pointer()
        .hover(|d| d.border_color(palette.border_strong))
        .children(poster.map(|(image, w, h)| {
            // The card's top corners, inside its border.
            div().relative().w(zpx(w)).h(zpx(h)).child(img(image).size_full().aspect_ratio(w / h).rounded_t(zpx(11.)).object_fit(gpui::ObjectFit::Cover)).child(
                div().absolute().inset_0().flex().items_center().justify_center().child(
                    div().size(zpx(44.)).rounded_full().bg(Hsla { a: 0.7, ..palette.app }).flex().items_center().justify_center().child("▶"),
                ),
            )
        }))
        .child(
            h_flex()
                .gap_3()
                .px_3()
                .py_2()
                .child(div().flex_none().text_size(zpx(20.)).child(glyph.to_owned()))
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
pub(crate) fn save_media(mxc: String, name: String, store: Option<Entity<Store>>, cx: &mut App) {
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
        let err = match task.await {
            Ok(Ok(())) => {
                cx.update(|cx| cx.reveal_path(&path));
                None
            }
            Ok(Err(e)) => Some(e.to_string()),
            Err(e) => Some(e.to_string()),
        };
        if let Some(e) = err {
            tracing::warn!("couldn't save {name}: {e}");
            if let Some(store) = store {
                cx.update(|cx| store.update(cx, |s, cx| s.show_banner(crate::store::Tone::Error, format!("couldn't save {name}: {e}"), cx)));
            }
        }
    })
    .detach();
}

fn save(mxc: String, name: String, cx: &mut App) {
    save_media(mxc, name, None, cx);
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
