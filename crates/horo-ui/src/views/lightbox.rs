//! A picture full-size over the window: the thumbnail at once, the
//! original when it arrives. A click anywhere closes it.

use gpui::{AnyElement, App, Entity, Hsla, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, StyledImage, div, img, px};
use gpui_component::v_flex;

use crate::{media::Media, store::Store, theme::Palette};

pub fn lightbox(store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let shown = store.read(cx).lightbox.clone()?;
    let image = shown
        .mxc
        .as_deref()
        .and_then(|mxc| Media::image(mxc, None, cx))
        .or_else(|| shown.thumbnail.as_deref().and_then(|mxc| Media::image(mxc, Some((800, 800)), cx)));
    let close = store.clone();
    Some(
        v_flex()
            .id("lightbox")
            .occlude()
            .absolute()
            .inset_0()
            .p_8()
            .gap_3()
            .items_center()
            .justify_center()
            .bg(Hsla { a: 0.92, ..palette.app })
            .cursor_pointer()
            .on_click(move |_, _, cx| close.update(cx, |s, cx| s.close_lightbox(cx)))
            .child(match image {
                Some(image) => div().flex_1().min_h_0().w_full().child(img(image).size_full().object_fit(gpui::ObjectFit::Contain)).into_any_element(),
                None => div().text_color(palette.text_tertiary).child("loading…").into_any_element(),
            })
            .child(div().flex_none().text_sm().text_color(palette.text_secondary).max_w(px(600.)).truncate().child(shown.name))
            .into_any_element(),
    )
}
