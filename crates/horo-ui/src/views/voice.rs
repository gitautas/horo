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
    h_flex,
    slider::Slider,
    v_flex,
};
use horo_core::dto::RoomSummary;
use horo_rtc::call::screen_key;

use crate::theme::{corner, space, text, zpx};
use crate::{
    call::{CallStatus, CallView, FocusMode, ParticipantMenu, user_of},
    store::{Known, Store},
    theme::Palette,
    views::{
        avatar::{avatar, speaking_ring},
        menu,
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
        let pct = (vol * 100.0).round() as u32;
        let menu_info = remote_id.map(|id| (id, person.user_id.clone(), person.real_name.clone(), person.avatar.clone()));
        let badge = menu_info.as_ref().filter(|_| local_muted || pct != 100).map(|(id, uid, name, av)| {
            let s_badge = store.clone();
            let (id, uid, name, av) = (id.clone(), uid.clone(), name.clone(), av.clone());
            div()
                .id(("v-badge", ix))
                .ml_auto()
                .flex_none()
                .px(zpx(5.))
                .py(zpx(1.))
                .rounded(corner(4.))
                .cursor_pointer()
                .font_family(palette.mono_font.clone())
                .text_size(text(9.5))
                .text_color(if local_muted {
                    palette.danger
                } else if pct > 100 {
                    palette.idle
                } else {
                    palette.text_tertiary
                })
                .hover(|d| d.bg(palette.raised).text_color(palette.text))
                .child(if local_muted { "muted".to_owned() } else { format!("{pct}%") })
                .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
                    cx.stop_propagation();
                    let target = ParticipantMenu {
                        identity: id.clone(),
                        user_id: uid.clone(),
                        name: name.clone(),
                        avatar: av.clone(),
                        at: event.position,
                    };
                    s_badge.update(cx, |s, cx| s.open_participant_menu(target, cx));
                })
        });
        let row = h_flex()
            .id(("voice-roster-row", ix))
            .gap_2()
            .pl(zpx(40.))
            .pr(space(8.))
            .py(zpx(2.))
            .rounded(corner(6.))
            .items_center()
            .when(menu_info.is_some(), |d| d.hover(|d| d.bg(palette.raised)))
            .when_some(menu_info, |d, (id, uid, name, av)| {
                let s_ctx = store.clone();
                d.on_mouse_down(gpui::MouseButton::Right, move |event, _, cx| {
                    cx.stop_propagation();
                    let target = ParticipantMenu {
                        identity: id.clone(),
                        user_id: uid.clone(),
                        name: name.clone(),
                        avatar: av.clone(),
                        at: event.position,
                    };
                    s_ctx.update(cx, |s, cx| s.open_participant_menu(target, cx));
                })
            })
            .child(speaking_ring(face, zpx(20.), person.speaking && !local_muted, palette))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(if person.speaking && !local_muted { palette.text } else { palette.text_secondary }).child(person.name))
            .when(person.muted, |row| row.child(icon("icons/mic-off.svg").size(zpx(12.)).text_color(palette.text_tertiary)))
            .children(badge);
        rows.push(row);
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

/// For `count` 16:9 tiles in `width` × `height` with `columns` columns,
/// the largest tile `(w, h)` that fits.
fn fit_for(count: usize, width: f32, height: f32, columns: usize) -> (f32, f32) {
    const GAP: f32 = 8.;
    let rows = count.max(1).div_ceil(columns);
    let w = (width - GAP * (columns - 1) as f32) / columns as f32;
    let h = (height - GAP * (rows - 1) as f32) / rows as f32;
    let w = w.min(h * 16. / 9.).floor().max(0.);
    (w, (w * 9. / 16.).floor().max(0.))
}

