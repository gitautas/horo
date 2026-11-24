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
    AnyElement, App, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder,
};
use gpui_component::{
    Sizable,
    Icon,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::dto::RoomSummary;
use horo_rtc::call::screen_key;

use crate::theme::zpx;
use crate::{
    call::{CallStatus, CallView, user_of},
    store::{Known, Store},
    theme::Palette,
    views::{avatar::avatar, verification::scrim},
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
        add(&p.identity, &p.name, false, false, p.camera, p.screen);
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

    let (open, leave, expand) = (store.clone(), store.clone(), store.clone());
    let room_id = call.room_id.clone();
    let corner = if failed {
        Button::new("call-dismiss").ghost().label("dismiss").on_click(move |_, _, cx| leave.update(cx, |s, cx| s.clear_failed_call(cx)))
    } else {
        Button::new("call-expand-panel")
            .ghost()
            .small()
            .icon(icon("icons/maximize-2.svg"))
            .tooltip("fill the window with the call")
            .on_click(move |_, _, cx| expand.update(cx, |s, cx| s.set_call_fullscreen(true, cx)))
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
                    .child(h_flex().gap_2().child(div().flex_none().size(zpx(8.)).rounded_full().bg(colour)).child(div().truncate().text_sm().text_color(colour).child(status)))
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
            .when(!failed, |d| d.child(controls(store, &call)))
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
    want(store, &people, cx);
    if people.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for person in people {
        let face = avatar(&person.user_id, &person.real_name, person.avatar.as_deref(), zpx(20.), cx);
        rows.push(
            h_flex()
                .gap_2()
                .pl(zpx(40.))
                .py(zpx(2.))
                .child(div().flex_none().rounded_full().border_2().border_color(if person.speaking { palette.online } else { gpui::transparent_black() }).child(face))
                .child(div().truncate().text_xs().text_color(if person.speaking { palette.text } else { palette.text_secondary }).child(person.name))
                .when(person.muted, |row| row.child(icon("icons/mic-off.svg").size(zpx(12.)).text_color(palette.text_tertiary))),
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

/// A tile's own size in the grid: screens big, cameras 16:9, faces small.
fn natural(shows: &Shows) -> (f32, f32) {
    match shows {
        Shows::Screen { .. } => (480., 270.),
        Shows::Sharing | Shows::OwnCamera | Shows::Camera { .. } => (240., 135.),
        Shows::Face => (96., 104.),
    }
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
        Shows::Screen { identity } => screens.as_ref().and_then(|s| camera_picture(s, &screen_key(identity), fit)),
        Shows::Camera { identity } => screens.as_ref().and_then(|s| camera_picture(s, identity, fit)),
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
            let key = (!focused).then(|| key.clone());
            click.update(cx, |s, cx| s.set_call_focus(key, cx));
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
    base.bg(palette.card)
        .child(
            v_flex()
                .size_full()
                .gap_1()
                .px_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .rounded_full()
                        .p(zpx(2.))
                        .border_2()
                        .border_color(if person.speaking { palette.online } else { gpui::transparent_black() })
                        .child(face),
                )
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
fn stage(call: &CallView, store: &Entity<Store>, palette: &Palette, full: bool, cx: &mut App) -> AnyElement {
    let tiles = tiles(call, store, cx);
    let focused = call.focus.as_ref().and_then(|key| tiles.iter().position(|t| &t.key == key));
    let Some(focused) = focused else {
        let scale = if full { 1.5 } else { 1. };
        let drawn: Vec<AnyElement> = tiles
            .iter()
            .map(|t| {
                let (w, h) = natural(&t.shows);
                draw(t, Size::Fixed(w * scale, h * scale), false, store, palette, cx)
            })
            .collect();
        return h_flex()
            .flex_wrap()
            .gap_2()
            .when(full, |d| d.flex_1().min_h_0().justify_center().content_center())
            .children(drawn)
            .into_any_element();
    };
    let big = draw(&tiles[focused], Size::Fill, true, store, palette, cx);
    let others: Vec<AnyElement> = tiles
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != focused)
        .map(|(_, t)| draw(t, Size::Fixed(FILM.0, FILM.1), false, store, palette, cx))
        .collect();
    v_flex()
        .gap_2()
        .when(full, |d| d.flex_1().min_h_0())
        .child(div().w_full().map(|d| if full { d.flex_1().min_h_0() } else { d.h(zpx(420.)) }).child(big))
        .when(!others.is_empty(), |d| d.child(h_flex().flex_wrap().gap_2().when(full, |d| d.justify_center()).children(others)))
        .into_any_element()
}

/// The call's people, above the timeline of the call's room (unless the
/// call fills the window).
pub fn strip(room_id: &str, store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let call = store.read(cx).call.clone().filter(|c| c.room_id == room_id && c.is_live() && !c.fullscreen)?;
    let connecting = call.status == CallStatus::Connecting;
    let expand = store.clone();
    Some(
        div()
            .relative()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(palette.border_subtle)
            .when(connecting, |strip| strip.child(div().pb_2().text_sm().text_color(palette.text_tertiary).child("joining the call…")))
            .child(stage(&call, store, palette, false, cx))
            .child(
                div().absolute().top(zpx(14.)).right(zpx(18.)).child(
                    Button::new("call-expand")
                        .ghost()
                        .small()
                        .icon(icon("icons/maximize-2.svg"))
                        .tooltip("fill the window")
                        .on_click(move |_, _, cx| expand.update(cx, |s, cx| s.set_call_fullscreen(true, cx))),
                ),
            )
            .into_any_element(),
    )
}

/// The call filling the window: its tiles as big as they'll go, and its
/// controls under them. Escape (or the button) puts it back.
pub fn fullscreen(store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let state = store.read(cx);
    let call = state.call.clone().filter(|c| c.fullscreen && c.is_live())?;
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
            .child(stage(&call, store, palette, true, cx))
            .child(h_flex().justify_center().child(controls(store, &call)))
            .into_any_element(),
    )
}

/// Our camera, screen, microphone, hearing and the way out.
fn controls(store: &Entity<Store>, call: &CallView) -> gpui::Div {
    let row = h_flex().gap_1();
    #[cfg(target_os = "macos")]
    let row = {
        let (share, sharing) = (store.clone(), call.sharing);
        let (camera, camera_on) = (store.clone(), call.camera);
        row.child(
            Button::new("call-screen")
                .ghost()
                .icon(icon(if sharing { "icons/screen-share-off.svg" } else { "icons/screen-share.svg" }))
                .tooltip(if sharing { "stop sharing your screen" } else { "share your screen" })
                .when(sharing, |b| b.primary())
                .on_click(move |_, _, cx| share.update(cx, |s, cx| s.set_call_screen(!sharing, cx))),
        )
        .child(
            Button::new("call-camera")
                .ghost()
                .icon(icon(if camera_on { "icons/video.svg" } else { "icons/video-off.svg" }))
                .tooltip(if camera_on { "turn your camera off" } else { "turn your camera on" })
                .when(camera_on, |b| b.primary())
                .on_click(move |_, _, cx| camera.update(cx, |s, cx| s.set_call_camera(!camera_on, cx))),
        )
    };
    let (mute, deafen, leave) = (store.clone(), store.clone(), store.clone());
    let (muted, deafened) = (call.muted, call.deafened);
    row.child(
        Button::new("call-mute")
            .ghost()
            .icon(icon(if muted { "icons/mic-off.svg" } else { "icons/mic.svg" }))
            .tooltip(if muted { "unmute" } else { "mute" })
            .when(muted, |b| b.danger())
            .on_click(move |_, _, cx| mute.update(cx, |s, cx| s.set_call_muted(!muted, cx))),
    )
    .child(
        Button::new("call-deafen")
            .ghost()
            .icon(icon(if deafened { "icons/headphone-off.svg" } else { "icons/headphones.svg" }))
            .tooltip(if deafened { "undeafen" } else { "deafen" })
            .when(deafened, |b| b.danger())
            .on_click(move |_, _, cx| deafen.update(cx, |s, cx| s.set_call_deafened(!deafened, cx))),
    )
    .child(
        Button::new("call-leave")
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
fn camera_picture(screens: &horo_rtc::call::Screens, identity: &str, fit: gpui::ObjectFit) -> Option<AnyElement> {
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

#[cfg(not(target_os = "macos"))]
fn own_camera(_store: &Entity<Store>, _fit: gpui::ObjectFit, _cx: &mut App) -> Option<AnyElement> {
    None
}

#[cfg(not(target_os = "macos"))]
fn camera_picture(_screens: &horo_rtc::call::Screens, _identity: &str, _fit: gpui::ObjectFit) -> Option<AnyElement> {
    None
}

/// Whether anyone in our call has a camera on (the window then redraws
/// every frame for them).
pub fn any_camera(store: &Entity<Store>, cx: &App) -> bool {
    store.read(cx).call.as_ref().is_some_and(|c| c.camera || c.participants.iter().any(|p| p.camera || p.screen))
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
