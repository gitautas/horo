//! Right-click menus: one look for all of them (spaces, rooms, messages),
//! after the WebView build's ContextMenu.

use gpui::{AnyElement, Div, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Stateful, Styled, div, prelude::FluentBuilder, px};
use gpui_component::{h_flex, v_flex};

use crate::theme::{Palette, corner, space, text};

/// The menu's card; the caller adds its rows and how it closes.
pub fn frame(id: impl Into<ElementId>, palette: &Palette) -> Stateful<Div> {
    v_flex()
        .id(id)
        .occlude()
        .min_w(px(220.))
        .p(space(4.))
        .rounded(corner(14.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .shadow_lg()
        .text_color(palette.text)
}

/// One row. `check` is a toggle or a choice: its tick (or room for one)
/// in a column of its own, so the words line up.
pub fn item(id: impl Into<ElementId>, words: impl Into<gpui::SharedString>, enabled: bool, check: Option<bool>, palette: &Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .px(space(12.))
        .py(space(7.))
        .rounded(corner(10.))
        .text_size(text(13.))
        .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(palette.inset)))
        .when(!enabled, |d| d.text_color(palette.text_tertiary))
        .children(check.map(|on| div().flex_none().w(px(18.)).text_color(palette.accent).child(if on { "✓" } else { "" })))
        .child(words.into())
}

/// A quiet line naming what the menu is for, or a group in it.
pub fn caption(words: impl Into<gpui::SharedString>, palette: &Palette) -> Div {
    div().px(space(12.)).pt(space(6.)).pb(space(4.)).text_size(text(11.)).text_color(palette.text_tertiary).truncate().child(words.into())
}

pub fn separator(palette: &Palette) -> Div {
    div().my(space(4.)).mx(space(8.)).h(px(1.)).bg(palette.border_subtle)
}

/// The menu where the pointer was, kept inside the window, over
/// everything.
pub fn place(at: Point<Pixels>, menu: impl IntoElement) -> AnyElement {
    gpui::deferred(gpui::anchored().position(at).snap_to_window_with_margin(px(8.)).child(menu)).with_priority(2).into_any_element()
}