/// The biggest 16:9 tiles `count` of which fit in `width` × `height` with
/// gaps between them, returning `((tile_w, tile_h), columns)`.
/// When `prev_cols` is provided, keeps that column count unless another
/// layout is at least 12% wider so resizing across a boundary doesn't flicker.
fn fit_grid(count: usize, width: f32, height: f32, prev_cols: Option<usize>) -> ((f32, f32), usize) {
    let n = count.max(1);
    let mut best = ((0., 0.), 1);
    for columns in 1..=n {
        let size = fit_for(n, width, height, columns);
        if size.0 > best.0.0 {
            best = (size, columns);
        }
    }
    if let Some(prev) = prev_cols.filter(|&c| c >= 1 && c <= n) {
        let current = fit_for(n, width, height, prev);
        if current.0 > 0. && best.0.0 <= current.0 * 1.12 {
            return (current, prev);
        }
    }
    best
}

/// The biggest 16:9 tiles `count` of which fit in `width` × `height` with
/// gaps between them, over whichever number of columns does best.
#[cfg(test)]
fn fit(count: usize, width: f32, height: f32) -> (f32, f32) {
    fit_grid(count, width, height, None).0
}

/// The size of the tiles beside a focused one.
const FILM: (f32, f32) = (176., 99.);

/// One tile, drawn at `size`. Clicking it focuses it; clicking the focused
/// one goes back to everyone alike.
fn draw(tile: &Tile, size: Size, focused: bool, store: &Entity<Store>, palette: &Palette, cx: &mut App) -> AnyElement {
    let (screens, own_identity) = {
        let s = store.read(cx);
        (s.call_live.as_ref().map(|l| l.screens.clone()), s.call.as_ref().and_then(|c| c.identity.clone()))
    };
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
    let remote_menu = person
        .identity
        .clone()
        .filter(|id| own_identity.as_deref() != Some(id.as_str()))
        .map(|id| (id, person.user_id.clone(), person.real_name.clone(), person.avatar.clone()));
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
        })
        .when_some(remote_menu, |d, (id, uid, name, av)| {
            let s_ctx = store.clone();
            d.on_mouse_down(gpui::MouseButton::Right, move |event, _, cx| {
                cx.stop_propagation();
                let target = ParticipantMenu {
                    identity: id.clone(),
                    user_id: uid.clone(),
                    name: name.clone(),
                    avatar: av.clone(),
                    at: event.position,
                };
                s_ctx.update(cx, |s, cx| s.open_participant_menu(target, cx));
            })
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
        let (fitted, columns) = if full {
            fit_grid(tiles.len(), area.0, area.1, None)
        } else {
            thread_local! {
                static LAST_PIP_COLS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
            }
            let (prev_n, prev_c) = LAST_PIP_COLS.with(|c| c.get());
            let prev = (prev_n == tiles.len() && prev_c > 0).then_some(prev_c);
            let result = fit_grid(tiles.len(), area.0, area.1, prev);
            LAST_PIP_COLS.with(|c| c.set((tiles.len(), result.1)));
            result
        };
        let rows: Vec<AnyElement> = tiles
            .chunks(columns.max(1))
            .map(|row| {
                let drawn: Vec<AnyElement> = row
                    .iter()
                    .map(|t| draw(t, Size::Fixed(fitted.0, fitted.1), false, store, palette, cx))
                    .collect();
                h_flex().gap_2().justify_center().items_center().children(drawn).into_any_element()
            })
            .collect();
        return v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .gap_2()
            .justify_center()
            .items_center()
            .children(rows)
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
        .flex_1()
        .min_h_0()
        .w_full()
        .gap_2()
        .child(div().w_full().flex_1().min_h_0().child(big))
        .when(!others.is_empty(), |d| d.child(h_flex().flex_none().flex_wrap().gap_2().justify_center().children(others)))
        .into_any_element()
}

/// Drag payload for repositioning the floating mini call modal.
#[derive(Clone, Copy)]
pub struct DraggedCallPip;

/// Drag payload for resizing the floating mini call window from any edge or corner.
#[derive(Clone, Copy)]
pub struct ResizedCallPip(pub crate::call::PipResizeEdge);

/// Floating mini call window that snaps to any of the four viewport corners
/// (draggable from side to side and resizable from any edge or corner) whenever
/// you're in a live call (unless fullscreen or minimized). Shows video tiles
/// when anyone is on camera/screen, or participant avatar tiles with speaking
/// rings in voice-only calls.
pub fn floating(store: &Entity<Store>, palette: &Palette, window: &gpui::Window, cx: &mut App) -> Vec<AnyElement> {
    let state = store.read(cx);
    let Some(call) = state.call.clone().filter(|c| c.is_live() && !c.fullscreen && !c.pip_hidden && !c.popped_out) else {
        return Vec::new();
    };
    let room_id = call.room_id.clone();
    let room_name = state
        .rooms
        .iter()
        .find(|r| r.id == call.room_id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| call.room_id.clone());
    let connecting = call.status == CallStatus::Connecting;
    let expand = store.clone();
    let popout = store.clone();
    let hide = store.clone();
    let open_room = store.clone();
    let drag_start = store.clone();
    let drop_drag = store.clone();
    let drop_resize = store.clone();
    let zoom = crate::theme::zoom();
    let vp = window.viewport_size();
    let viewport = (f32::from(vp.width) / zoom, f32::from(vp.height) / zoom);
    let raw_size = call.pip_size.unwrap_or(crate::call::PIP_DEFAULT_SIZE);
    let raw_pos = call.pip_pos.unwrap_or_else(|| crate::call::pip_corner_pos(call.pip_corner, raw_size, viewport));
    let ((right, bottom), (pip_w, pip_h)) = crate::call::clamp_pip_rect(raw_pos, raw_size, viewport);

    // If a drag or resize ended outside the window (no drop event), snap back cleanly.
    if call.pip_pos.is_some() && !cx.has_active_drag() {
        let store = store.clone();
        let was_dragging = call.is_pip_dragging();
        cx.defer(move |cx| {
            store.update(cx, |s, cx| {
                if was_dragging {
                    s.end_call_pip_drag(viewport, cx);
                } else {
                    s.end_call_pip_resize(cx);
                }
            });
        });
    }

    // Border (2) + header + body padding + controls + gaps (+ connecting banner if shown).
    let chrome_h = if connecting { 130. } else { 106. };
    let stage_area = ((pip_w - 24.).max(80.), (pip_h - chrome_h).max(60.));

    let resize_handle = |id: &'static str, edge: crate::call::PipResizeEdge, cursor: gpui::CursorStyle, store: Entity<Store>| {
        div()
            .id(id)
            .absolute()
            .cursor(cursor)
            .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
                cx.stop_propagation();
                let z = crate::theme::zoom();
                let pos = (f32::from(event.position.x) / z, f32::from(event.position.y) / z);
                let vp = window.viewport_size();
                let viewport = (f32::from(vp.width) / z, f32::from(vp.height) / z);
                store.update(cx, |s, _| s.begin_call_pip_resize(pos, viewport));
            })
            .on_drag(ResizedCallPip(edge), |_, _, _, cx| cx.new(|_| gpui::EmptyView))
    };

    let t_handle = resize_handle("call-pip-resize-t", crate::call::PipResizeEdge::Top, gpui::CursorStyle::ResizeUpDown, store.clone())
        .top_0()
        .left(zpx(12.))
        .right(zpx(12.))
        .h(zpx(6.));
    let b_handle = resize_handle("call-pip-resize-b", crate::call::PipResizeEdge::Bottom, gpui::CursorStyle::ResizeUpDown, store.clone())
        .bottom_0()
        .left(zpx(12.))
        .right(zpx(12.))
        .h(zpx(6.));
    let l_handle = resize_handle("call-pip-resize-l", crate::call::PipResizeEdge::Left, gpui::CursorStyle::ResizeLeftRight, store.clone())
        .left_0()
        .top(zpx(12.))
        .bottom(zpx(12.))
        .w(zpx(6.));
    let r_handle = resize_handle("call-pip-resize-r", crate::call::PipResizeEdge::Right, gpui::CursorStyle::ResizeLeftRight, store.clone())
        .right_0()
        .top(zpx(12.))
        .bottom(zpx(12.))
        .w(zpx(6.));

    let grip_color = palette.text_tertiary;
    let docked = call.pip_corner;
    let tl_handle = resize_handle("call-pip-resize-tl", crate::call::PipResizeEdge::TopLeft, gpui::CursorStyle::ResizeUpLeftDownRight, store.clone())
        .top_0()
        .left_0()
        .size(zpx(14.))
        .when(docked == crate::call::PipCorner::BottomRight, |d| {
            d.flex()
                .items_start()
                .justify_start()
                .p(zpx(4.))
                .child(div().size(zpx(6.)).rounded_tl(corner(2.)).border_l_2().border_t_2().border_color(grip_color).opacity(0.55))
        });
    let tr_handle = resize_handle("call-pip-resize-tr", crate::call::PipResizeEdge::TopRight, gpui::CursorStyle::ResizeUpRightDownLeft, store.clone())
        .top_0()
        .right_0()
        .size(zpx(12.))
        .when(docked == crate::call::PipCorner::BottomLeft, |d| {
            d.flex()
                .items_start()
                .justify_end()
                .p(zpx(4.))
                .child(div().size(zpx(6.)).rounded_tr(corner(2.)).border_r_2().border_t_2().border_color(grip_color).opacity(0.55))
        });
    let bl_handle = resize_handle("call-pip-resize-bl", crate::call::PipResizeEdge::BottomLeft, gpui::CursorStyle::ResizeUpRightDownLeft, store.clone())
        .bottom_0()
        .left_0()
        .size(zpx(14.))
        .when(docked == crate::call::PipCorner::TopRight, |d| {
            d.flex()
                .items_end()
                .justify_start()
                .p(zpx(4.))
                .child(div().size(zpx(6.)).rounded_bl(corner(2.)).border_l_2().border_b_2().border_color(grip_color).opacity(0.55))
        });
    let br_handle = resize_handle("call-pip-resize-br", crate::call::PipResizeEdge::BottomRight, gpui::CursorStyle::ResizeUpLeftDownRight, store.clone())
        .bottom_0()
        .right_0()
        .size(zpx(14.))
        .when(docked == crate::call::PipCorner::TopLeft, |d| {
            d.flex()
                .items_end()
                .justify_end()
                .p(zpx(4.))
                .child(div().size(zpx(6.)).rounded_br(corner(2.)).border_r_2().border_b_2().border_color(grip_color).opacity(0.55))
        });

    let mut out = Vec::with_capacity(2);
    if call.is_pip_dragging() {
        let target = crate::call::nearest_pip_corner((right, bottom), (pip_w, pip_h), viewport);
        let (tr, tb) = crate::call::pip_corner_pos(target, (pip_w, pip_h), viewport);
        out.push(
            div()
                .absolute()
                .bottom(zpx(tb))
                .right(zpx(tr))
                .w(zpx(pip_w))
                .h(zpx(pip_h))
                .rounded(corner(16.))
                .bg(Hsla { a: 0.08, ..palette.accent })
                .border_2()
                .border_color(Hsla { a: 0.45, ..palette.accent })
                .into_any_element(),
        );
    }

    out.push(
        v_flex()
            .id("call-floating")
            .absolute()
            .bottom(zpx(bottom))
            .right(zpx(right))
            .w(zpx(pip_w))
            .h(zpx(pip_h))
            .occlude()
            .on_drop(move |_: &DraggedCallPip, window, cx| {
                let z = crate::theme::zoom();
                let vp = window.viewport_size();
                let viewport = (f32::from(vp.width) / z, f32::from(vp.height) / z);
                drop_drag.update(cx, |s, cx| s.end_call_pip_drag(viewport, cx));
            })
            .on_drop(move |_: &ResizedCallPip, _, cx| {
                drop_resize.update(cx, |s, cx| s.end_call_pip_resize(cx));
            })
            .rounded(corner(16.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.border)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .id("call-floating-header")
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(space(8.))
                    .px(space(12.))
                    .py(space(8.))
                    .border_b_1()
                    .border_color(palette.border_subtle)
                    .cursor_grab()
                    .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
                        let z = crate::theme::zoom();
                        let pos = (f32::from(event.position.x) / z, f32::from(event.position.y) / z);
                        let vp = window.viewport_size();
                        let viewport = (f32::from(vp.width) / z, f32::from(vp.height) / z);
                        drag_start.update(cx, |s, _| s.begin_call_pip_drag(pos, viewport));
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
                                Button::new("call-popout")
                                    .ghost()
                                    .small()
                                    .icon(gpui_component::Icon::new(gpui_component::IconName::ExternalLink))
                                    .tooltip("pop out into separate window")
                                    .on_click(move |_, _, cx| open_popout_window(popout.clone(), cx)),
                            )
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
                    .flex_1()
                    .min_h_0()
                    .p(space(10.))
                    .gap(space(8.))
                    .when(connecting, |col| col.child(div().flex_none().text_xs().text_color(palette.text_tertiary).child("joining the call…")))
                    .child(stage(&call, store, palette, false, stage_area, cx))
                    .child(h_flex().flex_none().justify_center().child(controls(store, &call, "float"))),
            )
            .child(t_handle)
            .child(b_handle)
            .child(l_handle)
            .child(r_handle)
            .child(tl_handle)
            .child(tr_handle)
            .child(bl_handle)
            .child(br_handle)
            .into_any_element(),
    );
    out
}

