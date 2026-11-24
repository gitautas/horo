//! The frame the WebView build's `Modal` draws: a dimmed window with a
//! card in the middle, a title, and a close button. Escape and a press
//! outside the card close it; what closing means is the caller's.

use gpui::{
    AnyElement, App, ClickEvent, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseDownEvent, ParentElement,
    StatefulInteractiveElement, Styled, Window, div,
};
use gpui_component::{h_flex, v_flex};

use crate::theme::zpx;
use crate::{
    theme::{Palette, corner, heading, space},
    views::verification::scrim,
};

pub fn modal(
    id: &'static str,
    title: String,
    width: f32,
    focus: &FocusHandle,
    palette: &Palette,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    body: AnyElement,
) -> AnyElement {
    let (escape, outside, button) = (on_close.clone(), on_close.clone(), on_close);
    scrim(palette)
        .child(
            v_flex()
                .id(id)
                .track_focus(focus)
                .w(zpx(width))
                .max_w_full()
                .max_h(gpui::relative(0.88))
                .rounded(corner(24.))
                .bg(palette.card)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                .text_color(palette.text)
                .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        cx.stop_propagation();
                        escape(window, cx);
                    }
                })
                .on_mouse_down_out(move |_: &MouseDownEvent, window, cx| outside(window, cx))
                .child(
                    h_flex()
                        .flex_none()
                        .justify_between()
                        .px(space(24.))
                        .pt(space(22.))
                        .pb(space(14.))
                        .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(21.)).child(title))
                        .child(
                            div()
                                .id((id, 1usize))
                                .size(zpx(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .text_color(palette.text_tertiary)
                                .hover(|d| d.bg(palette.raised).text_color(palette.text))
                                .child("✕")
                                .on_click(move |_: &ClickEvent, window, cx| button(window, cx)),
                        ),
                )
                .child(div().id((id, 2usize)).flex_1().min_h_0().overflow_y_scroll().px(space(24.)).pb(space(24.)).child(body)),
        )
        .into_any_element()
}

/// The small upper-case label over a field, as `RaveLabel` draws it.
pub fn label(palette: &Palette, words: &str) -> gpui::Div {
    div()
        .mb(zpx(6.))
        .font_family(palette.heading_font.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(crate::theme::label(9.5))
        .text_color(palette.text_tertiary)
        .child(words.to_owned())
}

/// A text field's well.
pub fn well(palette: &Palette) -> gpui::Div {
    h_flex()
        .gap(space(8.))
        .px(space(13.))
        .py(space(8.))
        .rounded(corner(14.))
        .bg(palette.inset)
        .border_1()
        .border_color(palette.border_subtle)
        .text_size(crate::theme::text(14.))
}

/// The WebView build's main `Button`: gold, ink text, dimmed while it
/// can't be pressed. Clicks are the caller's, through `on_click`, which
/// should do nothing while `enabled` is false.
pub fn accent_button(id: impl Into<gpui::ElementId>, words: &str, enabled: bool, small: bool, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    use gpui::prelude::FluentBuilder;
    div()
        .id(id)
        .flex_none()
        .px(space(if small { 12. } else { 18. }))
        .py(space(if small { 4. } else { 8. }))
        .rounded_full()
        .bg(palette.accent)
        .text_color(palette.text_on_accent)
        .font_weight(FontWeight::BOLD)
        .text_size(crate::theme::text(if small { 12. } else { 13.5 }))
        .child(words.to_owned())
        .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(palette.accent_hover)))
        .when(!enabled, |d| d.opacity(0.45))
}
