//! The call's pieces of the window, as the WebView build has them:
//!
//! - [`panel`]: "voice connected" at the foot of the room list, where
//!   Discord keeps it: status, the room (click to go there), mute, leave.
//! - [`roster`]: who's in a voice room, under it in the list; live from the
//!   call while we're in it, from the room's call membership otherwise.
//! - [`strip`]: everyone in the call, above the room's timeline while it's
//!   the call's room, with a ring on whoever is speaking.
//! - [`ringing`]: someone calling you, with answer and decline.
//!
//! The room header's call button lives with the header, in `shell`.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, div, prelude::FluentBuilder,
};
use gpui_component::{
    Sizable,
    Icon,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::dto::RoomSummary;
use horo_rtc::call::screen_key;

use crate::theme::{corner, space, text, zpx};
use crate::{
    call::{CallStatus, CallView, FocusMode, user_of},
    store::{Known, Store},
    theme::Palette,
    views::{
        avatar::{avatar, speaking_ring},
        verification::scrim,
    },
};

pub fn icon(path: &'static str) -> Icon {
    Icon::default().path(path)
}

/// Someone in a call, as listed.
#[derive(Clone)]
struct Person {
    user_id: String,
    /// Their identity in the call (one device), when known.
    identity: Option<String>,
    /// Their camera is on.
    camera: bool,
    /// They're sharing their screen.
    screen: bool,
    /// Their name, or "you".
    name: String,
    /// Their own name, for the monogram, and their picture.
    real_name: String,
    avatar: Option<String>,
    speaking: bool,
    /// Only known for ourselves.
    muted: bool,
}

fn localpart(user_id: &str) -> &str {
    user_id.trim_start_matches('@').split(':').next().unwrap_or(user_id)
}

/// Everyone in our call, ourselves first, one entry per person (several
/// devices of one person are one person).
fn people(call: &CallView, profiles: &HashMap<String, Known>) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::new();
    let mut add = |identity: &str, name: &str, muted: bool, me: bool, camera: bool, screen: bool| {
        let user = user_of(identity).to_owned();
        // A person's devices are one tile, except a device showing its
        // camera: that picture needs a tile of its own (your phone's, say,
        // beside this computer's).
        if !camera && !screen && out.iter().any(|p| p.user_id == user && !p.camera && !p.screen) {
            return;
        }
        let mut person = person(&user, profiles, call.is_speaking(&user), muted);
        person.identity = Some(identity.to_owned());
        person.camera = camera;
        person.screen = screen;
        if me {
            person.name = "you".into();
        } else if !profiles.get(&user).is_some_and(|k| k.name.is_some()) && !name.is_empty() && name != identity {
            // The call's own name for them until their profile arrives.
            person.name = name.to_owned();
            person.real_name = name.to_owned();
        }
        out.push(person);
    };
    if let Some(me) = &call.identity {
        add(me, "", call.muted, true, false, false);
    }
    for p in &call.participants {
        add(&p.identity, &p.name, p.muted, false, p.camera, p.screen);
    }
    out
}

/// Someone, by what their profile says (their username until it's in).
fn person(user: &str, profiles: &HashMap<String, Known>, speaking: bool, muted: bool) -> Person {
    let known = profiles.get(user);
    let name = known.and_then(|k| k.name.clone()).unwrap_or_else(|| localpart(user).to_owned());
    Person { user_id: user.to_owned(), identity: None, camera: false, screen: false, real_name: name.clone(), name, avatar: known.and_then(|k| k.avatar.clone()), speaking, muted }
}

/// Ask for the profiles of the people shown that we don't know yet.
fn want(store: &Entity<Store>, people: &[Person], cx: &mut App) {
    let users: Vec<String> = people.iter().map(|p| p.user_id.clone()).filter(|u| !store.read(cx).profiles.contains_key(u)).collect();
    if !users.is_empty() {
        let store = store.clone();
        cx.defer(move |cx| store.update(cx, |s, cx| s.want_profiles(users, cx)));
    }
}

pub fn panel(store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let call = state.call.clone()?;
    let room_name = state.rooms.iter().find(|r| r.id == call.room_id).map(|r| r.name.clone()).unwrap_or_else(|| call.room_id.clone());
    let (status, colour) = match &call.status {
        CallStatus::Connected => ("voice connected".to_owned(), palette.online),
        CallStatus::Connecting => ("connecting…".to_owned(), palette.idle),
        CallStatus::Reconnecting => ("reconnecting…".to_owned(), palette.idle),
        CallStatus::Failed(why) => (format!("couldn't connect: {why}"), palette.danger),
    };
    let failed = matches!(call.status, CallStatus::Failed(_));

    let (open, leave, expand, pop_out) = (store.clone(), store.clone(), store.clone(), store.clone());
    let room_id = call.room_id.clone();
    let pip_hidden = call.pip_hidden;
    let corner: AnyElement = if failed {
        Button::new("call-dismiss").ghost().label("dismiss").on_click(move |_, _, cx| leave.update(cx, |s, cx| s.clear_failed_call(cx))).into_any_element()
    } else {
        h_flex()
            .gap_1()
            .when(pip_hidden, |row| {
                row.child(
                    Button::new("call-pip-show")
                        .ghost()
                        .small()
                        .icon(icon("icons/video.svg"))
                        .tooltip("show mini call window")
                        .on_click(move |_, _, cx| pop_out.update(cx, |s, cx| s.set_call_pip_hidden(false, cx))),
                )
            })
            .child(
                Button::new("call-expand-panel")
                    .ghost()
                    .small()
                    .icon(icon("icons/maximize-2.svg"))
                    .tooltip("fill the window with the call")
                    .on_click(move |_, _, cx| expand.update(cx, |s, cx| s.set_call_fullscreen(true, cx))),
            )
            .into_any_element()
    };

    Some(
        v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(palette.border_subtle)
            .child(
                h_flex().gap_2().child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                    // A failure is read in full (it says what to fix); the
                    // other states fit on a line.
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(div().flex_none().mt(zpx(5.)).size(zpx(8.)).rounded_full().bg(colour))
                        .child(div().min_w_0().text_sm().text_color(colour).map(|d| if failed { d } else { d.truncate() }).child(status)),
                )
                    .child(
                        div()
                            .id("call-room")
                            .truncate()
                            .text_xs()
                            .text_color(palette.text_tertiary)
                            .cursor_pointer()
                            .hover(|d| d.text_color(palette.text))
                            .child(room_name)
                            .on_click(move |_, _, cx| {
                                let room_id = room_id.clone();
                                open.update(cx, |s, cx| s.open_room(room_id, cx))
                            }),
                    ),
                )
                .child(corner),
            )
            .when(!failed, |d| d.child(controls(store, &call, "panel")))
            .into_any_element(),
    )
}