/// A standalone OS window for the active call (ideal for dual-monitor setups).
pub struct CallPopoutWindow {
    store: Entity<Store>,
}

impl CallPopoutWindow {
    fn new(store: Entity<Store>, cx: &mut gpui::Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.on_release(|this: &mut CallPopoutWindow, cx| {
            this.store.update(cx, |s, cx| s.set_call_popped_out(false, cx));
        })
        .detach();
        Self { store }
    }
}

impl gpui::Render for CallPopoutWindow {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let state = self.store.read(cx);
        let Some(call) = state.call.clone().filter(|c| c.is_live() && c.popped_out) else {
            window.remove_window();
            return div().into_any_element();
        };
        let room_name = state
            .rooms
            .iter()
            .find(|r| r.id == call.room_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| call.room_id.clone());
        let (zoom, viewport) = (crate::theme::zoom(), window.viewport_size());
        let area = ((f32::from(viewport.width) / zoom - 32.).max(120.), (f32::from(viewport.height) / zoom - 32. - 40. - 44. - 24.).max(120.));
        let dock_back = self.store.clone();
        if any_camera(&self.store, cx) {
            window.request_animation_frame();
        }
        v_flex()
            .id("call-popout-window")
            .relative()
            .size_full()
            .bg(palette.app)
            .text_color(palette.text)
            .font_family(palette.body_font.clone())
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .pl(if cfg!(target_os = "macos") { zpx(64.) } else { zpx(0.) })
                    .items_center()
                    .child(icon("icons/phone-call.svg").size(zpx(16.)).text_color(palette.online))
                    .child(div().flex_1().truncate().font_family(palette.heading_font.clone()).text_lg().font_weight(FontWeight::SEMIBOLD).child(room_name))
                    .child(
                        Button::new("call-popout-dock")
                            .ghost()
                            .icon(icon("icons/minimize-2.svg"))
                            .tooltip("dock back into main window")
                            .on_click(move |_, window, cx| {
                                dock_back.update(cx, |s, cx| s.set_call_popped_out(false, cx));
                                window.remove_window();
                            }),
                    ),
            )
            .child(stage(&call, &self.store, &palette, true, area, cx))
            .child(h_flex().flex_none().justify_center().child(controls(&self.store, &call, "popout")))
            .children(mixer(&self.store, &palette, window, cx))
            .children(participant_menu(&self.store, &palette, window, cx))
            .into_any_element()
    }
}

