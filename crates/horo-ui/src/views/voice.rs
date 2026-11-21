//! The call's pieces of the window, as the WebView build has them:
//!
//! - [`panel`]: "voice connected" at the foot of the room list, where
//!   Discord keeps it: status, the room (click to go there), mute, leave.
//! - [`roster`]: who's in a voice room, under it in the list; live from the
//!   call while we're in it, from the room's call membership otherwise.
//! - [`strip`]: everyone in the call, above the room's timeline while it's
//!   the call's room, with a ring on whoever is speaking.
//! - [`ringing`]: someone calling you, with answer and decline.

use gpui::{
    AnyElement, App, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
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
    store::Store,
    theme::Palette,
    views::{avatar::avatar, verification::scrim},
};

pub fn icon(path: &'static str) -> Icon {
    Icon::default().path(path)
}

/// Someone in a call, as listed.
struct Person {
    user_id: String,
    name: String,
    speaking: bool,
    /// Only known for ourselves.
    muted: bool,
}

fn localpart(user_id: &str) -> &str {
    user_id.trim_start_matches('@').split(':').next().unwrap_or(user_id)
}

/// Everyone in our call, ourselves first, one entry per person (several
/// devices of one person are one person).
fn people(call: &CallView) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::new();
    let mut add = |identity: &str, name: &str, muted: bool| {
        let user = user_of(identity).to_owned();
        if out.iter().any(|p| p.user_id == user) {
            return;
        }
        let name = if name.is_empty() || name == identity { localpart(&user).to_owned() } else { name.to_owned() };
        out.push(Person { speaking: call.is_speaking(&user), user_id: user, name, muted });
    };
    if let Some(me) = &call.identity {
        add(me, "you", call.muted);
    }
    for p in &call.participants {
        add(&p.identity, &p.name, false);
    }
    out
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
        h_flex()
            .gap_1()
            .child(
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
        Some(call) => people(call),
        None => room
            .call_participants
            .iter()
            .map(|user| Person { name: localpart(user).to_owned(), user_id: user.clone(), speaking: false, muted: false })
            .collect(),
    };
    if people.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for person in people {
        let face = avatar(&person.user_id, &person.name, None, px(20.), cx);
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
    for person in people(&call) {
        let face = avatar(&person.user_id, &person.name, None, px(56.), cx);
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
                })),
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

/// The room header's call button: join, or leave the call you're in here.
pub fn header_button(room: &RoomSummary, store: &Entity<Store>, cx: &App) -> AnyElement {
    let in_this = store.read(cx).call.as_ref().is_some_and(|c| c.room_id == room.id && c.is_live());
    let others = room.call_participants.len();
    let (store, room_id) = (store.clone(), room.id.clone());
    if in_this {
        return Button::new("header-leave")
            .danger()
            .icon(icon("icons/phone-off.svg"))
            .label("leave call")
            .on_click(move |_, _, cx| store.update(cx, |s, cx| s.leave_call(cx)))
            .into_any_element();
    }
    let label: SharedString = if room.has_active_call && others > 0 { format!("join call · {others}").into() } else { "call".into() };
    Button::new("header-call")
        .ghost()
        .icon(icon("icons/phone.svg"))
        .label(label)
        .on_click(move |_, _, cx| {
            let room_id = room_id.clone();
            store.update(cx, |s, cx| s.join_call(room_id, cx))
        })
        .into_any_element()
}
