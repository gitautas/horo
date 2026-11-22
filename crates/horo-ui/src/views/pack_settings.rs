//! Settings' emotes & stickers section, as PackSettings.tsx: every pack
//! you can see, editable where you may (rename it, carry it everywhere,
//! add images, change an image's shortcode or what it's for, remove it),
//! and making a new pack, of your own or in a room.
//!
//! Every change is one edit horo-core applies to what the server has now
//! (read, change one thing, write back), then the list is read again, and
//! so are the picker's packs, so a new emote is usable straight away.

use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use horo_core::matrix::MatrixCore;

/// One change to run against the account, off the UI thread.
type Job = Pin<Box<dyn Future<Output = horo_core::Result<()>> + Send>>;

use gpui::{
    AnyElement, AppContext as _, AsyncWindowContext, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, WeakEntity, Window, div, img, prelude::FluentBuilder, px,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    switch::Switch,
    v_flex,
};
use horo_core::matrix::{
    packs::{self as core_packs, ImagePackDto, PackEdit, PackImageDto, PackRoomDto, PackSource, PackTarget},
    profile,
};

use crate::{
    backend::Backend,
    media::Media,
    packs,
    store::Store,
    theme::{Palette, corner, heading, space, text},
    views::{
        modal::{accent_button, label, well},
        voice::icon,
    },
};

pub struct PackSettings {
    store: Entity<Store>,
    /// `None` until first read.
    packs: Option<Vec<ImagePackDto>>,
    rooms: Vec<PackRoomDto>,
    busy: bool,
    error: Option<SharedString>,
    /// Each editable pack's name field, by pack id.
    names: HashMap<String, Entity<InputState>>,
    /// The image open for editing: (pack id, shortcode) and its field.
    editing: Option<(String, String, Entity<InputState>)>,
    new_mine: Entity<InputState>,
    new_room_name: Entity<InputState>,
    new_room: Option<String>,
}

fn target_of(pack: &ImagePackDto) -> PackTarget {
    PackTarget { room_id: pack.room_id.clone(), state_key: pack.state_key.clone() }
}

