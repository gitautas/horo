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
    prelude::FluentBuilder, px,
};
use gpui_component::{
    Icon,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::dto::RoomSummary;

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

    let (open, mute, leave) = (store.clone(), store.clone(), store.clone());
    #[cfg(target_os = "macos")]
    let camera_button = {
        let (toggle, on) = (store.clone(), call.camera);
        Button::new("call-camera")
            .ghost()
            .icon(icon(if on { "icons/video.svg" } else { "icons/video-off.svg" }))
            .tooltip(if on { "turn your camera off" } else { "turn your camera on" })
            .when(on, |b| b.primary())
            .on_click(move |_, _, cx| toggle.update(cx, |s, cx| s.set_call_camera(!on, cx)))
    };
    #[cfg(target_os = "macos")]
    let share_button = {
        let (toggle, on) = (store.clone(), call.sharing);
        Button::new("call-screen")
            .ghost()
            .icon(icon(if on { "icons/screen-share-off.svg" } else { "icons/screen-share.svg" }))
            .tooltip(if on { "stop sharing your screen" } else { "share your screen" })
            .when(on, |b| b.primary())
            .on_click(move |_, _, cx| toggle.update(cx, |s, cx| s.set_call_screen(!on, cx)))
    };
    let room_id = call.room_id.clone();
    let controls = if failed {
        h_flex().child(
            Button::new("call-dismiss")
                .ghost()
                .label("dismiss")
                .on_click(move |_, _, cx| leave.update(cx, |s, cx| s.clear_failed_call(cx))),
        )
    } else {
        let muted = call.muted;
        let row = h_flex().gap_1();
        #[cfg(target_os = "macos")]
        let row = row.child(share_button).child(camera_button);
        row.child(
                Button::new("call-mute")
                    .ghost()
                    .icon(icon(if muted { "icons/mic-off.svg" } else { "icons/mic.svg" }))
                    .tooltip(if muted { "unmute" } else { "mute" })
                    .when(muted, |b| b.danger())
                    .on_click(move |_, _, cx| mute.update(cx, |s, cx| s.set_call_muted(!muted, cx))),
            )
            .child(
                Button::new("call-leave")
                    .danger()
                    .icon(icon("icons/phone-off.svg"))
                    .tooltip("leave the call")
                    .on_click(move |_, _, cx| leave.update(cx, |s, cx| s.leave_call(cx))),
            )
    };

    Some(
        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(palette.border_subtle)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(h_flex().gap_2().child(div().flex_none().size(px(8.)).rounded_full().bg(colour)).child(div().truncate().text_sm().text_color(colour).child(status)))
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
            .child(controls)
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
        let face = avatar(&person.user_id, &person.real_name, person.avatar.as_deref(), px(20.), cx);
        rows.push(
            h_flex()
                .gap_2()
                .pl(px(40.))
                .py(px(2.))
                .child(div().flex_none().rounded_full().border_2().border_color(if person.speaking { palette.online } else { gpui::transparent_black() }).child(face))
                .child(div().truncate().text_xs().text_color(if person.speaking { palette.text } else { palette.text_secondary }).child(person.name))
                .when(person.muted, |row| row.child(icon("icons/mic-off.svg").size(px(12.)).text_color(palette.text_tertiary))),
        );
    }
    Some(v_flex().pb_1().children(rows).into_any_element())
}

/// The call's people, above the timeline of the call's room.
pub fn strip(room_id: &str, store: &Entity<Store>, palette: &Palette, cx: &mut App) -> Option<AnyElement> {
    let call = store.read(cx).call.clone().filter(|c| c.room_id == room_id && c.is_live())?;
    let connecting = call.status == CallStatus::Connecting;
    let mut tiles = Vec::new();
    let everyone = people(&call, &store.read(cx).profiles);
    want(store, &everyone, cx);
    let screens = store.read(cx).call_live.as_ref().map(|l| l.screens.clone());
    let me = call.identity.clone();
    for person in everyone {
        // A shared screen: its own, bigger tile, before the person's.
        if person.screen
            && let Some(picture) = person.identity.as_deref().and_then(|id| camera_picture(screens.as_ref()?, &horo_rtc::call::screen_key(id), SCREEN_W, SCREEN_H))
        {
            tiles.push(screen_tile(picture, &format!("{}'s screen", person.real_name), &person, palette).into_any_element());
        }
        if person.identity.is_some() && person.identity == me && call.sharing {
            tiles.push(sharing_tile(palette).into_any_element());
        }
        // Our own camera, straight from it rather than round the server.
        if call.camera
            && person.identity.is_some()
            && person.identity == me
            && let Some(picture) = own_camera(store, cx)
        {
            tiles.push(video_tile(picture, &person, palette).into_any_element());
            continue;
        }
        if person.camera
            && let Some(picture) = person.identity.as_deref().and_then(|id| camera_picture(screens.as_ref()?, id, VIDEO_W, VIDEO_H))
        {
            tiles.push(video_tile(picture, &person, palette).into_any_element());
            continue;
        }
        let face = avatar(&person.user_id, &person.real_name, person.avatar.as_deref(), px(56.), cx);
        tiles.push(
            v_flex()
                .w(px(96.))
                .py_2()
                .gap_1()
                .items_center()
                .rounded(px(16.))
                .bg(palette.card)
                .child(
                    div()
                        .rounded_full()
                        .p(px(2.))
                        .border_2()
                        .border_color(if person.speaking { palette.online } else { gpui::transparent_black() })
                        .child(face),
                )
                .child(h_flex().gap_1().max_w(px(88.)).child(div().truncate().text_xs().child(person.name)).when(person.muted, |row| {
                    row.child(icon("icons/mic-off.svg").size(px(12.)).text_color(palette.danger))
                }))
                .into_any_element(),
        );
    }
    Some(
        h_flex()
            .flex_wrap()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(palette.border_subtle)
            .when(connecting, |strip| strip.child(div().text_sm().text_color(palette.text_tertiary).child("joining the call…")))
            .children(tiles)
            .into_any_element(),
    )
}

