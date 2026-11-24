//! Settings over the whole window, as SettingsView.tsx lays them out: the
//! sections down the left, one at a time on the right, Escape to leave.
//!
//! Account (who you are, your profile, signing out), security (devices and
//! key backup), notifications (banners, sounds, volume) and about. Voice,
//! emotes and appearance join as the app gains the things they set.

use gpui::{
    AnyElement, AppContext, Context, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use horo_rtc::devices::DeviceInfo;
use horo_core::{
    dto::SessionInfo,
    matrix::profile::{self, ProfileDto, ProfileUpdate},
};

use crate::theme::zpx;
use crate::{
    backend::Backend,
    media::Media,
    prefs::Prefs,
    sounds::{self, CALL_SOUNDS, Kind, MESSAGE_SOUNDS, NONE},
    store::{Phase, SettingsSection, Store},
    theme::Palette,
    views::{avatar::avatar, pack_settings::PackSettings, security::Security},
};

pub struct Settings {
    store: Entity<Store>,
    focus: FocusHandle,
    security: Option<Entity<Security>>,
    packs: Option<Entity<PackSettings>>,
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
    volume: Entity<SliderState>,
    /// Microphones and speakers, once listed.
    devices: Option<(Vec<DeviceInfo>, Vec<DeviceInfo>)>,
    /// The chosen microphone, listened to while the voice section is open.
    meter: Option<Preview<horo_rtc::voice::Meter>>,
    /// The chosen camera, likewise; and the cameras there are.
    #[cfg(target_os = "macos")]
    camera: Option<Preview<crate::camera::Camera>>,
    #[cfg(target_os = "macos")]
    cameras: Option<Vec<DeviceInfo>>,
    /// We've asked for the camera (the system asks the user once).
    #[cfg(target_os = "macos")]
    camera_asked: bool,
}

/// A device opened for a preview: which, and how it went.
struct Preview<T> {
    device: Option<String>,
    state: PreviewState<T>,
}

enum PreviewState<T> {
    Opening,
    Live(T),
    Failed(String),
}

impl<T: Send + 'static> Preview<T> {
    /// Open `device` off the main thread (it can take a moment); the
    /// result lands in `slot` if it still wants that device.
    fn open(
        device: Option<String>,
        start: impl FnOnce(Option<&str>) -> Result<T, String> + Send + 'static,
        slot: fn(&mut Settings) -> &mut Option<Preview<T>>,
        cx: &mut Context<Settings>,
    ) -> Preview<T> {
        let wanted = device.clone();
        let task = cx.background_spawn(async move { start(wanted.as_deref()) });
        let wanted = device.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(preview) = slot(this)
                    && preview.device == wanted
                    && matches!(preview.state, PreviewState::Opening)
                {
                    preview.state = match result {
                        Ok(live) => PreviewState::Live(live),
                        Err(e) => PreviewState::Failed(e),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        Preview { device, state: PreviewState::Opening }
    }
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

        let start = store.read(cx).prefs.notification_volume;
        let volume = cx.new(|_| SliderState::new().min(0.0).max(1.0).step(0.05).default_value(start));
        // Saved, and played so you hear it, when the slider is let go.
        cx.subscribe(&volume, |this, _, event: &SliderEvent, cx| {
            if let SliderEvent::Release(value) = event {
                let level = value.start();
                this.store.update(cx, |s, cx| s.update_prefs(|p| p.notification_volume = level, cx));
                let id = this.store.read(cx).prefs.message_sound.clone();
                sounds::play(Kind::Message, &id, level);
            }
        })
        .detach();

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut settings = Settings {
            store,
            focus,
            security: None,
            packs: None,
            profile: None,
            status,
            bio,
            cover: String::new(),
            busy: false,
            saved: false,
            error: None,
            confirm_wipe: false,
            volume,
            devices: None,
            meter: None,
            #[cfg(target_os = "macos")]
            camera: None,
            #[cfg(target_os = "macos")]
            cameras: None,
            #[cfg(target_os = "macos")]
            camera_asked: false,
        };
        settings.list_devices(cx);
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
        let face = avatar(&session.user_id, &name, session.avatar_url.as_deref(), zpx(64.), cx);

        let identity = card(palette).child(
            h_flex()
                .gap_4()
                .child(face)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().font_family(palette.heading_font.clone()).text_size(zpx(24.)).font_weight(FontWeight::SEMIBOLD).child(name))
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
            .h(zpx(96.))
            .w_full()
            .rounded(zpx(14.))
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

    fn notifications(&mut self, prefs: &Prefs, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let toggle = |id: &'static str, title: &'static str, detail: &'static str, on: bool, set: fn(&mut Prefs, bool), cx: &mut Context<Self>| {
            h_flex()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .child(div().text_sm().child(title))
                        .child(div().text_xs().text_color(palette.text_tertiary).child(detail)),
                )
                .child(Switch::new(id).checked(on).on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    let checked = *checked;
                    this.store.update(cx, |s, cx| s.update_prefs(|p| set(p, checked), cx));
                })))
        };
        let banners = card(palette)
            .child(toggle("notify-messages", "messages", "uses your account's push rules, so muted rooms stay quiet", prefs.notify_messages, |p, on| p.notify_messages = on, cx))
            .child(toggle("notify-calls", "calls", "when a call starts in one of your rooms", prefs.notify_calls, |p, on| p.notify_calls = on, cx))
            .child(toggle(
                "notify-focused",
                "notify while i'm here",
                "off: no banner for the room you're already reading",
                prefs.notify_when_focused,
                |p, on| p.notify_when_focused = on,
                cx,
            ));

        let picker = |kind: Kind, current: &str, cx: &mut Context<Self>| {
            let list = match kind {
                Kind::Message => MESSAGE_SOUNDS,
                Kind::Call => CALL_SOUNDS,
            };
            let ids = list.iter().map(|s| s.id).chain([NONE]);
            h_flex().flex_wrap().gap_2().children(ids.map(|id| {
                let selected = id == current;
                div()
                    .id(SharedString::from(format!("sound-{kind:?}-{id}")))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .border_1()
                    .border_color(if selected { palette.accent } else { palette.border })
                    .bg(if selected { Hsla { a: 0.15, ..palette.accent } } else { palette.raised })
                    .text_sm()
                    .cursor_pointer()
                    .child(id)
                    // Choosing one plays it, so you know what you picked.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.update(cx, |s, cx| {
                            s.update_prefs(
                                |p| match kind {
                                    Kind::Message => p.message_sound = id.to_owned(),
                                    Kind::Call => p.call_sound = id.to_owned(),
                                },
                                cx,
                            )
                        });
                        let volume = this.store.read(cx).prefs.notification_volume;
                        sounds::play(kind, id, volume);
                    }))
            }))
        };
        let sounds = card(palette)
            .child(field(palette, "message sound", Some("played when a message arrives you'd be pinged for.")))
            .child(picker(Kind::Message, &prefs.message_sound, cx))
            .child(field(palette, "call sound", Some("played when someone starts a call.")))
            .child(picker(Kind::Call, &prefs.call_sound, cx))
            .child(field(palette, "volume", None))
            .child(
                h_flex()
                    .gap_3()
                    .child(div().flex_1().child(Slider::new(&self.volume)))
                    .child(div().w(zpx(40.)).text_sm().text_color(palette.text_secondary).child(format!("{:.0}%", prefs.notification_volume * 100.0))),
            );

        v_flex()
            .gap_4()
            .child(heading(palette, "notifications"))
            .child(banners)
            .child(heading(palette, "sounds"))
            .child(sounds)
            .into_any_element()
    }

    /// Ask the system for microphones and speakers, off the UI thread.
    /// Keep the microphone meter and the camera preview running on the
    /// devices chosen now.
    fn run_previews(&mut self, cx: &mut Context<Self>) {
        let prefs = self.store.read(cx).prefs.clone();
        if self.meter.as_ref().is_none_or(|m| m.device != prefs.audio_input) {
            self.meter = Some(Preview::open(
                prefs.audio_input.clone(),
                |device| horo_rtc::voice::Meter::start(device).map_err(|e| e.to_string()),
                |this| &mut this.meter,
                cx,
            ));
        }
        #[cfg(target_os = "macos")]
        {
            if self.cameras.is_none() {
                self.cameras = Some(Vec::new());
                let task = cx.background_spawn(async move { crate::camera::cameras() });
                cx.spawn(async move |this, cx| {
                    let found = task.await;
                    let _ = this.update(cx, |this, cx| {
                        this.cameras = Some(found.into_iter().map(|c| DeviceInfo { id: c.id, name: c.name, is_default: c.is_default }).collect());
                        cx.notify();
                    });
                })
                .detach();
            }
            match crate::camera::allowed() {
                Some(true) => {
                    if self.camera.as_ref().is_none_or(|c| c.device != prefs.video_input) {
                        self.camera = Some(Preview::open(prefs.video_input.clone(), crate::camera::Camera::start, |this| &mut this.camera, cx));
                    }
                }
                // The system asks; the next frame sees the answer.
                None if !self.camera_asked => {
                    self.camera_asked = true;
                    crate::camera::ask(|_| {});
                }
                _ => {}
            }
        }
    }

    fn stop_previews(&mut self) {
        self.meter = None;
        #[cfg(target_os = "macos")]
        {
            self.camera = None;
        }
    }

    fn list_devices(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move { (horo_rtc::devices::inputs().unwrap_or_default(), horo_rtc::devices::outputs().unwrap_or_default()) });
        cx.spawn(async move |this, cx| {
            let devices = task.await;
            let _ = this.update(cx, |this, cx| {
                this.devices = Some(devices);
                cx.notify();
            });
        })
        .detach();
    }

    fn voice(&mut self, prefs: &Prefs, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some((inputs, outputs)) = self.devices.clone() else {
            return v_flex()
                .gap_4()
                .child(heading(palette, "media"))
                .child(card(palette).child(div().text_sm().text_color(palette.text_tertiary).child("looking for devices…")))
                .into_any_element();
        };
        // "system default" first, chosen when nothing is saved or the saved
        // device isn't plugged in: that's what a call falls back to.
        let picker = |key: &'static str, list: &[DeviceInfo], saved: &Option<String>, set: fn(&mut Prefs, Option<String>), cx: &mut Context<Self>| {
            let saved_present = saved.as_ref().filter(|id| list.iter().any(|d| &d.id == *id));
            let default_name = list.iter().find(|d| d.is_default).map(|d| d.name.clone());
            let mut rows = vec![(None, match default_name {
                Some(name) => format!("system default ({name})"),
                None => "system default".to_owned(),
            })];
            rows.extend(list.iter().map(|d| (Some(d.id.clone()), d.name.clone())));
            v_flex().gap(zpx(2.)).children(rows.into_iter().enumerate().map(|(i, (id, name))| {
                let selected = id.as_ref() == saved_present;
                h_flex()
                    .id(SharedString::from(format!("{key}-{i}")))
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(zpx(10.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(palette.raised))
                    .hover(|d| d.bg(palette.raised))
                    .child(div().flex_none().size(zpx(10.)).rounded_full().border_2().border_color(if selected { palette.accent } else { palette.border_strong }).when(selected, |d| d.bg(palette.accent)))
                    .child(div().truncate().text_sm().child(name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.store.update(cx, |s, cx| s.update_prefs(|p| set(p, id), cx));
                    }))
            }))
        };
        let in_call = self.store.read(cx).call.is_some();
        v_flex()
            .gap_4()
            .child(heading(palette, "media"))
            .child(
                card(palette)
                    .child(field(palette, "output device", Some("where you hear everyone else.")))
                    .child(picker("output", &outputs, &prefs.audio_output, |p, id| p.audio_output = id, cx))
                    .child(field(palette, "input device", Some("the microphone other people hear.")))
                    .child(picker("input", &inputs, &prefs.audio_input, |p, id| p.audio_input = id, cx))
                    .child(self.meter_bar(palette))
                    .when(inputs.is_empty(), |c| {
                        c.child(div().text_sm().text_color(palette.idle).child("no microphones found — check that horo has permission in system settings."))
                    })
                    .when(in_call, |c| c.child(div().text_xs().text_color(palette.text_tertiary).child("a change applies from your next call.")))
                    .child(
                        h_flex()
                            .child(Button::new("devices-refresh").ghost().label("look again").on_click(cx.listener(|this, _, _, cx| {
                                this.devices = None;
                                this.list_devices(cx);
                                cx.notify();
                            }))),
                    ),
            )
            .children(self.camera_card(prefs, palette, cx))
            .child(div().text_xs().text_color(palette.text_tertiary).child(
                "echo cancellation and noise suppression are always on: horo's own, the same on every platform.",
            ))
            .into_any_element()
    }

    /// How loud the chosen microphone is, live.
    fn meter_bar(&self, palette: &Palette) -> AnyElement {
        let (level, note) = match self.meter.as_ref().map(|m| &m.state) {
            Some(PreviewState::Live(meter)) => (meter.level(), "say something to check your microphone~".to_owned()),
            Some(PreviewState::Failed(why)) => (0., format!("couldn't open the microphone: {why}")),
            _ => (0., "opening the microphone…".to_owned()),
        };
        v_flex()
            .gap_1()
            .pt_1()
            .child(
                div()
                    .h(zpx(8.))
                    .w_full()
                    .rounded_full()
                    .bg(palette.inset)
                    .overflow_hidden()
                    .child(div().h_full().w(gpui::relative(level)).rounded_full().bg(if level > 0.85 { palette.idle } else { palette.online })),
            )
            .child(div().text_xs().text_color(palette.text_tertiary).child(note))
            .into_any_element()
    }

    /// The camera picker, with what the chosen camera sees.
    #[cfg(target_os = "macos")]
    fn camera_card(&mut self, prefs: &Prefs, palette: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cameras = self.cameras.clone().unwrap_or_default();
        let picture: AnyElement = match crate::camera::allowed() {
            Some(false) => message(palette, "horo isn't allowed to use the camera — allow it in system settings › privacy & security › camera."),
            None => message(palette, "asking for the camera…"),
            Some(true) => match self.camera.as_mut().map(|c| &mut c.state) {
                Some(PreviewState::Live(camera)) => match camera.mirrored_frame() {
                    Some(frame) => gpui::surface(frame).w(zpx(PREVIEW_W)).h(zpx(PREVIEW_H)).object_fit(gpui::ObjectFit::Cover).into_any_element(),
                    None => message(palette, "waiting for the camera…"),
                },
                Some(PreviewState::Failed(why)) => message(palette, &format!("couldn't open the camera: {why}")),
                _ => message(palette, "opening the camera…"),
            },
        };
        let saved_present = prefs.video_input.as_ref().filter(|id| cameras.iter().any(|d| &d.id == *id));
        let default_name = cameras.iter().find(|d| d.is_default).map(|d| d.name.clone());
        let mut rows = vec![(None, match default_name {
            Some(name) => format!("system default ({name})"),
            None => "system default".to_owned(),
        })];
        rows.extend(cameras.iter().map(|d| (Some(d.id.clone()), d.name.clone())));
        let list = v_flex().gap(zpx(2.)).children(rows.into_iter().enumerate().map(|(i, (id, name))| {
            let selected = id.as_ref() == saved_present;
            h_flex()
                .id(SharedString::from(format!("camera-{i}")))
                .gap_2()
                .px_3()
                .py_2()
                .rounded(zpx(10.))
                .cursor_pointer()
                .when(selected, |d| d.bg(palette.raised))
                .hover(|d| d.bg(palette.raised))
                .child(div().flex_none().size(zpx(10.)).rounded_full().border_2().border_color(if selected { palette.accent } else { palette.border_strong }).when(selected, |d| d.bg(palette.accent)))
                .child(div().truncate().text_sm().child(name))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.store.update(cx, |s, cx| s.update_prefs(|p| p.video_input = id, cx));
                }))
        }));
        Some(
            card(palette)
                .child(field(palette, "camera", Some("what people see in video calls.")))
                .child(
                    div()
                        .w(zpx(PREVIEW_W))
                        .h(zpx(PREVIEW_H))
                        .rounded(zpx(14.))
                        .overflow_hidden()
                        .bg(palette.inset)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(picture),
                )
                .when(cameras.is_empty() && self.cameras.is_some(), |c| c.child(div().text_sm().text_color(palette.idle).child("no cameras found.")))
                .child(list)
                .into_any_element(),
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn camera_card(&mut self, _prefs: &Prefs, _palette: &Palette, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    /// The same news as the update banner, and a way to ask.
    fn updates(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        use crate::store::UpdateState;
        let state = self.store.read(cx).update.clone();
        let (line, warn) = match &state {
            UpdateState::Checking => ("looking for a new version…".to_owned(), false),
            UpdateState::Available(r) => (format!("horo {} is out — this build updates from where you installed it", r.version), false),
            UpdateState::UpToDate => ("you're on the latest version~".to_owned(), false),
            UpdateState::Failed(e) => (format!("couldn't look for a new version: {e}"), true),
            UpdateState::Idle => ("checked automatically every few hours".to_owned(), false),
        };
        let action = match &state {
            UpdateState::Available(r) => {
                let url = r.url.clone();
                Button::new("update-open").small().primary().label("open release").on_click(move |_, _, cx| cx.open_url(&url))
            }
            _ => {
                let checking = state == UpdateState::Checking;
                Button::new("update-check")
                    .small()
                    .ghost()
                    .label(if checking { "checking…" } else { "check now" })
                    .disabled(checking)
                    .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.check_for_update(true, cx))))
            }
        };
        card(palette)
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(zpx(13.5)).text_color(palette.text).child("updates"))
                            .child(div().mt_1().text_xs().line_height(gpui::relative(1.5)).text_color(if warn { palette.idle } else { palette.text_tertiary }).child(line)),
                    )
                    .child(action),
            )
            .into_any_element()
    }

    fn about(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_4()
            .child(heading(palette, "about"))
            .child(
                card(palette)
                    .child(div().font_family(palette.heading_font.clone()).text_size(zpx(36.)).text_color(palette.accent).child("horo"))
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
            .child(self.updates(palette, cx))
            .into_any_element()
    }
}

