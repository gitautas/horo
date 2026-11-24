//! Leaving a space, as SpaceDialogs.tsx's LeaveSpace. A space is only a
//! folder of rooms, so leaving it alone takes nothing away: its rooms stay
//! in "everything". Taking them too is a choice, off by default, and only
//! ever covers rooms no other space of yours holds.

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Window,
    div,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants},
    h_flex,
    switch::Switch,
    v_flex,
};
use horo_core::matrix::rooms as core_rooms;
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    rooms,
    store::Store,
    theme::{Palette, heading, space, text},
    views::{explore::Closed, modal::modal},
};

pub struct LeaveSpace {
    store: Entity<Store>,
    space_id: String,
    focus: FocusHandle,
    with_rooms: bool,
    leaving: bool,
    error: Option<SharedString>,
}

impl EventEmitter<Closed> for LeaveSpace {}

impl LeaveSpace {
    pub fn new(store: Entity<Store>, space_id: String, window: &mut Window, cx: &mut Context<Self>) -> LeaveSpace {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        LeaveSpace { store, space_id, focus, with_rooms: false, leaving: false, error: None }
    }

    /// The rooms that would go with it, (id, name).
    fn own_rooms(&self, cx: &Context<Self>) -> Vec<(String, String)> {
        let store = self.store.read(cx);
        rooms::only_in_space(&store.rooms, &store.spaces, &self.space_id).into_iter().map(|r| (r.id.clone(), r.name.clone())).collect()
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.leaving {
            return;
        }
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let taking: Vec<String> = if self.with_rooms { self.own_rooms(cx).into_iter().map(|(id, _)| id).collect() } else { Vec::new() };
        self.leaving = true;
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, taking) = (backend.state.clone(), taking.clone());
            async move {
                let core = state.core().await?;
                core_rooms::leave(&core, &space).await?;
                // The space is gone now either way; a room that won't let
                // go is counted, not a reason to undo anything.
                let mut stuck = 0;
                for id in taking {
                    let ok = match OwnedRoomId::try_from(id.as_str()) {
                        Ok(room) => core_rooms::leave(&core, &room).await.is_ok(),
                        Err(_) => false,
                    };
                    if !ok {
                        stuck += 1;
                    }
                }
                Ok::<_, horo_core::Error>(stuck)
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(stuck) => {
                        if stuck > 0 {
                            let name = this.store.read(cx).spaces.iter().find(|s| s.id == this.space_id).map(|s| s.name.clone()).unwrap_or_default();
                            let (which, them) = if stuck == 1 { ("one room".to_owned(), "it") } else { (format!("{stuck} rooms"), "them") };
                            let message = format!("left {name}, but {which} wouldn't let go — try leaving {them} from room settings");
                            this.store.update(cx, |s, cx| s.show_banner(crate::store::Tone::Error, message, cx));
                        }
                        let space_id = this.space_id.clone();
                        this.store.update(cx, |s, cx| {
                            if s.space.as_deref() == Some(space_id.as_str()) {
                                s.open_space(None, cx);
                            }
                            for id in &taking {
                                s.close_room(id, cx);
                            }
                            s.refresh_spaces_now(cx);
                        });
                        cx.emit(Closed);
                    }
                    Err(e) => {
                        this.leaving = false;
                        this.error = Some(format!("couldn't leave: {e}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for LeaveSpace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette: Palette = cx.global::<Palette>().clone();
        let name = self.store.read(cx).spaces.iter().find(|s| s.id == self.space_id).map(|s| s.name.clone());
        let Some(name) = name else { return div().into_any_element() };
        let own = self.own_rooms(cx);
        let names: Vec<&str> = own.iter().map(|(_, n)| n.as_str()).collect();
        let with_rooms = (!own.is_empty()).then(|| {
            let which = if own.len() == 1 {
                format!("the 1 room you're in here: {}", own[0].1)
            } else {
                format!("the {} rooms you're in here: {}", own.len(), rooms::list_names(&names, 3))
            };
            h_flex()
                .items_start()
                .gap(space(12.))
                .mb(zpx(6.))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::BOLD).text_size(heading(13.5)).child("leave its rooms too"))
                        .child(
                            div()
                                .mt(zpx(2.))
                                .text_size(text(12.))
                                .line_height(gpui::relative(1.45))
                                .text_color(palette.text_tertiary)
                                .child(format!("{which}. rooms that are also in another of your spaces stay.")),
                        ),
                )
                .child(div().pt(space(2.)).child(Switch::new("leave-with-rooms").checked(self.with_rooms).on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.with_rooms = *on;
                    cx.notify();
                }))))
        });
        let leaving = self.leaving;
        let body = v_flex()
            .child(
                div()
                    .mb(zpx(16.))
                    .text_size(text(13.))
                    .line_height(gpui::relative(1.55))
                    .text_color(palette.text_secondary)
                    .child("it drops out of your rail. rooms you're in stay in \"everything\" unless you take them with you — and if the space is private, you'd need an invite to get back."),
            )
            .children(with_rooms)
            .children(self.error.clone().map(|e| div().mt(zpx(8.)).text_size(text(12.5)).text_color(palette.danger).child(e)))
            .child(
                h_flex()
                    .justify_end()
                    .gap(space(8.))
                    .mt(zpx(20.))
                    .child(Button::new("leave-space-stay").ghost().label("stay").on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))))
                    .child(
                        Button::new("leave-space-go")
                            .danger()
                            .label(if leaving { "leaving…" } else { "leave" })
                            .disabled(leaving)
                            .on_click(cx.listener(|this, _, _, cx| this.leave(cx))),
                    ),
            )
            .into_any_element();
        let this = cx.weak_entity();
        modal(
            "leave-space",
            format!("leave {name}?"),
            440.,
            &self.focus,
            &palette,
            move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(Closed));
            },
            body,
        )
    }
}
