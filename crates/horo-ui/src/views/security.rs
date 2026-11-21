//! This account's devices and its key backup, as the settings' security
//! pane in SettingsView.tsx has them: verify a device, turn backup on, or
//! restore from a recovery key. A panel of its own until settings exist.
//!
//! A new device can't read older encrypted messages until it either gets
//! verified by one that can, or opens the server's key backup with the
//! recovery key. Both end in horo-core fetching the keys and asking the open
//! timelines to decrypt again.

use gpui::{
    AnyElement, AppContext, ClipboardItem, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use horo_core::verification::{self, DeviceInfo, RecoveryState, RecoveryStatus};

use crate::{backend::Backend, store::Store, theme::Palette, verify::Stage, views::verification::scrim};

pub struct Security {
    store: Entity<Store>,
    devices: Option<Vec<DeviceInfo>>,
    recovery: Option<RecoveryStatus>,
    /// A recovery key just made, shown once.
    new_key: Option<SharedString>,
    restore: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    notice: Option<SharedString>,
    /// What the store said last, to refresh when trust changes.
    seen: (Option<String>, bool),
}

impl Security {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Security {
        let restore = cx.new(|cx| InputState::new(window, cx).placeholder("EsT…").masked(true));
        cx.subscribe_in(&restore, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.restore(window, cx);
            }
        })
        .detach();
        // A verification finishing, or the device's trust changing, changes
        // what this panel shows.
        cx.observe(&store, |this, store, cx| {
            let state = store.read(cx);
            let seen = (state.device_trust.clone(), state.verification.as_ref().is_some_and(|f| f.stage() == Stage::Done));
            if seen != this.seen {
                this.seen = seen;
                this.refresh(cx);
            }
            cx.notify();
        })
        .detach();
        let mut security = Security { store, devices: None, recovery: None, new_key: None, restore, busy: false, error: None, notice: None, seen: (None, false) };
        security.refresh(cx);
        security
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let devices = verification::own_devices(&core).await?;
                let recovery = verification::recovery_status(&core).await?;
                Ok::<_, horo_core::Error>((devices, recovery))
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok((devices, recovery))) => {
                        this.devices = Some(devices);
                        this.recovery = Some(recovery);
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn enable_backup(&mut self, cx: &mut Context<Self>) {
        self.begin(cx);
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { verification::enable_recovery(&*state.core().await?).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok(key)) => this.new_key = Some(key.into()),
                    Ok(Err(e)) => this.error = Some(format!("key backup: {e}").into()),
                    Err(e) => this.error = Some(format!("key backup: {e}").into()),
                }
                this.finish(cx);
            });
        })
        .detach();
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.restore.read(cx).value().trim().to_owned();
        if key.is_empty() || self.busy {
            return;
        }
        self.begin(cx);
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { verification::recover(&state.core().await?, &key).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(Ok(())) => {
                        this.restore.update(cx, |input, cx| input.set_value("", window, cx));
                        this.notice = Some("restored. your older messages will decrypt as their keys come down.".into());
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                this.finish(cx);
            });
        })
        .detach();
    }

    fn begin(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        self.notice = None;
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.busy = false;
        self.refresh(cx);
        cx.notify();
    }

    fn device_row(&self, device: &DeviceInfo, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let action: AnyElement = if device.is_verified {
            div().text_sm().text_color(palette.online).child("verified").into_any_element()
        } else if device.is_current {
            Button::new("verify-self")
                .ghost()
                .label("verify from another device")
                .disabled(self.busy)
                .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.start_verification(None, cx))))
                .into_any_element()
        } else {
            let id = device.device_id.clone();
            Button::new(SharedString::from(format!("verify-{id}")))
                .primary()
                .label("verify")
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.store.update(cx, |s, cx| s.start_verification(Some(id), cx))
                }))
                .into_any_element()
        };
        h_flex()
            .gap_3()
            .py_2()
            .border_t_1()
            .border_color(palette.border_subtle)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().truncate().child(device.display_name.clone().unwrap_or_else(|| device.device_id.clone())))
                            .when(device.is_current, |row| row.child(div().text_xs().text_color(palette.accent).child("this one"))),
                    )
                    .child(div().text_xs().text_color(palette.text_tertiary).child(device.device_id.clone())),
            )
            .child(action)
            .into_any_element()
    }

    fn backup(&self, recovery: &RecoveryStatus, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let title = match recovery.state {
            RecoveryState::Enabled => "recovery is on",
            RecoveryState::Incomplete => "recovery is half set up",
            _ => "recovery is off",
        };
        let explanation = if recovery.state == RecoveryState::Incomplete {
            div()
                .p_3()
                .rounded(px(12.))
                .bg(Hsla { a: 0.08, ..palette.idle })
                .border_1()
                .border_color(Hsla { a: 0.35, ..palette.idle })
                .text_sm()
                .text_color(palette.text_secondary)
                .child(div().text_color(palette.idle).font_weight(FontWeight::SEMIBOLD).child("this is why your old messages are unreadable."))
                .child("you have a key backup on the server, but this device doesn't have the key to open it. enter your recovery key below and your history will decrypt.")
        } else {
            div().text_sm().text_color(palette.text_secondary).child(
                "key backup stores your message keys on the server, encrypted with a recovery key only you hold. without it, losing this device means losing your encrypted history.",
            )
        };

        let key_or_button: Option<AnyElement> = if let Some(key) = &self.new_key {
            let copy = key.clone();
            Some(
                v_flex()
                    .gap_2()
                    .p_4()
                    .rounded(px(12.))
                    .bg(palette.inset)
                    .border_1()
                    .border_color(palette.accent)
                    .child(div().text_xs().text_color(palette.text_tertiary).child("write this down — you'll only see it once"))
                    .child(div().text_color(palette.accent).child(key.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Button::new("copy-key").ghost().label("copy").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()))))
                            .child(Button::new("saved-key").ghost().label("i've saved it").on_click(cx.listener(|this, _, _, cx| {
                                this.new_key = None;
                                cx.notify();
                            }))),
                    )
                    .into_any_element(),
            )
        } else if recovery.state != RecoveryState::Enabled && recovery.state != RecoveryState::Incomplete {
            Some(
                Button::new("enable-backup")
                    .primary()
                    .label("turn on key backup")
                    .loading(self.busy)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.enable_backup(cx)))
                    .into_any_element(),
            )
        } else {
            None
        };

        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_xs().text_color(palette.text_tertiary).child(if recovery.cross_signing_ready {
                        "cross-signing ready"
                    } else {
                        "cross-signing not set up"
                    })),
            )
            .child(explanation)
            .children(key_or_button)
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().text_color(palette.text_secondary).child("restore from a recovery key"))
                    .child(
                        h_flex().gap_2().child(div().flex_1().child(Input::new(&self.restore).mask_toggle())).child(
                            Button::new("restore")
                                .primary()
                                .label("restore")
                                .loading(self.busy)
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| this.restore(window, cx))),
                        ),
                    )
                    .child(div().text_xs().text_color(palette.text_tertiary).child("the key you saved when you turned on backup.")),
            )
            .into_any_element()
    }
}