/// Who's in a voice room, listed under it.
pub fn roster(room: &RoomSummary, store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let live = state.call.as_ref().filter(|c| c.room_id == room.id && c.status == CallStatus::Connected);
    let people: Vec<Person> = match live {
        Some(call) => people(call, &state.profiles),
        None => room.call_participants.iter().map(|user| person(user, &state.profiles, false, false)).collect(),
    };
    let own_identity = live.and_then(|c| c.identity.clone());
    let volumes = live.map(|c| c.participant_volumes.clone()).unwrap_or_default();
    let in_live_call = live.is_some();
    want(store, &people, cx);
    if people.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for (ix, person) in people.into_iter().enumerate() {
        let face = avatar(&person.user_id, &person.real_name, person.avatar.as_deref(), zpx(20.), cx);
        let remote_id = if in_live_call {
            person.identity.clone().filter(|id| own_identity.as_deref() != Some(id.as_str()))
        } else {
            None
        };
        let vol = remote_id.as_deref().and_then(|id| volumes.get(id).copied()).unwrap_or(1.0);
        let local_muted = vol <= 0.001;
        let controls = remote_id.map(|id| {
            let (s_down, s_mute, s_up) = (store.clone(), store.clone(), store.clone());
            let (id_down, id_mute, id_up) = (id.clone(), id.clone(), id);
            let pct = (vol * 100.0).round() as u32;
            h_flex()
                .ml_auto()
                .flex_none()
                .gap(zpx(2.))
                .items_center()
                .when(!local_muted && pct == 100, |d| d.invisible().group_hover("voice-row", |s| s.visible()))
                .child(
                    div()
                        .id(("v-down", ix))
                        .px(zpx(4.))
                        .rounded(corner(4.))
                        .cursor_pointer()
                        .text_size(text(10.))
                        .text_color(palette.text_tertiary)
                        .hover(|d| d.bg(palette.raised).text_color(palette.text))
                        .child("−")
                        .on_click(move |_, _, cx| s_down.update(cx, |s, cx| s.set_participant_volume(id_down.clone(), (vol - 0.25).max(0.0), cx))),
                )
                .child(
                    div()
                        .id(("v-mute", ix))
                        .px(zpx(4.))
                        .rounded(corner(4.))
                        .cursor_pointer()
                        .font_family(palette.mono_font.clone())
                        .text_size(text(9.5))
                        .text_color(if local_muted { palette.danger } else { palette.text_tertiary })
                        .hover(|d| d.bg(palette.raised).text_color(palette.text))
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(if local_muted { "unmute for me" } else { "mute for me" }).build(window, cx)
                        })
                        .child(if local_muted { "muted".to_owned() } else { format!("{pct}%") })
                        .on_click(move |_, _, cx| s_mute.update(cx, |s, cx| s.set_participant_volume(id_mute.clone(), if local_muted { 1.0 } else { 0.0 }, cx))),
                )
                .child(
                    div()
                        .id(("v-up", ix))
                        .px(zpx(4.))
                        .rounded(corner(4.))
                        .cursor_pointer()
                        .text_size(text(10.))
                        .text_color(palette.text_tertiary)
                        .hover(|d| d.bg(palette.raised).text_color(palette.text))
                        .child("+")
                        .on_click(move |_, _, cx| s_up.update(cx, |s, cx| s.set_participant_volume(id_up.clone(), (vol + 0.25).min(2.0), cx))),
                )
        });
        rows.push(
            h_flex()
                .group("voice-row")
                .gap_2()
                .pl(zpx(40.))
                .pr(space(8.))
                .py(zpx(2.))
                .items_center()
                .child(speaking_ring(face, zpx(20.), person.speaking && !local_muted, palette))
                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(if person.speaking && !local_muted { palette.text } else { palette.text_secondary }).child(person.name))
                .when(person.muted, |row| row.child(icon("icons/mic-off.svg").size(zpx(12.)).text_color(palette.text_tertiary)))
                .children(controls),
        );
    }
    Some(v_flex().pb_1().children(rows).into_any_element())
}

