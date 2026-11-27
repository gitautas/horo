//! A picture full-size over the window: the thumbnail at once, the
//! original when it arrives. A click anywhere closes it.
//!
//! The picture is sized here, in pixels, to fit the window less a margin
//! and room for its name, never larger than it is. Leaving it to the
//! layout (a box the size of the window, the picture 100% of it,
//! "contain") didn't work: the box's height never became definite, so the
//! picture fell back to its own proportions at full width and a tall one
//! ran off the bottom of the window.

use gpui::{
    AnyElement, App, Entity, Hsla, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, StyledImage, Window, div, img,
    prelude::FluentBuilder, px,
};
use gpui_component::{h_flex, v_flex};

use crate::{media::Media, store::Store, theme::Palette, views::content};

/// Clear space around the picture.
const MARGIN: f32 = 32.;
/// Extra horizontal clearance when prev/next gallery arrows are shown.
const GALLERY_MARGIN_X: f32 = 80.;
/// Room under it for its name.
const CAPTION: f32 = 40.;

/// `natural` scaled down (never up) to fit in `room`.
pub fn fit_within(natural: (f32, f32), room: (f32, f32)) -> (f32, f32) {
    let (w, h) = natural;
    if w <= 0. || h <= 0. {
        return room;
    }
    let scale = (room.0 / w).min(room.1 / h).min(1.);
    (w * scale, h * scale)
}