/// Open the active call in a separate OS window.
pub fn open_popout_window(store: Entity<Store>, cx: &mut App) {
    let room_name = {
        let s = store.read(cx);
        let Some(call) = s.call.as_ref().filter(|c| c.is_live()) else { return };
        if call.popped_out {
            return;
        }
        s.rooms.iter().find(|r| r.id == call.room_id).map(|r| r.name.clone()).unwrap_or_else(|| "call".into())
    };
    store.update(cx, |s, cx| s.set_call_popped_out(true, cx));
    let bounds = gpui::Bounds::centered(None, gpui::size(gpui::px(640.), gpui::px(460.)), cx);
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(format!("horo — {room_name}").into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(gpui::px(14.), gpui::px(14.))),
        }),
        window_min_size: Some(gpui::size(gpui::px(360.), gpui::px(280.))),
        app_id: Some("horo".into()),
        ..Default::default()
    };
    let s_for_win = store.clone();
    if cx
        .open_window(options, move |window, cx| {
            let view = cx.new(|cx| CallPopoutWindow::new(s_for_win, cx));
            cx.new(|cx| gpui_component::Root::new(view, window, cx))
        })
        .is_err()
    {
        store.update(cx, |s, cx| s.set_call_popped_out(false, cx));
    }
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

/// Right-click context menu for a remote call participant: smooth draggable
/// volume slider (`0%–200%`), local mute, reset, full mixer shortcut, and profile.
pub fn participant_menu(store: &Entity<Store>, palette: &Palette, window: &mut gpui::Window, cx: &mut App) -> Option<AnyElement> {
    let target = store.read(cx).call_participant_menu.clone()?;
    let call = store.read(cx).call.clone().filter(|c| c.is_live())?;
    let vol = call.participant_volumes.get(&target.identity).copied().unwrap_or(1.0);
    let pct = (vol * 100.0).round() as u32;
    let muted = vol <= 0.001;
    let slider = store.update(cx, |s, cx| s.call_volume_slider(&target.identity, cx));
    if (slider.read(cx).value().start() - vol).abs() > 0.005 {
        slider.update(cx, |s, cx| s.set_value(vol, window, cx));
    }

    let face = avatar(&target.user_id, &target.name, target.avatar.as_deref(), zpx(22.), cx);
    let (close_out, s_mute, s_reset, s_mixer, s_profile) = (store.clone(), store.clone(), store.clone(), store.clone(), store.clone());
    let (id_mute, id_reset, uid_profile) = (target.identity.clone(), target.identity.clone(), target.user_id.clone());
    let at = target.at;

    let card = menu::frame("call-participant-menu", palette)
        .w(zpx(248.))
        .on_mouse_down_out(move |_, _, cx| close_out.update(cx, |s, cx| s.close_participant_menu(cx)))
        .child(
            v_flex()
                .gap(zpx(8.))
                .px(space(10.))
                .pt(space(8.))
                .pb(space(6.))
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
                                .gap(space(7.))
                                .child(face)
                                .child(div().truncate().text_xs().font_weight(FontWeight::BOLD).text_color(palette.text).child(target.name)),
                        )
                        .child(
                            div()
                                .font_family(palette.mono_font.clone())
                                .text_size(text(11.))
                                .text_color(if muted {
                                    palette.danger
                                } else if pct > 100 {
                                    palette.idle
                                } else {
                                    palette.text_secondary
                                })
                                .child(if muted { "muted".to_owned() } else { format!("{pct}%") }),
                        ),
                )
                .child(
                    v_flex()
                        .gap(zpx(2.))
                        .child(Slider::new(&slider))
                        .child(
                            h_flex()
                                .justify_between()
                                .text_size(text(9.5))
                                .text_color(palette.text_tertiary)
                                .child("0%")
                                .child("100%")
                                .child("200%"),
                        ),
                ),
        )
        .child(menu::separator(palette))
        .child(
            menu::item("pmenu-mute", "mute for me", true, Some(muted), palette).on_click(move |_, _, cx| {
                let id = id_mute.clone();
                s_mute.update(cx, |s, cx| s.set_participant_volume(id, if muted { 1.0 } else { 0.0 }, cx));
            }),
        )
        .when(pct != 100, |d| {
            d.child(
                menu::item("pmenu-reset", "reset volume to 100%", true, Some(false), palette).on_click(move |_, _, cx| {
                    let id = id_reset.clone();
                    s_reset.update(cx, |s, cx| s.set_participant_volume(id, 1.0, cx));
                }),
            )
        })
        .child(menu::separator(palette))
        .child(
            menu::item("pmenu-mixer", "open volume mixer…", true, None, palette).on_click(move |_, _, cx| {
                s_mixer.update(cx, |s, cx| s.open_call_mixer(cx));
            }),
        )
        .child(
            menu::item("pmenu-profile", "view profile", true, None, palette).on_click(move |_, _, cx| {
                let uid = uid_profile.clone();
                s_profile.update(cx, |s, cx| {
                    s.close_participant_menu(cx);
                    s.open_profile(uid, at, None, cx);
                });
            }),
        );

    Some(menu::place(at, card))
}

