//! Emoji (SAS) verification: the dialog, the "new device wants in~" strip
//! for an incoming request, and the strip that says this device can't read
//! its history yet. As VerificationModal.tsx does it.
//!
//! The point is that both people compare the same seven pictures out of
//! band, so the copy is plain about what "they match" means: someone who
//! clicks through without looking has verified nothing.

use gpui::{
    AnyElement, App, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use horo_core::verification::RequestState;

use crate::{
    store::{SettingsSection, Store, VerifyAction},
    theme::Palette,
    verify::Stage,
};

/// The dialog over the whole window, while there's a verification to show
/// (other than an incoming one nobody has accepted yet: that's the strip).
pub fn dialog(store: &Entity<Store>, palette: &Palette, cx: &App) -> Option<AnyElement> {
    let state = store.read(cx);
    let flow = state.verification.as_ref()?;
    if is_unanswered(flow) {
        return None;
    }
    let stage = flow.stage();
    let who = flow.who();
    let other = flow.request.other_user_id.clone();

    let title = match stage {
        Stage::Done => "verified~".to_owned(),
        Stage::Cancelled => "verification stopped".to_owned(),
        _ => format!("verify {who}"),
    };

    let body: AnyElement = match stage {
        Stage::Done => outcome(
            palette,
            palette.online,
            "all good",
            if flow.request.is_self_verification {
                "your devices trust each other now, and your message history will follow you.".to_owned()
            } else {
                format!("you and {who} are verified. messages between you are now marked as trusted.")
            },
        ),
        Stage::Cancelled => outcome(palette, palette.danger, "not verified", flow.cancel_body()),
        Stage::Compare => {
            let emoji = flow.sas.as_ref().and_then(|s| s.emoji.clone()).unwrap_or_default();
            v_flex()
                .gap_4()
                .child(div().text_sm().text_color(palette.text_secondary).child(format!(
                    "compare these with {who}. they should be the same, in the same order, on both screens."
                )))
                .child(h_flex().flex_wrap().gap_2().justify_center().children(emoji.into_iter().map(|e| {
                    v_flex()
                        .w(px(84.))
                        .py_3()
                        .gap_1()
                        .items_center()
                        .rounded(px(14.))
                        .bg(palette.raised)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .child(div().text_size(px(30.)).child(e.symbol))
                        .child(div().text_xs().text_color(palette.text_tertiary).child(e.description.to_lowercase()))
                })))
                .into_any_element()
        }
        Stage::Confirmed => waiting(palette, "waiting for them to confirm…"),
        Stage::Waiting if flow.request.we_started => waiting(palette, "waiting for them to accept…"),
        Stage::Waiting => waiting(palette, "accept to see the emoji to compare"),
    };

    let act = |action: VerifyAction| {
        let store = store.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| store.update(cx, |s, cx| s.act_on_verification(action, cx))
    };
    let close = {
        let store = store.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| store.update(cx, |s, cx| s.close_verification(cx))
    };
    let buttons = h_flex().gap_2().justify_end().map(|row| match stage {
        Stage::Done | Stage::Cancelled => row.child(Button::new("verify-close").primary().label("close").on_click(close)),
        Stage::Compare => row
            .child(Button::new("verify-mismatch").ghost().label("they don't match").on_click(act(VerifyAction::Mismatch)))
            .child(Button::new("verify-confirm").primary().label("they match~").on_click(act(VerifyAction::Confirm))),
        _ => row
            .child(Button::new("verify-cancel").ghost().label("cancel").on_click(act(VerifyAction::Cancel)))
            .when(!flow.request.we_started && flow.request.state == RequestState::Requested, |row| {
                row.child(Button::new("verify-accept").primary().label("verify").on_click(act(VerifyAction::Accept)))
            }),
    });

    let card = v_flex()
        .w(px(440.))
        .p_6()
        .gap_5()
        .rounded(px(20.))
        .bg(palette.card)
        .border_1()
        .border_color(palette.border_strong)
        .shadow_lg()
        .child(
            v_flex()
                .child(div().font_family(palette.heading_font.clone()).text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().text_color(palette.text_tertiary).child(other)),
        )
        .child(body)
        .children(state.verification_error.clone().map(|e| div().text_sm().text_color(palette.danger).child(e)))
        .child(buttons);

    Some(scrim(palette).child(card).into_any_element())
}

/// The strips at the top of the room pane: an incoming request to answer,
/// or, failing that, this device not being trusted yet.
pub fn banner(store: &Entity<Store>, palette: &Palette, cx: &App) -> Option<AnyElement> {
    let state = store.read(cx);
    if let Some(flow) = state.verification.as_ref().filter(|f| is_unanswered(f)) {
        let who = if flow.request.is_self_verification { "another of your devices".to_owned() } else { flow.who() };
        let (accept, later) = (store.clone(), store.clone());
        return Some(strip(
            palette,
            "new device wants in~",
            format!("{who} asked to verify. verify it so your messages stay readable everywhere."),
            vec![
                Button::new("banner-verify")
                    .primary()
                    .label("verify")
                    .on_click(move |_, _, cx| accept.update(cx, |s, cx| s.act_on_verification(VerifyAction::Accept, cx)))
                    .into_any_element(),
                Button::new("banner-later")
                    .ghost()
                    .label("later")
                    .on_click(move |_, _, cx| later.update(cx, |s, cx| s.close_verification(cx)))
                    .into_any_element(),
            ],
        ));
    }
    if state.device_trust.as_deref() == Some("unverified") && state.verification.is_none() && state.settings.is_none() {
        let open = store.clone();
        return Some(strip(
            palette,
            "this device isn't verified yet",
            "older encrypted messages stay unreadable until you verify it from another device or enter your recovery key.".to_owned(),
            vec![
                Button::new("banner-security")
                    .primary()
                    .label("set it up")
                    .on_click(move |_, _, cx| open.update(cx, |s, cx| s.show_settings(Some(SettingsSection::Security), cx)))
                    .into_any_element(),
            ],
        ));
    }
    None
}

/// An incoming request nobody has said yes to: the strip asks, not the dialog.
fn is_unanswered(flow: &crate::verify::Flow) -> bool {
    !flow.request.we_started && flow.request.state == RequestState::Requested && flow.sas.is_none()
}

/// The dimmed layer under a dialog. It swallows clicks, so nothing behind
/// it can be pressed while the dialog is up.
pub fn scrim(palette: &Palette) -> gpui::Div {
    div().occlude().absolute().inset_0().flex().items_center().justify_center().bg(Hsla { a: 0.7, ..palette.app })
}

fn strip(palette: &Palette, title: &str, text: String, buttons: Vec<AnyElement>) -> AnyElement {
    h_flex()
        .mx_4()
        .mt_3()
        .gap_3()
        .px_4()
        .py_3()
        .rounded(px(16.))
        .bg(palette.raised)
        .border_1()
        .border_color(Hsla { a: 0.35, ..palette.accent })
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(title.to_owned())))
                .child(div().text_sm().text_color(palette.text_secondary).child(text)),
        )
        .child(h_flex().gap_2().flex_none().children(buttons))
        .into_any_element()
}

fn outcome(palette: &Palette, colour: Hsla, title: &str, body: String) -> AnyElement {
    v_flex()
        .gap_1()
        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(colour).child(SharedString::from(title.to_owned())))
        .child(div().text_sm().text_color(palette.text_secondary).child(body))
        .into_any_element()
}

fn waiting(palette: &Palette, text: &str) -> AnyElement {
    div().py_5().text_sm().text_color(palette.text_secondary).child(SharedString::from(text.to_owned())).into_any_element()
}