pub fn lightbox(store: &Entity<Store>, palette: &Palette, window: &Window, cx: &mut App) -> Option<AnyElement> {
    let gallery = store.read(cx).lightbox.clone()?;
    let shown = gallery.current()?.clone();
    let has_gallery = gallery.items.len() > 1;
    let image = shown
        .mxc
        .as_deref()
        .and_then(|mxc| Media::image(mxc, None, cx))
        .or_else(|| shown.thumbnail.as_deref().and_then(|mxc| Media::image(mxc, Some((800, 800)), cx)));
    let viewport = window.viewport_size();
    let margin_x = if has_gallery { GALLERY_MARGIN_X } else { MARGIN };
    let room = ((f32::from(viewport.width) - 2. * margin_x).max(64.), (f32::from(viewport.height) - 2. * MARGIN - CAPTION).max(64.));

    let picture = match image {
        Some(image) => {
            // The original's size as the event gives it, so a thumbnail
            // standing in is drawn the size the original will be; the
            // file's own header when the event doesn't say.
            let natural = shown
                .width
                .zip(shown.height)
                .map(|(w, h)| (w as f32, h as f32))
                .or_else(|| imagesize::blob_size(image.bytes()).ok().map(|s| (s.width as f32, s.height as f32)))
                .unwrap_or(room);
            let (w, h) = fit_within(natural, room);
            div()
                .id("lightbox-picture")
                .cursor_default()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(img(image).w(px(w)).h(px(h)).object_fit(gpui::ObjectFit::Contain))
                .into_any_element()
        }
        None => div().text_color(palette.text_tertiary).child("loading…").into_any_element(),
    };
    let (close_backdrop, close_btn) = (store.clone(), store.clone());
    let save_target = shown.mxc.clone().or_else(|| shown.thumbnail.clone());
    let save_button = save_target.map(|mxc| {
        let (name, store) = (shown.name.clone(), store.clone());
        div()
            .id("lightbox-save")
            .h(px(34.))
            .px(px(14.))
            .rounded_full()
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .hover(|d| d.bg(palette.card))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .text_color(palette.text_secondary)
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("save this picture").build(window, cx))
            .child("save")
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                content::save_media(mxc.clone(), name.clone(), Some(store.clone()), cx);
            })
    });
    let close_button = div()
        .id("lightbox-close")
        .size(px(34.))
        .rounded_full()
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .hover(|d| d.bg(palette.card))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_sm()
        .text_color(palette.text_secondary)
        .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("close (esc)").build(window, cx))
        .child("✕")
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            close_btn.update(cx, |s, cx| s.close_lightbox(cx));
        });

    let arrow_top = ((f32::from(viewport.height) - 44.) / 2.).max(64.);
    let prev_button = has_gallery.then(|| {
        let enabled = gallery.can_prev();
        let prev_store = store.clone();
        div()
            .id("lightbox-prev")
            .absolute()
            .left(px(18.))
            .top(px(arrow_top))
            .size(px(44.))
            .rounded_full()
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(22.))
            .text_color(if enabled { palette.text } else { palette.text_tertiary })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(palette.card).border_color(palette.accent))
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("previous media (←)").build(window, cx))
            })
            .when(!enabled, |d| d.opacity(0.3).cursor_default())
            .child("‹")
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                if enabled {
                    prev_store.update(cx, |s, cx| s.step_lightbox(-1, cx));
                }
            })
    });
    let next_button = has_gallery.then(|| {
        let enabled = gallery.can_next();
        let next_store = store.clone();
        div()
            .id("lightbox-next")
            .absolute()
            .right(px(18.))
            .top(px(arrow_top))
            .size(px(44.))
            .rounded_full()
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(22.))
            .text_color(if enabled { palette.text } else { palette.text_tertiary })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(palette.card).border_color(palette.accent))
                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("next media (→)").build(window, cx))
            })
            .when(!enabled, |d| d.opacity(0.3).cursor_default())
            .child("›")
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                if enabled {
                    next_store.update(cx, |s, cx| s.step_lightbox(1, cx));
                }
            })
    });

    Some(
        v_flex()
            .id("lightbox")
            .occlude()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .bg(Hsla { a: 0.92, ..palette.app })
            .cursor_pointer()
            .on_click(move |_, _, cx| close_backdrop.update(cx, |s, cx| s.close_lightbox(cx)))
            .child(
                h_flex()
                    .absolute()
                    .top(px(18.))
                    .right(px(18.))
                    .gap(px(8.))
                    .children(save_button)
                    .child(close_button),
            )
            .children(prev_button)
            .children(next_button)
            .child(picture)
            .child(
                h_flex()
                    .id("lightbox-caption")
                    .cursor_default()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .flex_none()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .max_w(px(520.))
                            .truncate()
                            .text_sm()
                            .text_color(palette.text_secondary)
                            .child(shown.name),
                    )
                    .when(has_gallery, |row| {
                        row.child(
                            div()
                                .px(px(8.))
                                .py(px(2.))
                                .rounded_full()
                                .bg(palette.raised)
                                .border_1()
                                .border_color(palette.border_subtle)
                                .text_xs()
                                .text_color(palette.text_tertiary)
                                .child(format!("{} / {}", gallery.index + 1, gallery.items.len())),
                        )
                    }),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_without_growing() {
        // A tall photo in a wide window: as tall as the room, narrow.
        assert_eq!(fit_within((3072., 4096.), (1116., 676.)), (507., 676.));
        // A small one stays its size.
        assert_eq!(fit_within((200., 100.), (1116., 676.)), (200., 100.));
        // A wide one: as wide as the room.
        assert_eq!(fit_within((4000., 1000.), (1000., 700.)), (1000., 250.));
        assert_eq!(fit_within((0., 0.), (10., 20.)), (10., 20.));
    }

    #[test]
    fn gallery_steps_within_bounds() {
        use crate::store::{Lightbox, LightboxItem};
        let item = |name: &str| LightboxItem { mxc: None, thumbnail: None, width: None, height: None, name: name.to_owned() };
        let mut lb = Lightbox { items: vec![item("a"), item("b"), item("c")], index: 1 };
        assert!(lb.can_prev() && lb.can_next());
        assert!(lb.step(-1));
        assert_eq!(lb.index, 0);
        assert!(!lb.can_prev() && !lb.step(-1));
        assert!(lb.step(1));
        assert!(lb.step(1));
        assert_eq!(lb.index, 2);
        assert!(!lb.can_next() && !lb.step(1));
    }
}