/// What a tile in the call shows.
#[derive(Clone)]
enum Shows {
    /// Someone's shared screen.
    Screen { identity: String },
    /// Ours, while we share it (not echoed back at us: a hall of mirrors).
    Sharing,
    /// Our camera, straight from it rather than round the server.
    OwnCamera,
    /// Someone's camera.
    Camera { identity: String },
    /// Someone without a picture: their face.
    Face,
}

/// One tile: what it shows, whose it is, and the key focusing it.
struct Tile {
    key: String,
    shows: Shows,
    person: Person,
}

/// Every tile in the call, in order: a shared screen before its sharer.
fn tiles(call: &CallView, store: &Entity<Store>, cx: &mut App) -> Vec<Tile> {
    let everyone = people(call, &store.read(cx).profiles);
    want(store, &everyone, cx);
    let mut out = Vec::new();
    for person in everyone {
        let id = person.identity.clone();
        let mine = id.is_some() && id == call.identity;
        let key = id.clone().unwrap_or_else(|| person.user_id.clone());
        if person.screen
            && let Some(identity) = &id
        {
            out.push(Tile { key: screen_key(identity), shows: Shows::Screen { identity: identity.clone() }, person: person.clone() });
        }
        if mine && call.sharing {
            out.push(Tile { key: "sharing".into(), shows: Shows::Sharing, person: person.clone() });
        }
        let shows = if mine && call.camera {
            Shows::OwnCamera
        } else if person.camera
            && let Some(identity) = &id
        {
            Shows::Camera { identity: identity.clone() }
        } else {
            Shows::Face
        };
        out.push(Tile { key, shows, person });
    }
    out
}

/// How big a tile is drawn: so many design pixels, or all its parent has.
#[derive(Clone, Copy)]
enum Size {
    Fixed(f32, f32),
    Fill,
}

/// The biggest 16:9 tiles `count` of which fit in `width` x `height` with
/// gaps between them, over whichever number of columns does best.
fn fit(count: usize, width: f32, height: f32) -> (f32, f32) {
    const GAP: f32 = 8.;
    let mut best = (0., 0.);
    for columns in 1..=count.max(1) {
        let rows = count.max(1).div_ceil(columns);
        let w = (width - GAP * (columns - 1) as f32) / columns as f32;
        let h = (height - GAP * (rows - 1) as f32) / rows as f32;
        let w = w.min(h * 16. / 9.).floor();
        if w > best.0 {
            best = (w, (w * 9. / 16.).floor());
        }
    }
    best
}

/// The size of the tiles beside a focused one.
const FILM: (f32, f32) = (176., 99.);

/// One tile, drawn at `size`. Clicking it focuses it; clicking the focused
/// one goes back to everyone alike.
fn draw(tile: &Tile, size: Size, focused: bool, store: &Entity<Store>, palette: &Palette, cx: &mut App) -> AnyElement {
    let screens = store.read(cx).call_live.as_ref().map(|l| l.screens.clone());
    // A face can be cropped to fill a small tile; a screen, or anything
    // shown big, is shown whole.
    let fit = if matches!(size, Size::Fill) || matches!(tile.shows, Shows::Screen { .. }) { gpui::ObjectFit::Contain } else { gpui::ObjectFit::Cover };
    let picture = match &tile.shows {
        Shows::Screen { identity } => screens.as_ref().and_then(|s| camera_picture(s, &screen_key(identity), fit, cx)),
        Shows::Camera { identity } => screens.as_ref().and_then(|s| camera_picture(s, identity, fit, cx)),
        Shows::OwnCamera => own_camera(store, fit, cx),
        Shows::Sharing | Shows::Face => None,
    };
    let person = &tile.person;
    let screen = matches!(tile.shows, Shows::Screen { .. });
    let label = if screen { format!("{}'s screen", person.real_name) } else { person.name.clone() };
    let key = tile.key.clone();
    let click = store.clone();
    let base = div()
        .id(gpui::SharedString::from(format!("tile-{key}")))
        .relative()
        .flex_none()
        .overflow_hidden()
        .rounded(zpx(16.))
        .cursor_pointer()
        .map(|d| match size {
            Size::Fixed(w, h) => d.w(zpx(w)).h(zpx(h)),
            Size::Fill => d.size_full(),
        })
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(if focused { "back to everyone" } else { "show bigger" }).build(window, cx))
        .on_click(move |_, _, cx| {
            let next = if focused { FocusMode::Grid } else { FocusMode::Pinned(key.clone()) };
            click.update(cx, |s, cx| s.set_call_focus(next, cx));
        });
    let small = matches!(size, Size::Fixed(_, h) if h < 120.);

    if let Some(picture) = picture {
        return base
            .bg(if screen { Hsla::black() } else { palette.inset })
            .when(!screen, |d| d.border_2().border_color(if person.speaking { palette.online } else { gpui::transparent_black() }))
            .child(picture)
            .child(
                h_flex()
                    .absolute()
                    .left(zpx(6.))
                    .bottom(zpx(6.))
                    .right(zpx(6.))
                    .child(
                        h_flex()
                            .gap_1()
                            .px(zpx(6.))
                            .py(zpx(2.))
                            .rounded(zpx(8.))
                            .bg(Hsla { a: 0.65, ..palette.app })
                            .min_w_0()
                            .child(div().truncate().text_xs().child(label))
                            .when(person.muted && !screen, |row| row.child(icon("icons/mic-off.svg").size(zpx(12.)).text_color(palette.danger))),
                    ),
            )
            .into_any_element();
    }
    if matches!(tile.shows, Shows::Sharing) {
        return base
            .child(
                v_flex()
                    .size_full()
                    .gap_2()
                    .items_center()
                    .justify_center()
                    .bg(palette.card)
                    .border_1()
                    .border_color(palette.accent)
                    .rounded(zpx(16.))
                    .child(icon("icons/screen-share.svg").size(zpx(28.)).text_color(palette.accent))
                    .when(!small, |d| d.child(div().text_xs().text_color(palette.text_secondary).child("you're sharing your screen"))),
            )
            .into_any_element();
    }
    // A face (or a picture still on its way).
    let face_size = match size {
        Size::Fixed(_, h) => (h * 0.54).min(72.),
        Size::Fill => 112.,
    };
    let face = avatar(&person.user_id, &person.real_name, person.avatar.as_deref(), zpx(face_size), cx);
    // The whole tile lights too, as a camera's does.
    base.bg(palette.card)
        .border_2()
        .border_color(if person.speaking { palette.online } else { gpui::transparent_black() })
        .child(
            v_flex()
                .size_full()
                .gap_1()
                .px_1()
                .items_center()
                .justify_center()
                .child(speaking_ring(face, zpx(face_size), person.speaking, palette))
                .child(
                    h_flex()
                        .gap_1()
                        .max_w_full()
                        .child(div().truncate().text_xs().child(label))
                        .when(person.muted, |row| row.child(icon("icons/mic-off.svg").size(zpx(12.)).text_color(palette.danger))),
                ),
        )
        .into_any_element()
}

