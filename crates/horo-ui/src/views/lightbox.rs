//! A picture full-size over the window: the thumbnail at once, the
//! original when it arrives. Clicking the backdrop closes it; clicking the
//! picture zooms in around the cursor (Signal-style), dragging pans while
//! zoomed, clicking again (without dragging) zooms back out, and scrolling
//! smoothly adjusts zoom.

use gpui::{
    AnyElement, App, CursorStyle, Entity, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, ScrollDelta, ScrollEvent, StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder, px,
};
use gpui_component::{h_flex, v_flex};

use crate::{media::Media, store::Store, theme::Palette, views::content};

/// Clear space around the picture.
const MARGIN: f32 = 32.;
/// Extra horizontal clearance when prev/next gallery arrows are shown.
const GALLERY_MARGIN_X: f32 = 80.;
/// Room under it for its name.
const CAPTION: f32 = 40.;
/// Maximum zoom factor.
const MAX_ZOOM: f32 = 5.0;
/// Pointer movement (logical px) that turns a click into a pan drag.
const DRAG_THRESHOLD: f32 = 4.0;

/// `natural` scaled down (never up) to fit in `room`.
pub fn fit_within(natural: (f32, f32), room: (f32, f32)) -> (f32, f32) {
    let (w, h) = natural;
    if w <= 0. || h <= 0. {
        return room;
    }
    let scale = (room.0 / w).min(room.1 / h).min(1.);
    (w * scale, h * scale)
}

/// Target zoom factor on a single click: matches natural pixel density for
/// large photos, clamped to a comfortable `[2.2, 3.5]` range.
pub fn click_zoom_target(natural: (f32, f32), fitted: (f32, f32)) -> f32 {
    if fitted.0 <= 0. || fitted.1 <= 0. {
        return 2.5;
    }
    (natural.0 / fitted.0).max(natural.1 / fitted.1).clamp(2.2, 3.5)
}

/// Clamp `pan` so a zoomed picture of size `zoomed` stays within `viewport`
/// without exposing empty margin past its edges.
pub fn clamp_pan(pan: (f32, f32), zoomed: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    let max_x = ((zoomed.0 - viewport.0) / 2.0).max(0.0);
    let max_y = ((zoomed.1 - viewport.1) / 2.0).max(0.0);
    (pan.0.clamp(-max_x, max_x), pan.1.clamp(-max_y, max_y))
}