impl PackSettings {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> PackSettings {
        let new_mine = cx.new(|cx| InputState::new(window, cx).placeholder("pack name"));
        let new_room_name = cx.new(|cx| InputState::new(window, cx).placeholder("pack name"));
        for input in [&new_mine, &new_room_name] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        }
        let settings = PackSettings {
            store,
            packs: None,
            rooms: Vec::new(),
            busy: false,
            error: None,
            names: HashMap::new(),
            editing: None,
            new_mine,
            new_room_name,
            new_room: None,
        };
        settings.refresh(window, cx);
        settings
    }

    /// Read every pack and the rooms a pack could go in.
    fn refresh(&self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let all = core_packs::all(&core).await?;
                let rooms = core_packs::editable_rooms(&core).await?;
                Ok::<_, horo_core::Error>((all, rooms))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(Ok((all, rooms))) => {
                        this.sync_names(&all, window, cx);
                        this.packs = Some(all);
                        this.rooms = rooms;
                    }
                    Ok(Err(e)) => this.fail(e.to_string(), cx),
                    Err(e) => this.fail(e.to_string(), cx),
                }
                if this.packs.is_none() {
                    this.packs = Some(Vec::new());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error.into());
        cx.notify();
    }

    /// A name field for each pack this account may rename; renamed when it
    /// loses focus or on Enter.
    fn sync_names(&mut self, all: &[ImagePackDto], window: &mut Window, cx: &mut Context<Self>) {
        self.names.retain(|id, _| all.iter().any(|p| &p.id == id && p.can_edit));
        for pack in all.iter().filter(|p| p.can_edit) {
            match self.names.get(&pack.id) {
                Some(field) => {
                    if field.read(cx).value() != pack.display_name.as_str() && !gpui::Focusable::focus_handle(field.read(cx), cx).is_focused(window) {
                        let name = pack.display_name.clone();
                        field.update(cx, |f, cx| f.set_value(name, window, cx));
                    }
                }
                None => {
                    let name = pack.display_name.clone();
                    let field = cx.new(|cx| InputState::new(window, cx).placeholder("pack name").default_value(name));
                    let (id, target) = (pack.id.clone(), target_of(pack));
                    cx.subscribe_in(&field, window, move |this, field, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                            let name = field.read(cx).value().trim().to_owned();
                            let current = this.packs.iter().flatten().find(|p| p.id == id).map(|p| p.display_name.clone());
                            if !name.is_empty() && current.as_deref() != Some(name.as_str()) {
                                this.run_edit(target.clone(), PackEdit::SetName { name }, window, cx);
                            }
                        }
                    })
                    .detach();
                    self.names.insert(pack.id.clone(), field);
                }
            }
        }
    }

    /// Run one change, then read everything again (here and the picker's).
    fn run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        job: impl FnOnce(Arc<MatrixCore>) -> Job + Send + 'static,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { job(state.core().await?).await }
        });
        cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                this.refresh(window, cx);
                this.store.update(cx, |s, cx| s.reload_packs(cx));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run_edit(&mut self, target: PackTarget, edit: PackEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, move |core| Box::pin(async move { core_packs::edit(&core, target, edit).await }));
    }

    fn put(image: &PackImageDto, is_emoticon: bool, is_sticker: bool) -> PackEdit {
        PackEdit::PutImage {
            shortcode: image.shortcode.clone(),
            url: image.url.clone(),
            body: Some(image.body.clone()),
            is_emoticon,
            is_sticker,
            width: image.width,
            height: image.height,
            size: image.size,
            mimetype: image.mimetype.clone(),
        }
    }

    /// Pick images, upload each, and add it under its filename.
    fn add_images(&mut self, target: PackTarget, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("add".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let paths: Vec<String> = paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let _ = this.update_in(cx, |this, window, cx| {
                this.run(window, cx, move |core| Box::pin(async move {
                    let core = &*core;
                    for path in paths {
                        let url = profile::upload_media(core, &path).await?;
                        // Other clients size stickers by these; nobody wants
                        // to type them in.
                        let size = imagesize::size(&path).ok();
                        let edit = PackEdit::PutImage {
                            shortcode: packs::shortcode_from_path(&path),
                            url,
                            body: None,
                            is_emoticon: true,
                            is_sticker: true,
                            width: size.as_ref().map(|s| s.width as u32),
                            height: size.as_ref().map(|s| s.height as u32),
                            size: std::fs::metadata(&path).ok().map(|m| m.len()),
                            mimetype: packs::mime_from_path(&path),
                        };
                        core_packs::edit(core, target.clone(), edit).await?;
                    }
                    Ok(())
                }));
            });
        })
        .detach();
    }

    fn open_image(&mut self, pack: &ImagePackDto, image: &PackImageDto, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_ref().is_some_and(|(p, s, _)| *p == pack.id && *s == image.shortcode) {
            self.editing = None;
            cx.notify();
            return;
        }
        let code = image.shortcode.clone();
        let field = cx.new(|cx| InputState::new(window, cx).default_value(code.clone()));
        let target = target_of(pack);
        cx.subscribe_in(&field, window, move |this, field, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                let to = field.read(cx).value().trim().to_owned();
                if !to.is_empty() && to != code {
                    this.editing = None;
                    this.run_edit(target.clone(), PackEdit::Rename { from: code.clone(), to }, window, cx);
                }
            }
        })
        .detach();
        self.editing = Some((pack.id.clone(), image.shortcode.clone(), field));
        cx.notify();
    }

    fn thumb(image: &PackImageDto, size: f32, cx: &mut Context<Self>) -> AnyElement {
        match Media::image(&image.url, Some(((size * 2.) as u32, (size * 2.) as u32)), cx) {
            Some(picture) => img(picture).flex_none().size(px(size)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
            None => div().flex_none().size(px(size)).into_any_element(),
        }
    }

    fn pill(id: SharedString, words: &'static str, on: bool, palette: &Palette) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .px(space(12.))
            .py(space(5.))
            .rounded_full()
            .cursor_pointer()
            .font_family(palette.mono_font.clone())
            .text_size(text(11.5))
            .border_1()
            .map(|d| {
                if on {
                    d.text_color(palette.accent).bg(Hsla { a: 0.14, ..palette.accent }).border_color(palette.accent)
                } else {
                    d.text_color(palette.text_tertiary).border_color(palette.border_subtle)
                }
            })
            .child(words)
    }

    fn image_editor(&mut self, pack: &ImagePackDto, image: &PackImageDto, field: &Entity<InputState>, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let target = target_of(pack);
        let (emote, sticker) = (image.is_emoticon, image.is_sticker);
        let (t1, t2, t3) = (target.clone(), target.clone(), target);
        let (i1, i2) = (image.clone(), image.clone());
        let code = image.shortcode.clone();
        h_flex()
            .flex_wrap()
            .gap(space(10.))
            .p(space(12.))
            .mb(px(12.))
            .rounded(corner(16.))
            .bg(palette.inset)
            .border_1()
            .border_color(palette.border_subtle)
            .child(Self::thumb(image, 34., cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(160.))
                    .gap(space(4.))
                    .font_family(palette.mono_font.clone())
                    .child(div().text_color(palette.text_tertiary).child(":"))
                    .child(well(palette).flex_1().child(div().flex_1().child(Input::new(field).appearance(false))))
                    .child(div().text_color(palette.text_tertiary).child(":")),
            )
            .child(Self::pill("usage-emote".into(), "emote", emote, palette).on_click(cx.listener(move |this, _, window, cx| {
                this.run_edit(t1.clone(), Self::put(&i1, !emote, sticker), window, cx)
            })))
            .child(Self::pill("usage-sticker".into(), "sticker", sticker, palette).on_click(cx.listener(move |this, _, window, cx| {
                this.run_edit(t2.clone(), Self::put(&i2, emote, !sticker), window, cx)
            })))
            .child(
                div()
                    .id("image-remove")
                    .p(space(6.))
                    .cursor_pointer()
                    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(format!("remove :{code}:")).build(window, cx))
                    .child(icon("icons/trash.svg").size(px(15.)).text_color(palette.danger))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let shortcode = this.editing.take().map(|(_, s, _)| s).unwrap_or_default();
                        this.run_edit(t3.clone(), PackEdit::RemoveImage { shortcode }, window, cx)
                    })),
            )
            .into_any_element()
    }

    fn pack_card(&mut self, pack: &ImagePackDto, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let locked = !pack.can_edit;
        let title: AnyElement = match self.names.get(&pack.id).filter(|_| !locked) {
            Some(field) => well(palette).flex_1().py(space(6.)).font_weight(FontWeight::BOLD).child(div().flex_1().child(Input::new(field).appearance(false))).into_any_element(),
            None => div().flex_1().font_family(palette.heading_font.clone()).font_weight(FontWeight::BOLD).text_size(heading(15.)).child(pack.display_name.clone()).into_any_element(),
        };
        let everywhere = (pack.source == PackSource::Room).then(|| {
            let (room, key, on) = (pack.room_id.clone().unwrap_or_default(), pack.state_key.clone().unwrap_or_default(), pack.everywhere);
            h_flex()
                .gap(space(8.))
                .child(div().text_size(text(12.)).text_color(palette.text_tertiary).child("use everywhere"))
                .child(Switch::new(SharedString::from(format!("everywhere-{}", pack.id))).checked(on).on_click(cx.listener(move |this, checked: &bool, window, cx| {
                    let (room, key, on) = (room.clone(), key.clone(), *checked);
                    this.run(window, cx, move |core| Box::pin(async move {
                        let room = matrix_sdk::ruma::RoomId::parse(&room)?;
                        core_packs::set_everywhere(&core, &room, &key, on).await
                    }));
                })))
        });
        let note = (pack.source == PackSource::Room).then(|| {
            let whose = if locked {
                "you can use this pack, but you can't change it — that needs permission to send state in that room."
            } else {
                "shared with everyone in that room."
            };
            let reach = if pack.everywhere { "on in every room." } else { "on in that room only, until you turn it on everywhere." };
            div().mt(px(-8.)).mb(px(12.)).text_size(text(12.)).line_height(gpui::relative(1.5)).text_color(palette.text_tertiary).child(format!("{whose} {reach}"))
        });

        let editing = self.editing.clone().filter(|(p, _, _)| *p == pack.id);
        let mut grid = h_flex().flex_wrap().gap(space(6.)).mb(px(12.));
        for image in &pack.images {
            let on = editing.as_ref().is_some_and(|(_, s, _)| *s == image.shortcode);
            let face = Self::thumb(image, 38., cx);
            let (p, i) = (pack.clone(), image.clone());
            let code = image.shortcode.clone();
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("img-{}-{}", pack.id, image.shortcode)))
                    .size(px(56.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(14.))
                    .border_1()
                    .map(|d| if on { d.bg(Hsla { a: 0.16, ..palette.accent }).border_color(palette.accent) } else { d.bg(palette.inset).border_color(palette.border_subtle) })
                    .when(!locked, |d| d.cursor_pointer())
                    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(format!(":{code}:")).build(window, cx))
                    .child(face)
                    .when(!locked, |d| d.on_click(cx.listener(move |this, _, window, cx| this.open_image(&p, &i, window, cx)))),
            );
        }
        let editor = editing.and_then(|(_, code, field)| {
            let image = pack.images.iter().find(|i| i.shortcode == code)?.clone();
            Some(self.image_editor(pack, &image, &field, palette, cx))
        });
        let target = target_of(pack);
        let busy = self.busy;

        v_flex()
            .p(space(16.))
            .rounded(corner(20.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.border_subtle)
            .child(
                h_flex()
                    .gap(space(10.))
                    .mb(px(14.))
                    .child(icon(if pack.source == PackSource::User { "icons/circle-user.svg" } else { "icons/users.svg" }).size(px(17.)).text_color(palette.accent))
                    .child(title)
                    .children(everywhere),
            )
            .children(note)
            .map(|d| {
                if pack.images.is_empty() {
                    d.child(div().pb(space(12.)).pt(space(4.)).text_size(text(13.)).text_color(palette.text_tertiary).child("nothing in here yet~"))
                } else {
                    d.child(grid)
                }
            })
            .children(editor)
            .when(!locked, |d| {
                d.child(
                    h_flex().child(
                        Button::new(SharedString::from(format!("add-{}", pack.id)))
                            .ghost()
                            .small()
                            .label("+ add images")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| this.add_images(target.clone(), window, cx))),
                    ),
                )
            })
            .into_any_element()
    }

    fn new_pack(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let card = || v_flex().p(space(16.)).rounded(corner(20.)).bg(palette.card).border_1().border_color(palette.border_subtle);
        let hint = |words: &'static str| div().mt(px(10.)).text_size(text(12.)).line_height(gpui::relative(1.5)).text_color(palette.text_tertiary).child(words);
        let mine_ready = !self.new_mine.read(cx).value().trim().is_empty() && !self.busy;
        let mine = card()
            .child(label(palette, "NEW PACK OF YOUR OWN"))
            .child(
                h_flex()
                    .gap(space(8.))
                    .child(well(palette).flex_1().child(div().flex_1().child(Input::new(&self.new_mine).appearance(false))))
                    .child(accent_button("new-mine", "make it~", mine_ready, false, palette).on_click(cx.listener(move |this, _, window, cx| {
                        let name = this.new_mine.read(cx).value().trim().to_owned();
                        if name.is_empty() || this.busy {
                            return;
                        }
                        this.new_mine.update(cx, |f, cx| f.set_value("", window, cx));
                        this.run(window, cx, move |core| Box::pin(async move { core_packs::create_personal_pack(&core, &name).await.map(|_| ()) }));
                    }))),
            )
            .child(hint("as many as you like. each one gets a private room to live in — matrix keeps packs in rooms — and is turned on everywhere straight away."));

        let in_room = (!self.rooms.is_empty()).then(|| {
            let chosen = self.new_room.clone();
            let ready = chosen.is_some() && !self.new_room_name.read(cx).value().trim().is_empty() && !self.busy;
            let mut chips = h_flex().flex_wrap().gap(space(6.)).mb(px(10.));
            for room in &self.rooms {
                let on = chosen.as_deref() == Some(room.id.as_str());
                let id = room.id.clone();
                chips = chips.child(
                    div()
                        .id(SharedString::from(format!("pack-room-{}", room.id)))
                        .px(space(11.))
                        .py(space(5.))
                        .rounded_full()
                        .cursor_pointer()
                        .text_size(text(12.))
                        .border_1()
                        .map(|d| if on { d.bg(palette.accent).text_color(palette.text_on_accent).border_color(palette.accent) } else { d.text_color(palette.text_secondary).border_color(palette.border_subtle) })
                        .child(room.name.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.new_room = Some(id.clone());
                            cx.notify();
                        })),
                );
            }
            card()
                .child(label(palette, "NEW PACK IN A ROOM"))
                .child(chips)
                .child(
                    h_flex()
                        .gap(space(8.))
                        .child(well(palette).flex_1().child(div().flex_1().child(Input::new(&self.new_room_name).appearance(false))))
                        .child(accent_button("new-room-pack", "make it~", ready, false, palette).on_click(cx.listener(move |this, _, window, cx| {
                            let name = this.new_room_name.read(cx).value().trim().to_owned();
                            let Some(room) = this.new_room.clone().filter(|_| !name.is_empty() && !this.busy) else { return };
                            this.new_room_name.update(cx, |f, cx| f.set_value("", window, cx));
                            // The state key tells packs in one room apart;
                            // made from the name so it reads in a dump.
                            let target = PackTarget { room_id: Some(room), state_key: Some(packs::state_key_for(&name)) };
                            this.run_edit(target, PackEdit::SetName { name }, window, cx);
                        }))),
                )
                .child(hint("everyone in the room gets these, and other matrix clients can read them too."))
        });
        v_flex().gap(space(12.)).child(mine).children(in_room).into_any_element()
    }
}

