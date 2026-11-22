//! Making a room, and joining one by address, as CreateRoom.tsx. Opened
//! from the `+` in the room list's header.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    h_flex,
    input::{Input, InputEvent, InputState},
    switch::Switch,
    v_flex,
};
use horo_core::matrix::rooms::{self, NewRoom};

use crate::{
    backend::Backend,
    store::Store,
    theme::{Palette, heading, space, text},
    views::{
        explore::Closed,
        modal::{accent_button, label, modal, well},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Create,
    Join,
}

pub struct CreateRoom {
    store: Entity<Store>,
    focus: FocusHandle,
    mode: Mode,
    busy: bool,
    error: Option<SharedString>,
    voice: bool,
    name: Entity<InputState>,
    topic: Entity<InputState>,
    alias: Entity<InputState>,
    is_public: bool,
    // On by default, and only a choice here: encryption can't be turned on
    // later in a way that protects what came before, or turned off at all.
    encrypted: bool,
    add_to_space: bool,
    address: Entity<InputState>,
}

impl EventEmitter<Closed> for CreateRoom {}

impl CreateRoom {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> CreateRoom {
        let input = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            cx.subscribe(&state, |_, _, _: &InputEvent, cx| cx.notify()).detach();
            state
        };
        let name = input("movie night", window, cx);
        let topic = input("optional~", window, cx);
        let alias = input("movie-night", window, cx);
        let address = input("#room:server.tld", window, cx);
        cx.subscribe_in(&address, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.join(cx);
            }
        })
        .detach();
        name.update(cx, |s, cx| s.focus(window, cx));
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        CreateRoom {
            store,
            focus: cx.focus_handle(),
            mode: Mode::Create,
            busy: false,
            error: None,
            voice: false,
            name,
            topic,
            alias,
            is_public: false,
            encrypted: true,
            add_to_space: true,
            address,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(Closed);
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() || self.busy {
            return;
        }
        let text_of = |input: &Entity<InputState>, cx: &Context<Self>| Some(input.read(cx).value().trim().to_owned()).filter(|s| !s.is_empty());
        let parent_space = if self.add_to_space { self.store.read(cx).space.clone() } else { None };
        let new = NewRoom {
            name,
            topic: text_of(&self.topic, cx),
            is_public: self.is_public,
            alias: text_of(&self.alias, cx),
            encrypted: self.encrypted,
            invite: Vec::new(),
            parent_space: parent_space.clone(),
            voice: self.voice,
        };
        self.busy = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::create(&*state.core().await?, new).await }
        });
        self.finish(task, parent_space.is_some(), |result| (result.room_id.clone(), result.space_warning.clone()), cx);
    }

    fn join(&mut self, cx: &mut Context<Self>) {
        let address = self.address.read(cx).value().trim().to_owned();
        if address.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::join(&*state.core().await?, &address, &[]).await }
        });
        self.finish(task, false, |room_id: &String| (room_id.clone(), None), cx);
    }

    /// Wait for the room to reach the list (sync brings it; opening one it
    /// doesn't know yet fails), then open it and close.
    fn finish<T: Send + 'static>(
        &mut self,
        task: impl std::future::Future<Output = Result<horo_core::Result<T>, impl std::fmt::Display>> + 'static,
        refresh_spaces: bool,
        room_of: impl FnOnce(&T) -> (String, Option<String>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.downgrade();
        cx.spawn(async move |this, cx| {
            let outcome = match task.await {
                Ok(Ok(value)) => Ok(room_of(&value)),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Ok((room_id, _)) = &outcome {
                if refresh_spaces {
                    let _ = store.update(cx, |s, cx| s.refresh_spaces_now(cx));
                }
                for _ in 0..(5000 / 120) {
                    let there = store.update(cx, |s, _| s.rooms.iter().any(|r| r.id == *room_id)).unwrap_or(true);
                    if there {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_millis(120)).await;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok((room_id, warning)) => {
                        this.store.update(cx, |s, cx| s.open_room(room_id, cx));
                        if let Some(warning) = warning {
                            this.store.update(cx, |s, cx| s.show_banner(crate::store::Tone::Error, warning, cx));
                        }
                        this.close(cx);
                    }
                    Err(e) => this.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn tab(&self, id: &'static str, words: &'static str, on: bool, palette: &Palette) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .px(space(14.))
            .py(space(7.))
            .rounded_full()
            .cursor_pointer()
            .text_size(text(10.))
            .font_weight(FontWeight::EXTRA_BOLD)
            .border_1()
            .map(|d| {
                if on {
                    d.bg(palette.inset).text_color(palette.text).border_color(palette.border)
                } else {
                    d.text_color(palette.text_tertiary).border_color(gpui::transparent_black())
                }
            })
            .child(words.to_uppercase())
    }

    fn switch_row(&self, id: &'static str, (title, hint): (String, String), on: bool, set: fn(&mut Self, bool), palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .items_start()
            .gap(space(12.))
            .mb(px(14.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::BOLD).text_size(heading(13.5)).child(title))
                    .child(div().mt(px(2.)).text_size(text(12.)).line_height(gpui::relative(1.45)).text_color(palette.text_tertiary).child(hint)),
            )
            .child(div().pt(space(2.)).child(Switch::new(id).checked(on).on_click(cx.listener(move |this, checked: &bool, _, cx| {
                set(this, *checked);
                cx.notify();
            }))))
            .into_any_element()
    }

    fn hint(words: impl Into<SharedString>, palette: &Palette) -> gpui::Div {
        div().mt(px(6.)).text_size(text(11.5)).line_height(gpui::relative(1.45)).text_color(palette.text_tertiary).child(words.into())
    }

    fn create_form(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let space_name = {
            let store = self.store.read(cx);
            store.space.as_ref().and_then(|id| store.spaces.iter().find(|s| s.id == *id)).map(|s| s.name.clone())
        };
        let voice = self.voice;
        let can_make = !self.name.read(cx).value().trim().is_empty() && !self.busy;
        let public_hint = if self.is_public {
            "listed in your server's directory, and anyone can walk in".to_owned()
        } else if let (Some(space), true) = (&space_name, self.add_to_space) {
            format!("anyone in {space} can join; nobody else without an invite")
        } else {
            "invite only".to_owned()
        };
        let encrypt_hint = if self.encrypted {
            "only people in the room can read it. this can't be turned off later."
        } else {
            "your homeserver can read everything sent here. you can't turn encryption on later."
        };
        let field = |words: &str, child: AnyElement| v_flex().mb(px(14.)).child(label(palette, &words.to_uppercase())).child(child);

        v_flex()
            .child(field(
                "kind",
                v_flex()
                    .child(
                        h_flex()
                            .gap(space(6.))
                            .child(self.tab("kind-text", "text", !voice, palette).on_click(cx.listener(|this, _, _, cx| {
                                this.voice = false;
                                cx.notify();
                            })))
                            .child(self.tab("kind-voice", "voice", voice, palette).on_click(cx.listener(|this, _, _, cx| {
                                this.voice = true;
                                cx.notify();
                            }))),
                    )
                    .child(Self::hint(
                        if voice {
                            "click it in the sidebar to hop in, and see who's there without opening it. chat still works inside."
                        } else {
                            "a room for talking in text. you can still start a call in it."
                        },
                        palette,
                    ))
                    .into_any_element(),
            ))
            .child(field("name", well(palette).child(div().flex_1().child(Input::new(&self.name).appearance(false))).into_any_element()))
            .child(field("what's it for", well(palette).child(div().flex_1().child(Input::new(&self.topic).appearance(false))).into_any_element()))
            .child(self.switch_row("create-public", ("anyone can join".into(), public_hint), self.is_public, |this, on| this.is_public = on, palette, cx))
            .child(field(
                "address",
                v_flex()
                    .child(
                        h_flex()
                            .gap(space(6.))
                            .child(div().font_family(palette.mono_font.clone()).text_color(palette.text_tertiary).child("#"))
                            .child(well(palette).flex_1().child(div().flex_1().child(Input::new(&self.alias).appearance(false)))),
                    )
                    .child(Self::hint(
                        "optional. an address is just a nicer name to share than the room id — it doesn't let anyone in on its own, that's what \"anyone can join\" above decides.",
                        palette,
                    ))
                    .into_any_element(),
            ))
            .child(self.switch_row("create-encrypted", ("encrypt it".into(), encrypt_hint.into()), self.encrypted, |this, on| this.encrypted = on, palette, cx))
            .children(space_name.map(|space| {
                self.switch_row(
                    "create-in-space",
                    (
                        format!("put it in {space}"),
                        "you need permission to manage that space — if you haven't got it, the room still gets made, just not filed there".into(),
                    ),
                    self.add_to_space,
                    |this, on| this.add_to_space = on,
                    palette,
                    cx,
                )
            }))
            .child(
                h_flex().justify_end().mt(px(22.)).child(
                    accent_button("create-make", if self.busy { "making it…" } else { "make it" }, can_make, false, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                ),
            )
            .into_any_element()
    }

    fn join_form(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let can_join = !self.address.read(cx).value().trim().is_empty() && !self.busy;
        v_flex()
            .child(label(palette, "ADDRESS OR ID"))
            .child(well(palette).child(div().flex_1().child(Input::new(&self.address).appearance(false))))
            .child(div().mt(px(10.)).text_size(text(12.)).line_height(gpui::relative(1.5)).text_color(palette.text_tertiary).child("an alias like #uwu:m.uwu.lt, or a room id if someone gave you one."))
            .child(
                h_flex().justify_end().mt(px(22.)).child(
                    accent_button("join-go", if self.busy { "joining…" } else { "join" }, can_join, false, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.join(cx))),
                ),
            )
            .into_any_element()
    }
}

impl Render for CreateRoom {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let mode = self.mode;
        let tabs = h_flex()
            .gap(space(6.))
            .mb(px(20.))
            .child(self.tab("mode-create", "make one", mode == Mode::Create, &palette).on_click(cx.listener(|this, _, window, cx| {
                this.mode = Mode::Create;
                this.name.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            })))
            .child(self.tab("mode-join", "join one", mode == Mode::Join, &palette).on_click(cx.listener(|this, _, window, cx| {
                this.mode = Mode::Join;
                this.address.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            })));
        let _ = window;
        let form = match mode {
            Mode::Create => self.create_form(&palette, cx),
            Mode::Join => self.join_form(&palette, cx),
        };
        let body = v_flex()
            .child(tabs)
            .children(self.error.clone().map(|e| div().mb(px(12.)).text_size(text(12.5)).text_color(palette.danger).child(e)))
            .child(form)
            .into_any_element();
        let this = cx.weak_entity();
        modal(
            "create-room",
            if mode == Mode::Create { "make a room~".into() } else { "join a room".into() },
            460.,
            &self.focus,
            &palette,
            move |_, cx| {
                let _ = this.update(cx, |this, cx| this.close(cx));
            },
            body,
        )
    }
}
