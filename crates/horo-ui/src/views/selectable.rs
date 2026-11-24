//! Message text you can select and copy, links and all.
//!
//! gpui-component's `SelectableText` owns its text, so it can't also be the
//! `InteractiveText` that makes links clickable. This wraps any text element
//! instead: it registers the text's layout with the window's selection
//! (the `TextSelectionLayer` gpui-component's `Root` mounts, whose ⌘C copies
//! what's selected), paints the selection under it, and leaves clicks to the
//! element inside.

use gpui::{
    AnyElement, App, Bounds, Corners, Edges, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    PaintQuad, Pixels, Point, SharedString, TextLayout, Window, transparent_black,
};
use gpui_base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};

/// `inner` (a `StyledText`, or an `InteractiveText` around one), whose
/// `text` was laid out into `layout`, made selectable.
pub struct Selectable {
    id: ElementId,
    text: SharedString,
    layout: TextLayout,
    inner: AnyElement,
    colour: Hsla,
}

pub fn selectable(id: impl Into<ElementId>, text: impl Into<SharedString>, layout: TextLayout, inner: impl IntoElement, colour: Hsla) -> Selectable {
    Selectable { id: id.into(), text: text.into(), layout, inner: inner.into_any_element(), colour }
}

/// What the element keeps between frames: its run's selection, and a
/// redraw when a drag changes it.
struct Kept {
    handle: TextSelectionHandle,
    _refresh: gpui::Subscription,
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, global_id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, TextSelectionHandle) {
        let text = self.text.clone();
        let handle = window.with_element_state(global_id.expect("a selectable has an id"), |kept: Option<Kept>, window| {
            let kept = kept.unwrap_or_else(|| {
                let handle = TextSelectionHandle::new(text, cx);
                let refresh = handle.refresh_window_on_change(window, cx);
                Kept { handle, _refresh: refresh }
            });
            (kept.handle.clone(), kept)
        });
        // The text may have changed (an edit): copying falls back to it.
        handle.set_fallback_copy_text(self.text.to_string(), cx);
        (self.inner.request_layout(window, cx), handle)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut TextSelectionHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.inner.prepaint(window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds).with_text_bounds(vec![bounds]).with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut TextSelectionHandle,
        _: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let projection = handle.update_runs(&[TextSelectionRun::new(self.text.clone(), self.layout.clone(), bounds)], cx);
        for range in projection.ranges().iter().flatten() {
            let (Some(start), Some(end)) = (self.layout.position_for_index(range.start), self.layout.position_for_index(range.end)) else { continue };
            for quad in quads(start, end, self.layout.bounds(), self.layout.line_height()) {
                window.paint_quad(PaintQuad {
                    bounds: quad,
                    background: self.colour.into(),
                    corner_radii: Corners::default(),
                    border_widths: Edges::default(),
                    border_color: transparent_black(),
                    border_style: Default::default(),
                });
            }
        }
        self.inner.paint(window, cx);
    }
}

/// The boxes covering text from `start` to `end`: the rest of the first
/// line, whole lines between, the start of the last.
fn quads(start: Point<Pixels>, end: Point<Pixels>, bounds: Bounds<Pixels>, line_height: Pixels) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(start, Point::new(end.x, end.y + line_height))];
    }
    let mut out = vec![Bounds::from_corners(start, Point::new(bounds.right(), start.y + line_height))];
    if end.y > start.y + line_height {
        out.push(Bounds::from_corners(Point::new(bounds.left(), start.y + line_height), Point::new(bounds.right(), end.y)));
    }
    out.push(Bounds::from_corners(Point::new(bounds.left(), end.y), Point::new(end.x, end.y + line_height)));
    out
}