/// The camera preview's size: 16:9.
const PREVIEW_W: f32 = 320.;
const PREVIEW_H: f32 = 180.;

/// A quiet line in the preview's place.
fn message(palette: &Palette, text: &str) -> AnyElement {
    div().p_4().text_xs().text_center().text_color(palette.text_tertiary).child(text.to_owned()).into_any_element()
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

        if section == SettingsSection::Voice {
            self.run_previews(cx);
            // The meter and the picture move.
            window.request_animation_frame();
        } else {
            self.stop_previews();
        }
        if section == SettingsSection::Security && self.security.is_none() {
            let store = self.store.clone();
            self.security = Some(cx.new(|cx| Security::new(store, window, cx)));
        }
        if section == SettingsSection::Packs && self.packs.is_none() {
            let store = self.store.clone();
            self.packs = Some(cx.new(|cx| PackSettings::new(store, window, cx)));
        }

        let nav = v_flex()
            .w(zpx(232.))
            .flex_none()
            .h_full()
            .pt(zpx(52.))
            .px_3()
            .gap(zpx(3.))
            .bg(palette.card)
            .border_r_1()
            .border_color(palette.border_subtle)
            .children(SettingsSection::ALL.into_iter().map(|entry| {
                let selected = entry == section;
                div()
                    .id(SharedString::from(format!("settings-{}", entry.label())))
                    .px_3()
                    .py_2()
                    .rounded(zpx(12.))
                    .cursor_pointer()
                    .font_family(palette.heading_font.clone())
                    .text_size(zpx(17.))
                    .text_color(if selected { palette.text } else { palette.text_secondary })
                    .when(selected, |d| d.bg(palette.raised))
                    .hover(|d| d.bg(palette.raised))
                    .child(entry.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.show_settings(Some(entry), cx))))
            }));

        let body: AnyElement = match (section, &session) {
            (SettingsSection::Account, Some(session)) => self.account(session, &palette, cx),
            (SettingsSection::Security, _) => self.security.clone().map(|s| s.into_any_element()).unwrap_or_else(|| div().into_any_element()),
            (SettingsSection::Voice, _) => {
                let prefs = self.store.read(cx).prefs.clone();
                self.voice(&prefs, &palette, cx)
            }
            (SettingsSection::Notifications, _) => {
                let prefs = self.store.read(cx).prefs.clone();
                self.notifications(&prefs, &palette, cx)
            }
            (SettingsSection::Packs, _) => self.packs.clone().map(|s| s.into_any_element()).unwrap_or_else(|| div().into_any_element()),
            (SettingsSection::About, _) => self.about(&palette, cx),
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
                    .pt(zpx(52.))
                    .pb_10()
                    .child(div().max_w(zpx(620.)).mx_auto().px_10().child(body)),
            )
            .child(
                div()
                    .absolute()
                    .top(zpx(40.))
                    .right(zpx(32.))
                    .child(Button::new("settings-close").ghost().label("close (esc)").on_click(cx.listener(|this, _, _, cx| this.close(cx)))),
            )
    }
}

fn heading(palette: &Palette, text: &str) -> gpui::Div {
    div().pt_2().font_family(palette.heading_font.clone()).text_size(zpx(22.)).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(text.to_owned()))
}

fn card(palette: &Palette) -> gpui::Div {
    v_flex().gap_3().p_4().rounded(zpx(16.)).bg(palette.card).border_1().border_color(palette.border_subtle)
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