/// How big a camera's tile is: 16:9.
const VIDEO_W: f32 = 240.;
const VIDEO_H: f32 = 135.;

/// Someone's camera, as an element: on macOS a surface over the newest
/// picture (converted once per picture, not once per redraw).
/// How big a shared screen's tile is.
const SCREEN_W: f32 = 480.;
const SCREEN_H: f32 = 270.;

/// A tile with someone's shared screen in it.
fn screen_tile(picture: AnyElement, label: &str, person: &Person, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::SharedString::from(format!("screen-{}", person.identity.as_deref().unwrap_or(&person.user_id))))
        .relative()
        .w(px(SCREEN_W))
        .h(px(SCREEN_H))
        .rounded(px(16.))
        .overflow_hidden()
        .bg(Hsla::black())
        .child(picture)
        .child(
            div()
                .absolute()
                .left(px(6.))
                .bottom(px(6.))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(8.))
                .bg(Hsla { a: 0.65, ..palette.app })
                .max_w(px(SCREEN_W - 12.))
                .truncate()
                .text_xs()
                .child(label.to_owned()),
        )
}

/// While we share: a quiet tile saying so (our own screen echoed back at
/// us would only be a hall of mirrors).
fn sharing_tile(palette: &Palette) -> gpui::Stateful<gpui::Div> {
    v_flex()
        .id("sharing-you")
        .w(px(VIDEO_W))
        .h(px(VIDEO_H))
        .gap_2()
        .items_center()
        .justify_center()
        .rounded(px(16.))
        .bg(palette.card)
        .border_1()
        .border_color(palette.accent)
        .child(icon("icons/screen-share.svg").size(px(28.)).text_color(palette.accent))
        .child(div().text_xs().text_color(palette.text_secondary).child("you're sharing your screen"))
}

#[cfg(target_os = "macos")]
fn camera_picture(screens: &horo_rtc::call::Screens, identity: &str, width: f32, height: f32) -> Option<AnyElement> {
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
    // A screen is shown whole (its edges matter); a face can be cropped.
    let fit = if width > VIDEO_W { gpui::ObjectFit::Contain } else { gpui::ObjectFit::Cover };
    Some(gpui::surface(buffer).w(px(width)).h(px(height)).object_fit(fit).into_any_element())
}

/// What our camera sees now.
#[cfg(target_os = "macos")]
fn own_camera(store: &Entity<Store>, cx: &mut App) -> Option<AnyElement> {
    let buffer = store.update(cx, |s, _| s.call_live.as_mut()?.camera.as_mut()?.mirrored_frame())?;
    Some(gpui::surface(buffer).w(px(VIDEO_W)).h(px(VIDEO_H)).object_fit(gpui::ObjectFit::Cover).into_any_element())
}

#[cfg(not(target_os = "macos"))]
fn own_camera(_store: &Entity<Store>, _cx: &mut App) -> Option<AnyElement> {
    None
}

#[cfg(not(target_os = "macos"))]
fn camera_picture(_screens: &horo_rtc::call::Screens, _identity: &str, _width: f32, _height: f32) -> Option<AnyElement> {
    None
}

/// A tile with someone's camera in it, their name over the corner.
fn video_tile(picture: AnyElement, person: &Person, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::SharedString::from(format!("video-{}", person.identity.as_deref().unwrap_or(&person.user_id))))
        .relative()
        .w(px(VIDEO_W))
        .h(px(VIDEO_H))
        .rounded(px(16.))
        .overflow_hidden()
        .bg(palette.inset)
        .border_2()
        .border_color(if person.speaking { palette.online } else { gpui::transparent_black() })
        .child(picture)
        .child(
            h_flex()
                .absolute()
                .left(px(6.))
                .bottom(px(6.))
                .gap_1()
                .px(px(6.))
                .py(px(2.))
                .rounded(px(8.))
                .bg(Hsla { a: 0.65, ..palette.app })
                .max_w(px(VIDEO_W - 12.))
                .child(div().truncate().text_xs().child(person.name.clone()))
                .when(person.muted, |row| row.child(icon("icons/mic-off.svg").size(px(12.)).text_color(palette.danger))),
        )
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
    let picture = avatar(&incoming.room_id, &name, room.as_ref().and_then(|r| r.avatar_url.as_deref()), px(72.), cx);
    let (answer, decline) = (store.clone(), store.clone());
    Some(
        scrim(palette)
            .child(
                v_flex()
                    .w(px(320.))
                    .p_6()
                    .gap_4()
                    .items_center()
                    .rounded(px(24.))
                    .bg(palette.card)
                    .border_1()
                    .border_color(Hsla { a: 0.5, ..palette.online })
                    .shadow_lg()
                    .child(picture)
                    .child(div().font_family(palette.heading_font.clone()).text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).child(name))
                    .child(div().text_sm().text_color(palette.text_secondary).child("is calling you~"))
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