/// The call's tiles: everyone alike, or one big with the rest beside it.
/// `full` when the call fills the window (bigger tiles, the big one
/// taking what room there is).
/// `area` is the room the tiles have, in design pixels, when the call
/// fills the window: everyone alike then shares it in one grid.
fn stage(call: &CallView, store: &Entity<Store>, palette: &Palette, full: bool, area: (f32, f32), cx: &mut App) -> AnyElement {
    let mut tiles = tiles(call, store, cx);
    // In the mini PiP modal: when someone has video on, focus on video tiles
    // so they stay large; when it's voice-only, show everyone's avatar tile
    // with live speaking rings.
    if !full && tiles.iter().any(|t| !matches!(t.shows, Shows::Face)) {
        tiles.retain(|t| !matches!(t.shows, Shows::Face));
    }
    let first_screen = || tiles.iter().position(|t| matches!(t.shows, Shows::Screen { .. }));
    let focused = match &call.focus {
        FocusMode::Auto => first_screen(),
        FocusMode::Pinned(key) => tiles.iter().position(|t| &t.key == key).or_else(first_screen),
        FocusMode::Grid => None,
    };
    let Some(focused) = focused else {
        let fitted = fit(tiles.len(), area.0, area.1);
        let drawn: Vec<AnyElement> = tiles
            .iter()
            .map(|t| draw(t, Size::Fixed(fitted.0, fitted.1), false, store, palette, cx))
            .collect();
        return h_flex()
            .flex_wrap()
            .gap_2()
            .justify_center()
            .when(full, |d| d.flex_1().min_h_0().content_center())
            .children(drawn)
            .into_any_element();
    };
    let big = draw(&tiles[focused], Size::Fill, true, store, palette, cx);
    let others: Vec<AnyElement> = tiles
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != focused)
        .map(|(_, t)| {
            let sz = if full { FILM } else { (98., 56.) };
            draw(t, Size::Fixed(sz.0, sz.1), false, store, palette, cx)
        })
        .collect();
    v_flex()
        .gap_2()
        .when(full, |d| d.flex_1().min_h_0())
        .child(div().w_full().map(|d| if full { d.flex_1().min_h_0() } else { d.h(zpx(area.1)) }).child(big))
        .when(!others.is_empty(), |d| d.child(h_flex().flex_wrap().gap_2().when(full, |d| d.justify_center()).children(others)))
        .into_any_element()
}

/// Drag payload for repositioning the floating mini call modal.
#[derive(Clone, Copy)]
pub struct DraggedCallPip;

/// Drag payload for resizing the floating mini call modal from a corner.
#[derive(Clone, Copy)]
pub struct ResizedCallPip(pub crate::call::PipResizeCorner);