/// Floating per-user volume mixer popover (`0%–200%` draggable slider + local mute per speaker),
/// accessible from the call controls or any participant's right-click context menu.
pub fn mixer(store: &Entity<Store>, palette: &Palette, window: &mut gpui::Window, cx: &mut App) -> Option<AnyElement> {
    let (call, profiles) = {
        let state = store.read(cx);
        let call = state.call.clone().filter(|c| c.is_live() && c.mixer_open)?;
        (call, state.profiles.clone())
    };
    let own_identity = call.identity.clone();
    let list = people(&call, &profiles);
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
                let slider = store.update(cx, |s, cx| s.call_volume_slider(&id, cx));
                if (slider.read(cx).value().start() - vol).abs() > 0.005 {
                    slider.update(cx, |s, cx| s.set_value(vol, window, cx));
                }
                let face = avatar(&p.user_id, &p.real_name, p.avatar.as_deref(), zpx(26.), cx);
                let s_mute = store.clone();
                let id_mute = id.clone();

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
                        v_flex()
                            .gap(zpx(2.))
                            .child(Slider::new(&slider))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .text_size(text(9.5))
                                    .text_color(palette.text_tertiary)
                                    .child("0%")
                                    .child("100%")
                                    .child("200%"),
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
        row.child(
            Button::new(SharedString::from(format!("{prefix}-call-screen")))
                .ghost()
                .icon(icon(if sharing { "icons/screen-share-off.svg" } else { "icons/screen-share.svg" }))
                .tooltip(if sharing { "stop sharing your screen" } else { "share your screen" })
                .when(sharing, |b| b.primary())
                .on_click(move |_, _, cx| share.update(cx, |s, cx| s.set_call_screen(!sharing, cx))),
        )
    };
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let row = {
        let (camera, camera_on) = (store.clone(), call.camera);
        row.child(
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

/// Someone's camera (or screen, under its key) on Linux & Windows: a gpui
/// image of the newest picture, made once per picture.
#[cfg(not(target_os = "macos"))]
fn camera_picture(screens: &horo_rtc::call::Screens, identity: &str, fit: gpui::ObjectFit, cx: &mut App) -> Option<AnyElement> {
    use gpui::StyledImage as _;
    let Some(picture) = screens.get(identity) else {
        crate::picture::forget(identity, cx);
        return None;
    };
    let image = crate::picture::image(identity, picture.seq, &picture.frame, false, cx)?;
    Some(gpui::img(image).size_full().object_fit(fit).into_any_element())
}

/// What our camera sees now, on Linux & Windows.
#[cfg(not(target_os = "macos"))]
fn own_camera(store: &Entity<Store>, fit: gpui::ObjectFit, cx: &mut App) -> Option<AnyElement> {
    use gpui::StyledImage as _;
    let (seq, frame) = store.read(cx).call_live.as_ref()?.camera.as_ref()?.latest()?;
    let image = crate::picture::image(crate::call::OWN_CAMERA, seq, &frame, true, cx)?;
    Some(gpui::img(image).size_full().object_fit(fit).into_any_element())
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
    use super::{fit, fit_grid};

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

    #[test]
    fn pip_grid_hysteresis_avoids_flickering_at_boundary() {
        // Near the 1-col vs 2-col boundary for 2 tiles, keeping prev_cols=2
        // prevents flipping to 1 column on tiny height changes.
        let (_, cols) = fit_grid(2, 330., 196., Some(2));
        assert_eq!(cols, 2);
        // Once the window is decisively taller, it switches cleanly to 1 column.
        let (_, cols) = fit_grid(2, 330., 240., Some(2));
        assert_eq!(cols, 1);
    }
}