impl Render for Security {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let heading = |text: &'static str| {
            div().font_family(palette.heading_font.clone()).text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(palette.text).child(text)
        };
        let loading = || div().py_4().text_sm().text_color(palette.text_tertiary).child("loading…");

        let devices: AnyElement = match &self.devices {
            Some(devices) => {
                let rows: Vec<AnyElement> = devices.iter().map(|d| self.device_row(d, &palette, cx)).collect();
                v_flex().children(rows).into_any_element()
            }
            None => loading().into_any_element(),
        };
        let backup: AnyElement = match &self.recovery {
            Some(recovery) => self.backup(recovery, &palette, cx),
            None => loading().into_any_element(),
        };
        let verification_error = self.store.read(cx).verification_error.clone();

        scrim(&palette).child(
            v_flex()
                .id("security")
                .w(px(560.))
                .max_h(px(640.))
                .overflow_y_scroll()
                .p_6()
                .gap_4()
                .rounded(px(20.))
                .bg(palette.card)
                .border_1()
                .border_color(palette.border_strong)
                .shadow_lg()
                .child(
                    h_flex()
                        .child(
                            div()
                                .flex_1()
                                .font_family(palette.heading_font.clone())
                                .text_size(px(28.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("security"),
                        )
                        .child(
                            Button::new("security-close")
                                .ghost()
                                .label("close")
                                .on_click(cx.listener(|this, _, _, cx| this.store.update(cx, |s, cx| s.set_security_open(false, cx)))),
                        ),
                )
                .child(heading("your devices"))
                .child(div().text_sm().text_color(palette.text_secondary).child(
                    "verifying a device proves it's really yours and lets your encrypted history reach it.",
                ))
                .child(devices)
                .children(verification_error.map(|e| div().text_sm().text_color(palette.danger).child(e)))
                .child(heading("key backup"))
                .child(backup)
                .children(self.notice.clone().map(|n| div().text_sm().text_color(palette.online).child(n)))
                .children(self.error.clone().map(|e| div().text_sm().text_color(palette.danger).child(e))),
        )
    }
}
