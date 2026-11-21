//! Settings over the whole window, as SettingsView.tsx lays them out: the
//! sections down the left, one at a time on the right, Escape to leave.
//!
//! Account (who you are, your profile, signing out), security (devices and
//! key backup) and about. Voice, notifications, emotes and appearance join
//! as the app gains the things they set.

use gpui::{
    AnyElement, AppContext, Context, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder, px,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use horo_core::{
    dto::SessionInfo,
    matrix::profile::{self, ProfileDto, ProfileUpdate},
};

use crate::{
    backend::Backend,
    media::Media,
    store::{Phase, SettingsSection, Store},
    theme::Palette,
    views::{avatar::avatar, security::Security},
};

pub struct Settings {
    store: Entity<Store>,
    focus: FocusHandle,
    security: Option<Entity<Security>>,
    /// Your profile as the server last said; `None` while it loads.
    profile: Option<ProfileDto>,
    status: Entity<InputState>,
    bio: Entity<TextareaState>,
    /// The cover photo being edited (an mxc URI, or empty for none).
    cover: String,
    busy: bool,
    saved: bool,
    error: Option<SharedString>,
    /// "sign out and erase keys" has been pressed once.
    confirm_wipe: bool,
}

impl Settings {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Settings {
        let status = cx.new(|cx| InputState::new(window, cx).placeholder("vibing~"));
        let bio = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_auto_grow(3, 10, cx);
            state.set_placeholder("say something about yourself~", window, cx);
            state
        });
        // Typing changes whether there's anything to save.
        cx.subscribe(&status, |this, _, _: &InputEvent, cx| this.edited(cx)).detach();
        cx.subscribe(&bio, |this, _, _: &InputEvent, cx| this.edited(cx)).detach();
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut settings = Settings {
            store,
            focus,
            security: None,
            profile: None,
            status,
            bio,
            cover: String::new(),
            busy: false,
            saved: false,
            error: None,
            confirm_wipe: false,
        };
        settings.load_profile(window, cx);
        settings
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.saved = false;
        cx.notify();
    }

    fn load_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { profile::get_profile(&*state.core().await?, None).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(Ok(profile)) => {
                        let (status, bio) = (profile.status.clone().unwrap_or_default(), profile.bio.clone().unwrap_or_default());
                        this.status.update(cx, |s, cx| s.set_value(status, window, cx));
                        this.bio.update(cx, |s, cx| s.set_value(bio, window, cx));
                        this.cover = profile.cover_url.clone().unwrap_or_default();
                        this.profile = Some(profile);
                    }
                    Ok(Err(e)) => this.error = Some(format!("couldn't load your profile: {e}").into()),
                    Err(e) => this.error = Some(format!("couldn't load your profile: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// What's changed from the profile the server has: only that is sent,
    /// so a field left alone stays alone server-side.
    fn changes(&self, cx: &gpui::App) -> Option<ProfileUpdate> {
        let profile = self.profile.as_ref()?;
        let status = self.status.read(cx).value().to_string();
        let bio = self.bio.read(cx).value().to_string();
        let differs = |now: &str, then: &Option<String>| (now != then.as_deref().unwrap_or_default()).then(|| now.to_owned());
        let update = ProfileUpdate {
            status: differs(&status, &profile.status),
            bio: differs(&bio, &profile.bio),
            cover_url: differs(&self.cover, &profile.cover_url),
        };
        (update.status.is_some() || update.bio.is_some() || update.cover_url.is_some()).then_some(update)
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(update) = self.changes(cx) else { return };
        self.busy = true;
        self.error = None;
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                profile::set_profile(&core, update).await?;
                profile::get_profile(&core, None).await
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                match outcome {
                    Ok(Ok(profile)) => {
                        this.cover = profile.cover_url.clone().unwrap_or_default();
                        this.profile = Some(profile);
                        this.saved = true;
                    }
                    Ok(Err(e)) => this.error = Some(format!("couldn't save: {e}").into()),
                    Err(e) => this.error = Some(format!("couldn't save: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Pick a picture and upload it; it becomes the cover once saved.
    fn choose_cover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("choose a cover photo".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let Ok(task) = this.update(cx, |this, cx| {
                this.busy = true;
                this.error = None;
                cx.notify();
                let backend = Backend::global(cx);
                backend.run({
                    let state = backend.state.clone();
                    async move { profile::upload_media(&*state.core().await?, &path.to_string_lossy()).await }
                })
            }) else {
                return;
            };
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(Ok(mxc)) => {
                        this.cover = mxc;
                        this.saved = false;
                    }
                    Ok(Err(e)) => this.error = Some(format!("couldn't upload that: {e}").into()),
                    Err(e) => this.error = Some(format!("couldn't upload that: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.show_settings(None, cx));
    }

    fn account(&mut self, session: &SessionInfo, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let name = session.display_name.clone().unwrap_or_else(|| session.user_id.clone());
        let face = avatar(&session.user_id, &name, session.avatar_url.as_deref(), px(64.), true, palette.text_on_accent, cx);

        let identity = card(palette).child(
            h_flex()
                .gap_4()
                .child(face)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().font_family(palette.heading_font.clone()).text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(div().text_xs().text_color(palette.text_tertiary).child(session.user_id.clone())),
                ),
        );
        let where_ = card(palette)
            .child(row(palette, "homeserver", session.homeserver.clone()))
            .child(row(palette, "this device", session.device_id.clone()));

        let insecure = session.insecure_storage.then(|| {
            card(palette).border_color(Hsla { a: 0.4, ..palette.idle }).child(div().text_sm().text_color(palette.text_secondary).child(
                "no system keychain was available, so your access token and encryption keys are in a file readable only by your user account. anyone who can read your home directory can read them.",
            ))
        });

        let sign_out = card(palette)
            .child(div().text_sm().text_color(palette.text_secondary).child(
                "signing out keeps your local encryption keys, so you can sign back in and still read your history.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("sign-out").ghost().label("sign out").on_click(cx.listener(|this, _, _, cx| {
                        this.store.update(cx, |s, cx| s.sign_out(false, cx))
                    })))
                    .child({
                        let button = Button::new("sign-out-wipe").label(if self.confirm_wipe { "yes — erase everything" } else { "sign out and erase keys" });
                        let button = if self.confirm_wipe { button.danger() } else { button.ghost() };
                        button.on_click(cx.listener(|this, _, _, cx| {
                            if this.confirm_wipe {
                                this.store.update(cx, |s, cx| s.sign_out(true, cx));
                            } else {
                                this.confirm_wipe = true;
                                cx.notify();
                            }
                        }))
                    }),
            )
            .when(self.confirm_wipe, |c| {
                c.child(div().text_sm().text_color(palette.danger).child(
                    "this deletes the encrypted store on this machine. any messages whose keys aren't in your key backup become permanently unreadable — including for you.",
                ))
            });

        v_flex()
            .gap_4()
            .child(heading(palette, "account"))
            .child(identity)
            .child(where_)
            .child(heading(palette, "profile"))
            .child(self.profile_editor(palette, cx))
            .children(insecure)
            .child(heading(palette, "sign out"))
            .child(sign_out)
            .into_any_element()
    }

    fn profile_editor(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.profile.is_none() {
            return card(palette).child(div().text_sm().text_color(palette.text_tertiary).child("loading…")).into_any_element();
        }
        let preview = (!self.cover.is_empty()).then(|| Media::image(&self.cover, Some((1120, 280)), cx)).flatten();
        let cover = div()
            .h(px(96.))
            .w_full()
            .rounded(px(14.))
            .overflow_hidden()
            .border_1()
            .border_color(palette.border_subtle)
            .bg(palette.inset)
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match preview {
                Some(image) => d.child(img(image).size_full().object_fit(gpui::ObjectFit::Cover)),
                None => d.child(div().text_xs().text_color(palette.text_tertiary).child(if self.cover.is_empty() { "no cover yet" } else { "loading…" })),
            });
        let dirty = self.changes(cx).is_some();

        card(palette)
            .child(field(palette, "cover photo", Some("shown behind your avatar on your profile card.")))
            .child(cover)
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("cover-choose").ghost().label("choose an image").disabled(self.busy).on_click(cx.listener(|this, _, window, cx| this.choose_cover(window, cx))))
                    .when(!self.cover.is_empty(), |row| {
                        row.child(Button::new("cover-remove").ghost().label("remove").disabled(self.busy).on_click(cx.listener(|this, _, _, cx| {
                            this.cover.clear();
                            this.saved = false;
                            cx.notify();
                        })))
                    }),
            )
            .child(field(palette, "status", Some("a short line — what you're up to.")))
            .child(Input::new(&self.status))
            .child(field(palette, "bio", None))
            .child(Textarea::new(&self.bio))
            .children(self.error.clone().map(|e| div().text_sm().text_color(palette.danger).child(e)))
            .child(
                h_flex()
                    .gap_3()
                    .child(Button::new("profile-save").primary().label("save").loading(self.busy).disabled(!dirty || self.busy).on_click(cx.listener(|this, _, window, cx| this.save(window, cx))))
                    .when(self.saved && !dirty, |row| row.child(div().text_sm().text_color(palette.accent).child("saved~")))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(palette.text_tertiary).child("public and federated — anyone who can see you can read this")),
            )
            .into_any_element()
    }

    fn about(&self, palette: &Palette) -> AnyElement {
        v_flex()
            .gap_4()
            .child(heading(palette, "about"))
            .child(
                card(palette)
                    .child(div().font_family(palette.heading_font.clone()).text_size(px(36.)).text_color(palette.accent).child("horo"))
                    .child(div().text_sm().text_color(palette.text_secondary).child("a cute matrix client~"))
                    .child(
                        v_flex()
                            .pt_2()
                            .gap_1()
                            .font_family(palette.mono_font.clone())
                            .text_xs()
                            .text_color(palette.text_tertiary)
                            .child(format!("version {} (gpui)", env!("CARGO_PKG_VERSION")))
                            .child("matrix-rust-sdk 0.18 · e2ee via megolm")
                            .child("voice over matrixrtc + livekit, with horo's own media engine"),
                    ),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let store = self.store.read(cx);
        let section = store.settings.unwrap_or(SettingsSection::Account);
        let session = match &store.phase {
            Phase::SignedIn(info) => Some(info.clone()),
            _ => None,
        };

        if section == SettingsSection::Security && self.security.is_none() {
            let store = self.store.clone();
            self.security = Some(cx.new(|cx| Security::new(store, window, cx)));
        }

        let nav = v_flex()
            .w(px(232.))
            .flex_none()
            .h_full()
            .pt(px(52.))
            .px_3()
            .gap(px(3.))
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border_subtle)
            .child(div().px_3().pb_2().font_family(palette.heading_font.clone()).text_size(px(15.)).text_color(palette.text_tertiary).child("settings"))
            .children(SettingsSection::ALL.into_iter().map(|entry| {
                let selected = entry == section;
                div()
                    .id(SharedString::from(format!("settings-{}", entry.label())))
                    .px_3()
                    .py_2()
                    .rounded(px(12.))
                    .cursor_pointer()
                    .font_family(palette.heading_font.clone())
                    .text_size(px(17.))
                    .text_color(if selected { palette.text } else { palette.text_secondary })
                    .when(selected, |d| d.bg(palette.raised))
                    .hover(|d| d.bg(palette.raised))
                    .child(entry.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.show_settings(Some(entry), cx))))
            }));

        let body: AnyElement = match (section, &session) {
            (SettingsSection::Account, Some(session)) => self.account(session, &palette, cx),
            (SettingsSection::Security, _) => self.security.clone().map(|s| s.into_any_element()).unwrap_or_else(|| div().into_any_element()),
            (SettingsSection::About, _) => self.about(&palette),
            _ => div().into_any_element(),
        };

        h_flex()
            .id("settings")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
            .occlude()
            .absolute()
            .inset_0()
            .bg(palette.app)
            .text_color(palette.text)
            .child(nav)
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .pt(px(52.))
                    .pb_10()
                    .child(div().max_w(px(620.)).mx_auto().px_10().child(body)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(40.))
                    .right(px(32.))
                    .child(Button::new("settings-close").ghost().label("close (esc)").on_click(cx.listener(|this, _, _, cx| this.close(cx)))),
            )
    }
}

fn heading(palette: &Palette, text: &str) -> gpui::Div {
    div().pt_2().font_family(palette.heading_font.clone()).text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(text.to_owned()))
}

fn card(palette: &Palette) -> gpui::Div {
    v_flex().gap_3().p_4().rounded(px(16.)).bg(palette.card).border_1().border_color(palette.border_subtle)
}

fn row(palette: &Palette, title: &str, value: String) -> gpui::Div {
    v_flex()
        .child(div().text_sm().child(SharedString::from(title.to_owned())))
        .child(div().text_xs().text_color(palette.text_tertiary).child(value))
}

fn field(palette: &Palette, label: &str, hint: Option<&str>) -> gpui::Div {
    v_flex()
        .pt_1()
        .child(div().text_sm().text_color(palette.text_secondary).child(SharedString::from(label.to_owned())))
        .children(hint.map(|h| div().text_xs().text_color(palette.text_tertiary).child(SharedString::from(h.to_owned()))))
}