/// Pan offset that keeps the point under `cursor` anchored when zooming from
/// `old_zoom` to `new_zoom`.
fn zoom_pan(pan: (f32, f32), old_zoom: f32, new_zoom: f32, cursor: (f32, f32), center: (f32, f32), fitted: (f32, f32), viewport: (f32, f32)) -> (f32, f32) {
    if new_zoom <= 1.01 {
        return (0.0, 0.0);
    }
    let ratio = new_zoom / old_zoom.max(0.001);
    let dx = cursor.0 - center.0;
    let dy = cursor.1 - center.1;
    let next = (dx - (dx - pan.0) * ratio, dy - (dy - pan.1) * ratio);
    clamp_pan(next, (fitted.0 * new_zoom, fitted.1 * new_zoom), viewport)
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
    let vp = (f32::from(viewport.width), f32::from(viewport.height));
    let margin_x = if has_gallery { GALLERY_MARGIN_X } else { MARGIN };
    let room = ((vp.0 - 2. * margin_x).max(64.), (vp.1 - 2. * MARGIN - CAPTION).max(64.));
    let zoom = gallery.zoom;
    let zoomed = zoom > 1.01;
    let dragging = gallery.drag.is_some();

    let mut fitted_size: Option<((f32, f32), (f32, f32))> = None;

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
            let fitted = fit_within(natural, room);
            fitted_size = Some((natural, fitted));
            let (zw, zh) = (fitted.0 * zoom, fitted.1 * zoom);
            let pan = if zoomed { clamp_pan(gallery.pan, (zw, zh), vp) } else { (0.0, 0.0) };

            let cursor = if !zoomed {
                CursorStyle::PointingHand
            } else if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            };

            let s_down = store.clone();
            let s_click = store.clone();
            let el = div()
                .id("lightbox-picture")
                .cursor(cursor)
                .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let pos = (f32::from(ev.position.x), f32::from(ev.position.y));
                    s_down.update(cx, |s, cx| {
                        s.update_lightbox(
                            |lb| {
                                if lb.zoom > 1.01 {
                                    lb.drag = Some((pos, lb.pan, false));
                                }
                            },
                            cx,
                        );
                    });
                })
                .on_click(move |ev, _, cx| {
                    cx.stop_propagation();
                    let pos = (f32::from(ev.position().x), f32::from(ev.position().y));
                    s_click.update(cx, |s, cx| {
                        s.update_lightbox(
                            |lb| {
                                if lb.zoom <= 1.01 {
                                    let target = click_zoom_target(natural, fitted);
                                    let center = (vp.0 / 2.0, (vp.1 - CAPTION) / 2.0);
                                    lb.pan = zoom_pan((0.0, 0.0), 1.0, target, pos, center, fitted, vp);
                                    lb.zoom = target;
                                    lb.drag = None;
                                }
                            },
                            cx,
                        );
                    });
                })
                .child(img(image).w(px(zw)).h(px(zh)).object_fit(gpui::ObjectFit::Contain));

            if zoomed {
                let left = (vp.0 - zw) / 2.0 + pan.0;
                let top = (vp.1 - zh) / 2.0 + pan.1;
                div()
                    .id("lightbox-zoom-stage")
                    .absolute()
                    .inset_0()
                    .overflow_hidden()
                    .child(el.absolute().left(px(left)).top(px(top)))
                    .into_any_element()
            } else {
                el.into_any_element()
            }
        }
        None => div().text_color(palette.text_tertiary).child("loading…").into_any_element(),
    };

    let (close_backdrop, close_btn, reset_zoom_btn) = (store.clone(), store.clone(), store.clone());
    let zoom_badge = zoomed.then(|| {
        let pct = (zoom * 100.0).round() as u32;
        div()
            .id("lightbox-zoom-reset")
            .h(px(34.))
            .px(px(12.))
            .rounded_full()
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .hover(|d| d.bg(palette.card))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .font_family(palette.mono_font.clone())
            .text_xs()
            .text_color(palette.text_secondary)
            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("reset zoom").build(window, cx))
            .child(format!("{pct}%"))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                reset_zoom_btn.update(cx, |s, cx| s.update_lightbox(crate::store::Lightbox::reset_zoom, cx));
            })
    });
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

    let arrow_top = ((vp.1 - 44.) / 2.).max(64.);
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

    let (s_move, s_up, s_wheel) = (store.clone(), store.clone(), store.clone());

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
            .on_mouse_move(move |ev: &MouseMoveEvent, _, cx| {
                let Some((_, fitted)) = fitted_size else { return };
                let pos = (f32::from(ev.position.x), f32::from(ev.position.y));
                let has_drag = s_move.read(cx).lightbox.as_ref().is_some_and(|lb| lb.drag.is_some());
                if !has_drag {
                    return;
                }
                s_move.update(cx, |s, cx| {
                    s.update_lightbox(
                        |lb| {
                            if let Some((start_mouse, start_pan, moved)) = &mut lb.drag {
                                let dx = pos.0 - start_mouse.0;
                                let dy = pos.1 - start_mouse.1;
                                if *moved || dx.hypot(dy) >= DRAG_THRESHOLD {
                                    *moved = true;
                                    let zoomed = (fitted.0 * lb.zoom, fitted.1 * lb.zoom);
                                    lb.pan = clamp_pan((start_pan.0 + dx, start_pan.1 + dy), zoomed, vp);
                                }
                            }
                        },
                        cx,
                    );
                });
            })
            .on_mouse_up(MouseButton::Left, move |_ev: &MouseUpEvent, _, cx| {
                let has_drag = s_up.read(cx).lightbox.as_ref().is_some_and(|lb| lb.drag.is_some());
                if !has_drag {
                    return;
                }
                cx.stop_propagation();
                s_up.update(cx, |s, cx| {
                    s.update_lightbox(
                        |lb| {
                            if let Some((_, _, moved)) = lb.drag.take()
                                && !moved
                            {
                                // Clicked while zoomed without dragging: zoom back out.
                                lb.reset_zoom();
                            }
                        },
                        cx,
                    );
                });
            })
            .on_scroll(move |ev: &ScrollEvent, _, cx| {
                let Some((_, fitted)) = fitted_size else { return };
                cx.stop_propagation();
                let dy = match ev.delta {
                    ScrollDelta::Pixels(p) => f32::from(p.y),
                    ScrollDelta::Lines(l) => l.y * 24.0,
                };
                if dy.abs() < 0.01 {
                    return;
                }
                let cursor = (f32::from(ev.position.x), f32::from(ev.position.y));
                s_wheel.update(cx, |s, cx| {
                    s.update_lightbox(
                        |lb| {
                            let old = lb.zoom;
                            let factor = (1.0 + dy * 0.004).clamp(0.7, 1.4);
                            let next = (old * factor).clamp(1.0, MAX_ZOOM);
                            if next <= 1.02 {
                                lb.reset_zoom();
                            } else {
                                let center = if old <= 1.01 { (vp.0 / 2.0, (vp.1 - CAPTION) / 2.0) } else { (vp.0 / 2.0, vp.1 / 2.0) };
                                lb.pan = zoom_pan(lb.pan, old, next, cursor, center, fitted, vp);
                                lb.zoom = next;
                            }
                        },
                        cx,
                    );
                });
            })
            .child(picture)
            .when(!zoomed, |root| {
                root.child(
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
            })
            .child(
                h_flex()
                    .absolute()
                    .top(px(18.))
                    .right(px(18.))
                    .gap(px(8.))
                    .children(zoom_badge)
                    .children(save_button)
                    .child(close_button),
            )
            .children(prev_button)
            .children(next_button)
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
    fn gallery_steps_within_bounds_and_resets_zoom() {
        use crate::store::{Lightbox, LightboxItem};
        let item = |name: &str| LightboxItem { mxc: None, thumbnail: None, width: None, height: None, name: name.to_owned() };
        let mut lb = Lightbox::new(vec![item("a"), item("b"), item("c")], 1);
        lb.zoom = 2.5;
        lb.pan = (40.0, -20.0);
        assert!(lb.can_prev() && lb.can_next());
        assert!(lb.step(-1));
        assert_eq!(lb.index, 0);
        assert_eq!(lb.zoom, 1.0);
        assert_eq!(lb.pan, (0.0, 0.0));
        assert!(!lb.can_prev() && !lb.step(-1));
        assert!(lb.step(1));
        assert!(lb.step(1));
        assert_eq!(lb.index, 2);
        assert!(!lb.can_next() && !lb.step(1));
    }

    #[test]
    fn pan_clamps_to_zoomed_overflow() {
        // Zoomed image smaller than viewport stays centered.
        assert_eq!(clamp_pan((100., -50.), (400., 300.), (1000., 800.)), (0., 0.));
        // Zoomed image larger than viewport clamps to half the overflow.
        assert_eq!(clamp_pan((500., -500.), (2000., 1400.), (1000., 800.)), (500., -300.));
        assert_eq!(clamp_pan((800., 800.), (2000., 1400.), (1000., 800.)), (500., 300.));
    }
}