impl Render for PackSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let heading_el = div().pt_2().font_family(palette.heading_font.clone()).text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child("emotes & stickers");
        let Some(all) = self.packs.clone() else {
            return v_flex().gap(space(12.)).child(heading_el).child(div().py(space(40.)).text_center().text_color(palette.text_tertiary).child("loading…")).into_any_element();
        };
        // The MSC's one personal pack only exists on accounts an older client
        // made one for; shown when it has something in it, never offered.
        let legacy = all.iter().find(|p| p.source == PackSource::User && !p.images.is_empty()).cloned();
        let rooms: Vec<ImagePackDto> = all.iter().filter(|p| p.source == PackSource::Room).cloned().collect();
        let mut cards: Vec<AnyElement> = Vec::new();
        for pack in rooms.iter().chain(legacy.iter()) {
            cards.push(self.pack_card(pack, &palette, cx));
        }
        let empty = cards.is_empty().then(|| {
            v_flex().p(space(16.)).rounded(corner(20.)).bg(palette.card).border_1().border_color(palette.border_subtle).child(
                div()
                    .text_size(text(13.))
                    .line_height(gpui::relative(1.55))
                    .text_color(palette.text_secondary)
                    .child("no packs yet. make as many as you like — one for your own images, or one in a room to share with everyone in it."),
            )
        });
        let new_pack = self.new_pack(&palette, cx);
        v_flex()
            .gap(space(12.))
            .child(heading_el)
            .children(self.error.clone().map(|e| div().text_size(text(12.5)).text_color(palette.danger).child(e)))
            .children(empty)
            .children(cards)
            .child(new_pack)
            .into_any_element()
    }
}