/// Floating mini call modal anchored in the bottom-right corner (draggable and
/// resizable) whenever you're in a live call (unless fullscreen or minimized).
/// Shows video tiles when anyone is on camera/screen, or participant avatar
/// tiles with speaking rings in voice-only calls.
pub fn floating(store: &Entity<Store>, palette: &Palette, window: &gpui::Window, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let call = state.call.clone().filter(|c| c.is_live() && !c.fullscreen && !c.pip_hidden)?;
    let room_id = call.room_id.clone();
    let room_name = state
        .rooms
        .iter()
        .find(|r| r.id == call.room_id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| call.room_id.clone());
    let connecting = call.status == CallStatus::Connecting;
    let expand = store.clone();
    let hide = store.clone();
    let open_room = store.clone();
    let drag_start = store.clone();
    let zoom = crate::theme::zoom();
    let vp = window.viewport_size();
    let (vw, vh) = (f32::from(vp.width) / zoom, f32::from(vp.height) / zoom);
    let (raw_w, raw_h) = call.pip_size.unwrap_or(crate::call::PIP_DEFAULT_SIZE);
    let pip_w = raw_w.clamp(crate::call::PIP_MIN_SIZE.0, (vw - 24.).min(960.).max(crate::call::PIP_MIN_SIZE.0));
    let pip_h = raw_h.clamp(crate::call::PIP_MIN_SIZE.1, (vh - 140.).min(640.).max(crate::call::PIP_MIN_SIZE.1));
    let (raw_right, raw_bottom) = call.pip_pos.unwrap_or((18., 18.));
    let right = raw_right.clamp(12., (vw - pip_w - 12.).max(12.));
    let bottom = raw_bottom.clamp(12., (vh - pip_h - 110.).max(12.));

    let resize_handle = |id: &'static str, corner: crate::call::PipResizeCorner, cursor: gpui::CursorStyle, store: Entity<Store>| {
        div()
            .id(id)
            .absolute()
            .size(zpx(16.))
            .cursor(cursor)
            .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
                cx.stop_propagation();
                let z = crate::theme::zoom();
                let pos = (f32::from(event.position.x) / z, f32::from(event.position.y) / z);
                store.update(cx, |s, _| s.begin_call_pip_resize(pos));
            })
            .on_drag(ResizedCallPip(corner), |_, _, _, cx| cx.new(|_| gpui::EmptyView))
    };

    let tl_handle = resize_handle("call-pip-resize-tl", crate::call::PipResizeCorner::TopLeft, gpui::CursorStyle::ResizeUpLeftDownRight, store.clone())
        .top_0()
        .left_0();
    let bl_handle = resize_handle("call-pip-resize-bl", crate::call::PipResizeCorner::BottomLeft, gpui::CursorStyle::ResizeUpRightDownLeft, store.clone())
        .bottom_0()
        .left_0();
    let br_handle = resize_handle("call-pip-resize-br", crate::call::PipResizeCorner::BottomRight, gpui::CursorStyle::ResizeUpLeftDownRight, store.clone())
        .bottom_0()
        .right_0()
        .flex()
        .items_end()
        .justify_end()
        .p(zpx(4.))
        .child(div().size(zpx(6.)).rounded_br(corner(2.)).border_r_2().border_b_2().border_color(palette.text_tertiary).opacity(0.55));

    Some(
        v_flex()
            .id("call-floating")
            .absolute()
            .bottom(zpx(bottom))
            .right(zpx(right))
            .w(zpx(pip_w))
            .occlude()
            .rounded(corner(16.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.border)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .id("call-floating-header")
                    .items_center()
                    .justify_between()
                    .gap(space(8.))
                    .px(space(12.))
                    .py(space(8.))
                    .border_b_1()
                    .border_color(palette.border_subtle)
                    .cursor_grab()
                    .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
                        let z = crate::theme::zoom();
                        let pos = (f32::from(event.position.x) / z, f32::from(event.position.y) / z);
                        drag_start.update(cx, |s, _| s.begin_call_pip_drag(pos));
                    })
                    .on_drag(DraggedCallPip, |_, _, _, cx| cx.new(|_| gpui::EmptyView))
                    .child(
                        h_flex()
                            .id("call-floating-room")
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(space(7.))
                            .cursor_pointer()
                            .child(div().flex_none().size(zpx(8.)).rounded_full().bg(palette.online))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(text(12.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(palette.text)
                                    .child(room_name),
                            )
                            .on_click(move |_, _, cx| {
                                let id = room_id.clone();
                                open_room.update(cx, |s, cx| s.open_room(id, cx));
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("call-expand")
                                    .ghost()
                                    .small()
                                    .icon(icon("icons/maximize-2.svg"))
                                    .tooltip("fill the window")
                                    .on_click(move |_, _, cx| expand.update(cx, |s, cx| s.set_call_fullscreen(true, cx))),
                            )
                            .child(
                                Button::new("call-pip-hide")
                                    .ghost()
                                    .small()
                                    .icon(icon("icons/minimize-2.svg"))
                                    .tooltip("minimize mini call window")
                                    .on_click(move |_, _, cx| hide.update(cx, |s, cx| s.set_call_pip_hidden(true, cx))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .p(space(10.))
                    .gap(space(8.))
                    .when(connecting, |col| col.child(div().text_xs().text_color(palette.text_tertiary).child("joining the call…")))
                    .child(stage(&call, store, palette, false, (pip_w - 20., pip_h), cx))
                    .child(h_flex().justify_center().child(controls(store, &call, "float"))),
            )
            .child(tl_handle)
            .child(bl_handle)
            .child(br_handle)
            .into_any_element(),
    )
}

/// The call filling the window: its tiles as big as they'll go, and its
/// controls under them. Escape (or the button) puts it back.
pub fn fullscreen(store: &Entity<Store>, palette: &Palette, window: &gpui::Window, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let call = state.call.clone().filter(|c| c.fullscreen && c.is_live())?;
    // The window in design pixels, less the padding, the title row, the
    // controls and the gaps between them.
    let (zoom, viewport) = (crate::theme::zoom(), window.viewport_size());
    let area = (f32::from(viewport.width) / zoom - 32., f32::from(viewport.height) / zoom - 32. - 40. - 44. - 24.);
    let room_name = state.rooms.iter().find(|r| r.id == call.room_id).map(|r| r.name.clone()).unwrap_or_else(|| call.room_id.clone());
    let back = store.clone();
    Some(
        v_flex()
            .id("call-fullscreen")
            .absolute()
            .inset_0()
            .occlude()
            .bg(palette.app)
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    // Clear of the window's traffic lights.
                    .pl(zpx(64.))
                    .child(icon("icons/phone-call.svg").size(zpx(16.)).text_color(palette.online))
                    .child(div().flex_1().truncate().font_family(palette.heading_font.clone()).text_lg().font_weight(FontWeight::SEMIBOLD).child(room_name))
                    .child(
                        Button::new("call-shrink")
                            .ghost()
                            .icon(icon("icons/minimize-2.svg"))
                            .tooltip("back to the room (esc)")
                            .on_click(move |_, _, cx| back.update(cx, |s, cx| s.set_call_fullscreen(false, cx))),
                    ),
            )
            .child(stage(&call, store, palette, true, area, cx))
            .child(h_flex().justify_center().child(controls(store, &call, "full")))
            .into_any_element(),
    )
}

/// Floating per-user volume mixer popover (`0%–200%` + local mute per speaker),
/// accessible from the call controls in the voice panel, PiP modal, or fullscreen.
pub fn mixer(store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let call = state.call.clone().filter(|c| c.is_live() && c.mixer_open)?;
    let own_identity = call.identity.clone();
    let list = people(&call, &state.profiles);
    let remotes: Vec<Person> = list
        .into_iter()
        .filter(|p| p.identity.as_deref() != own_identity.as_deref())
        .collect();
    let any_custom = call.participant_volumes.values().any(|&v| (v - 1.0).abs() > 0.01);
    let close_out = store.clone();
    let close_btn = store.clone();
    let reset_store = store.clone();

    let rows: AnyElement = if remotes.is_empty() {
        div()
            .py(space(20.))
            .text_center()
            .text_xs()
            .text_color(palette.text_tertiary)
            .child("waiting for others to join the call…")
            .into_any_element()
    } else {
        v_flex()
            .gap(space(10.))
            .children(remotes.into_iter().enumerate().map(|(ix, p)| {
                let id = p.identity.clone().unwrap_or_else(|| p.user_id.clone());
                let vol = call.participant_volumes.get(&id).copied().unwrap_or(1.0);
                let pct = (vol * 100.0).round() as u32;
                let muted = vol <= 0.001;
                let face = avatar(&p.user_id, &p.real_name, p.avatar.as_deref(), zpx(26.), cx);
                let (s_mute, s_down, s_up) = (store.clone(), store.clone(), store.clone());
                let (id_mute, id_down, id_up) = (id.clone(), id.clone(), id.clone());

                // 20 clickable segments from 10% to 200% (segment 10 = 100%).
                let bar = h_flex()
                    .flex_1()
                    .h(zpx(18.))
                    .items_center()
                    .gap(zpx(2.))
                    .children((1u32..=20u32).map(|step| {
                        let target_vol = step as f32 * 0.1;
                        let lit = !muted && vol + 0.001 >= target_vol;
                        let boost = step > 10;
                        let is_unity = step == 10;
                        let s_seg = store.clone();
                        let id_seg = id.clone();
                        div()
                            .id(SharedString::from(format!("mix-seg-{ix}-{step}")))
                            .flex_1()
                            .h(zpx(if is_unity { 12. } else { 7. }))
                            .rounded(corner(2.))
                            .cursor_pointer()
                            .bg(if lit {
                                if boost { palette.idle } else { palette.accent }
                            } else {
                                palette.inset
                            })
                            .hover(|d| d.bg(if boost { palette.idle } else { palette.accent }))
                            .on_click(move |_, _, cx| {
                                let id = id_seg.clone();
                                s_seg.update(cx, |s, cx| s.set_participant_volume(id, target_vol, cx));
                            })
                    }));

                v_flex()
                    .gap(zpx(6.))
                    .p(space(9.))
                    .rounded(corner(12.))
                    .bg(palette.app)
                    .border_1()
                    .border_color(if p.speaking { palette.online } else { palette.border_subtle })
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap(space(8.))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .gap(space(8.))
                                    .child(face)
                                    .child(div().truncate().text_xs().font_weight(FontWeight::BOLD).text_color(palette.text).child(p.name)),
                            )
                            .child(
                                div()
                                    .id(("mix-mute", ix))
                                    .px(space(7.))
                                    .py(zpx(2.))
                                    .rounded(corner(6.))
                                    .cursor_pointer()
                                    .bg(if muted { Hsla { a: 0.16, ..palette.danger } } else { palette.card })
                                    .hover(|d| d.bg(palette.raised))
                                    .font_family(palette.mono_font.clone())
                                    .text_size(text(10.5))
                                    .text_color(if muted {
                                        palette.danger
                                    } else if pct > 100 {
                                        palette.idle
                                    } else {
                                        palette.text_secondary
                                    })
                                    .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("click to mute / unmute locally").build(window, cx))
                                    .child(if muted { "muted".to_owned() } else { format!("{pct}%") })
                                    .on_click(move |_, _, cx| {
                                        let id = id_mute.clone();
                                        s_mute.update(cx, |s, cx| s.set_participant_volume(id, if muted { 1.0 } else { 0.0 }, cx));
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap(space(6.))
                            .child(
                                div()
                                    .id(("mix-down", ix))
                                    .size(zpx(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(corner(6.))
                                    .cursor_pointer()
                                    .bg(palette.card)
                                    .hover(|d| d.bg(palette.raised))
                                    .text_xs()
                                    .text_color(palette.text_secondary)
                                    .child("−")
                                    .on_click(move |_, _, cx| {
                                        let id = id_down.clone();
                                        s_down.update(cx, |s, cx| s.set_participant_volume(id, (vol - 0.1).max(0.0), cx));
                                    }),
                            )
                            .child(bar)
                            .child(
                                div()
                                    .id(("mix-up", ix))
                                    .size(zpx(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(corner(6.))
                                    .cursor_pointer()
                                    .bg(palette.card)
                                    .hover(|d| d.bg(palette.raised))
                                    .text_xs()
                                    .text_color(palette.text_secondary)
                                    .child("+")
                                    .on_click(move |_, _, cx| {
                                        let id = id_up.clone();
                                        s_up.update(cx, |s, cx| s.set_participant_volume(id, (vol + 0.1).min(2.0), cx));
                                    }),
                            ),
                    )
            }))
            .into_any_element()
    };

    Some(
        v_flex()
            .id("call-mixer-popover")
            .occlude()
            .absolute()
            .bottom(space(78.))
            .right(space(18.))
            .w(zpx(320.))
            .max_h(zpx(400.))
            .rounded(corner(16.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.border)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down_out(move |_, _, cx| close_out.update(cx, |s, cx| s.close_call_mixer(cx)))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px(space(14.))
                    .py(space(10.))
                    .border_b_1()
                    .border_color(palette.border_subtle)
                    .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(palette.text).child("volume mixer"))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(space(6.))
                            .when(any_custom, |row| {
                                row.child(
                                    div()
                                        .id("mix-reset-all")
                                        .px(space(7.))
                                        .py(zpx(2.))
                                        .rounded(corner(6.))
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(palette.accent)
                                        .hover(|d| d.bg(palette.raised))
                                        .child("reset")
                                        .on_click(move |_, _, cx| reset_store.update(cx, |s, cx| s.reset_participant_volumes(cx))),
                                )
                            })
                            .child(
                                div()
                                    .id("mix-close")
                                    .px(space(6.))
                                    .py(zpx(2.))
                                    .rounded(corner(6.))
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(palette.text_tertiary)
                                    .hover(|d| d.bg(palette.raised).text_color(palette.text))
                                    .child("✕")
                                    .on_click(move |_, _, cx| close_btn.update(cx, |s, cx| s.close_call_mixer(cx))),
                            ),
                    ),
            )
            .child(div().id("mix-scroll").flex_1().min_h_0().overflow_y_scroll().p(space(10.)).child(rows))
            .into_any_element(),
    )
}

/// Our camera, screen, microphone, hearing, volume mixer and the way out.
fn controls(store: &Entity<Store>, call: &CallView, prefix: &'static str) -> gpui::Div {
    let row = h_flex().gap_1();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let row = {
        let (share, sharing) = (store.clone(), call.sharing);
        let (camera, camera_on) = (store.clone(), call.camera);
        row.child(
            Button::new(SharedString::from(format!("{prefix}-call-screen")))
                .ghost()
                .icon(icon(if sharing { "icons/screen-share-off.svg" } else { "icons/screen-share.svg" }))
                .tooltip(if sharing { "stop sharing your screen" } else { "share your screen" })
                .when(sharing, |b| b.primary())
                .on_click(move |_, _, cx| share.update(cx, |s, cx| s.set_call_screen(!sharing, cx))),
        )
        .child(
            Button::new(SharedString::from(format!("{prefix}-call-camera")))
                .ghost()
                .icon(icon(if camera_on { "icons/video.svg" } else { "icons/video-off.svg" }))
                .tooltip(if camera_on { "turn your camera off" } else { "turn your camera on" })
                .when(camera_on, |b| b.primary())
                .on_click(move |_, _, cx| camera.update(cx, |s, cx| s.set_call_camera(!camera_on, cx))),
        )
    };
    let (mute, deafen, mixer_store, leave) = (store.clone(), store.clone(), store.clone(), store.clone());
    let (muted, deafened, mixer_open) = (call.muted, call.deafened, call.mixer_open);
    row.child(
        Button::new(SharedString::from(format!("{prefix}-call-mute")))
            .ghost()
            .icon(icon(if muted { "icons/mic-off.svg" } else { "icons/mic.svg" }))
            .tooltip(if muted { "unmute" } else { "mute" })
            .when(muted, |b| b.danger())
            .on_click(move |_, _, cx| mute.update(cx, |s, cx| s.set_call_muted(!muted, cx))),
    )
    .child(
        Button::new(SharedString::from(format!("{prefix}-call-deafen")))
            .ghost()
            .icon(icon(if deafened { "icons/headphone-off.svg" } else { "icons/headphones.svg" }))
            .tooltip(if deafened { "undeafen" } else { "deafen" })
            .when(deafened, |b| b.danger())
            .on_click(move |_, _, cx| deafen.update(cx, |s, cx| s.set_call_deafened(!deafened, cx))),
    )
    .child(
        Button::new(SharedString::from(format!("{prefix}-call-mixer")))
            .ghost()
            .icon(gpui_component::Icon::new(gpui_component::IconName::Settings2))
            .tooltip("per-user volume mixer")
            .when(mixer_open, |b| b.primary())
            .on_click(move |_, _, cx| mixer_store.update(cx, |s, cx| s.toggle_call_mixer(cx))),
    )
    .child(
        Button::new(SharedString::from(format!("{prefix}-call-leave")))
            .danger()
            .icon(icon("icons/phone-off.svg"))
            .tooltip("leave the call")
            .on_click(move |_, _, cx| leave.update(cx, |s, cx| s.leave_call(cx))),
    )
}

/// Someone's camera (or screen, under its key), as an element filling its
/// tile: on macOS a surface over the newest picture (converted once per
/// picture, not once per redraw).
#[cfg(target_os = "macos")]
fn camera_picture(screens: &horo_rtc::call::Screens, identity: &str, fit: gpui::ObjectFit, _cx: &mut App) -> Option<AnyElement> {
    use std::{cell::RefCell, collections::HashMap};
    type Converted = (u64, objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>);
    thread_local! {
        static CONVERTED: RefCell<HashMap<String, Converted>> = RefCell::new(HashMap::new());
    }
    let picture = screens.get(identity)?;
    let buffer = CONVERTED.with_borrow_mut(|cache| {
        match cache.get(identity) {
            Some((seq, buffer)) if *seq == picture.seq => Some(buffer.clone()),
            _ => {
                let buffer = crate::video::pixel_buffer(&picture.frame)?;
                cache.insert(identity.to_owned(), (picture.seq, buffer.clone()));
                Some(buffer)
            }
        }
    })?;
    Some(gpui::surface(buffer).size_full().object_fit(fit).into_any_element())
}

/// What our camera sees now.
#[cfg(target_os = "macos")]
fn own_camera(store: &Entity<Store>, fit: gpui::ObjectFit, cx: &mut App) -> Option<AnyElement> {
    let buffer = store.update(cx, |s, _| s.call_live.as_mut()?.camera.as_mut()?.mirrored_frame())?;
    Some(gpui::surface(buffer).size_full().object_fit(fit).into_any_element())
}

/// Someone's camera (or screen, under its key) on Linux: a gpui image of
/// the newest picture, made once per picture.
#[cfg(target_os = "linux")]
fn camera_picture(screens: &horo_rtc::call::Screens, identity: &str, fit: gpui::ObjectFit, cx: &mut App) -> Option<AnyElement> {
    use gpui::StyledImage as _;
    let Some(picture) = screens.get(identity) else {
        crate::picture::forget(identity, cx);
        return None;
    };
    let image = crate::picture::image(identity, picture.seq, &picture.frame, false, cx)?;
    Some(gpui::img(image).size_full().object_fit(fit).into_any_element())
}

/// What our camera sees now, on Linux.
#[cfg(target_os = "linux")]
fn own_camera(store: &Entity<Store>, fit: gpui::ObjectFit, cx: &mut App) -> Option<AnyElement> {
    use gpui::StyledImage as _;
    let (seq, frame) = store.read(cx).call_live.as_ref()?.camera.as_ref()?.latest()?;
    let image = crate::picture::image(crate::call::OWN_CAMERA, seq, &frame, true, cx)?;
    Some(gpui::img(image).size_full().object_fit(fit).into_any_element())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn own_camera(_store: &Entity<Store>, _fit: gpui::ObjectFit, _cx: &mut App) -> Option<AnyElement> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn camera_picture(_screens: &horo_rtc::call::Screens, _identity: &str, _fit: gpui::ObjectFit, _cx: &mut App) -> Option<AnyElement> {
    None
}

/// Whether the call has anything to look at: a camera or a shared screen,
/// ours or anyone's.
fn has_video(call: &CallView) -> bool {
    call.camera || call.sharing || call.participants.iter().any(|p| p.camera || p.screen)
}

/// Whether anyone in our call has a camera on (the window then redraws
/// every frame for them).
pub fn any_camera(store: &Entity<Store>, cx: &App) -> bool {
    store.read(cx).call.as_ref().is_some_and(has_video)
}

/// Someone calling you: answer or decline, over everything.
pub fn ringing(store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let incoming = state.incoming.clone()?;
    let room = state.rooms.iter().find(|r| r.id == incoming.room_id).cloned();
    let name = room.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| incoming.room_id.clone());
    let video = room.as_ref().is_some_and(|r| r.is_video_room);
    let picture = avatar(&incoming.room_id, &name, room.as_ref().and_then(|r| r.avatar_url.as_deref()), zpx(72.), cx);
    let (answer, decline) = (store.clone(), store.clone());
    Some(
        scrim(palette)
            .child(
                v_flex()
                    .w(zpx(320.))
                    .p_6()
                    .gap_4()
                    .items_center()
                    .rounded(zpx(24.))
                    .bg(palette.card)
                    .border_1()
                    .border_color(Hsla { a: 0.5, ..palette.online })
                    .shadow_lg()
                    .child(picture)
                    .child(div().font_family(palette.heading_font.clone()).text_size(zpx(24.)).font_weight(FontWeight::SEMIBOLD).child(name))
                    .child(div().text_sm().text_color(palette.text_secondary).child(if video { "incoming video call~" } else { "incoming call~" }))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("call-decline")
                                    .danger()
                                    .icon(icon("icons/phone-off.svg"))
                                    .label("decline")
                                    .on_click(move |_, _, cx| decline.update(cx, |s, cx| s.stop_ringing(cx))),
                            )
                            .child(
                                Button::new("call-answer")
                                    .primary()
                                    .icon(icon("icons/phone.svg"))
                                    .label("answer")
                                    .on_click(move |_, _, cx| answer.update(cx, |s, cx| s.answer(cx))),
                            ),
                    ),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn tiles_fill_the_window() {
        // One tile on a wide screen is as tall as the room allows.
        assert_eq!(fit(1, 1600., 900.), (1600., 900.));
        // Three on a 16:10 screen: two rows of two beat three across.
        let (w, h) = fit(3, 1600., 1000.);
        assert_eq!((w, h), (796., 447.));
        // Three on a very wide, short area: one row.
        assert_eq!(fit(3, 2400., 500.).0, 794.);
        // Four on a near-square area: two by two.
        let (w, _) = fit(4, 1000., 800.);
        assert_eq!(w, 496.);
    }
}
