//! Signing in, or making an account, as LoginScreen.tsx: first the
//! homeserver (asked how it signs people in), then a password, single
//! sign-on, or sign-up one stage at a time — each stage the server asks
//! for (an invite token, its terms, an email link, a captcha in the
//! browser) shown on its own.

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, img, prelude::FluentBuilder, px,
};
use gpui_component::{
    Disableable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use horo_core::{
    dto::{HomeserverInfo, RegisterAuth, RegisterChallenge, RegisterEmailToken, RegisterStageKind, RegisterStep, SessionInfo},
    matrix::auth,
};

use crate::{
    backend::Backend,
    registration::next_stage,
    store::{Phase, Store},
    theme::{Palette, corner, space, text},
    views::voice::icon,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Server,
    Credentials,
    Register,
}

pub struct Login {
    store: Entity<Store>,
    stage: Stage,
    server: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    confirm: Entity<InputState>,
    token: Entity<InputState>,
    email: Entity<InputState>,
    /// What the homeserver said about itself.
    info: Option<HomeserverInfo>,
    /// Sign-up under way: what the server still wants.
    challenge: Option<RegisterChallenge>,
    agreed: bool,
    /// The verification email in flight, and how many times it's gone.
    email_token: Option<RegisterEmailToken>,
    send_attempt: u32,
    busy: bool,
    error: Option<SharedString>,
}

/// What the server wants next for a new account, as the screen needs it.
#[derive(Clone, PartialEq)]
enum Next {
    /// The username and passwords, before the server has asked anything.
    Details,
    Token,
    Terms,
    Email,
    Fallback(Option<String>),
    Dummy,
    /// Only flows horo can't do (a phone number).
    Impossible,
}

impl Login {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Login {
        let input = |placeholder: &str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            cx.new(move |cx| InputState::new(window, cx).placeholder(placeholder).masked(masked))
        };
        let server = cx.new(|cx| InputState::new(window, cx).placeholder("matrix.org").default_value("matrix.org"));
        let username = input("username", false, window, cx);
        let password = input("password", true, window, cx);
        let confirm = input("password, again", true, window, cx);
        let token = input("registration token", false, window, cx);
        let email = input("you@example.com", false, window, cx);
        // Enter in any field does what the button would.
        for field in [&server, &username, &password, &confirm, &token, &email] {
            cx.subscribe_in(field, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                // Typing changes whether the button can be pressed.
                InputEvent::Change => cx.notify(),
                _ => {}
            })
            .detach();
        }
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        server.update(cx, |s, cx| s.focus(window, cx));
        Login {
            store,
            stage: Stage::Server,
            server,
            username,
            password,
            confirm,
            token,
            email,
            info: None,
            challenge: None,
            agreed: false,
            email_token: None,
            send_attempt: 0,
            busy: false,
            error: None,
        }
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_owned()
    }

    fn server_name(&self, cx: &App) -> String {
        Self::value(&self.server, cx)
    }

    fn next(&self) -> Next {
        match &self.challenge {
            None => Next::Details,
            Some(c) => match next_stage(c) {
                None => Next::Impossible,
                Some(stage) => match stage.kind {
                    RegisterStageKind::RegistrationToken => Next::Token,
                    RegisterStageKind::Terms => Next::Terms,
                    RegisterStageKind::Email => Next::Email,
                    RegisterStageKind::Fallback => Next::Fallback(stage.fallback_url.clone()),
                    RegisterStageKind::Dummy => Next::Dummy,
                    RegisterStageKind::Unsupported => Next::Impossible,
                },
            },
        }
    }

    /// Switch screens, dropping any half-finished sign-up.
    fn go_to(&mut self, stage: Stage, window: &mut Window, cx: &mut Context<Self>) {
        self.stage = stage;
        self.challenge = None;
        self.agreed = false;
        self.email_token = None;
        self.send_attempt = 0;
        self.error = None;
        self.token.update(cx, |s, cx| s.set_value("", window, cx));
        let focus = if stage == Stage::Server { &self.server } else { &self.username };
        focus.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Run `job` on the core's runtime with the screen busy; `done` gets
    /// its answer, a failure is said under the form.
    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        job: impl std::future::Future<Output = horo_core::Result<T>> + Send + 'static,
        done: impl FnOnce(&mut Login, T, &mut Context<Login>) + 'static,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let task = Backend::global(cx).run(job);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(Ok(value)) => done(this, value, cx),
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn signed_in(&mut self, info: SessionInfo, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.signed_in(info, cx));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let server = self.server_name(cx);
        match self.stage {
            Stage::Server => self.run(cx, async move { auth::discover(&server).await }, |this, info, cx| {
                this.info = Some(info);
                this.stage = Stage::Credentials;
                cx.notify();
            }),
            Stage::Credentials => {
                let (username, password) = (Self::value(&self.username, cx), self.password.read(cx).value().to_string());
                let backend = Backend::global(cx);
                let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
                self.run(cx, async move { auth::login_password(host, &state, &dir, &server, &username, &password).await }, |this, info, cx| this.signed_in(info, cx));
            }
            Stage::Register => match self.next() {
                Next::Details => {
                    if self.password.read(cx).value() != self.confirm.read(cx).value() {
                        self.error = Some("those passwords don't match".into());
                        cx.notify();
                        return;
                    }
                    self.register(None, cx);
                }
                Next::Token => {
                    let token = Self::value(&self.token, cx);
                    self.register(Some(RegisterAuth::RegistrationToken { token }), cx);
                }
                Next::Terms => self.register(Some(RegisterAuth::Terms), cx),
                Next::Email => match self.email_token.clone() {
                    None => self.send_email(false, cx),
                    Some(t) => self.register(Some(RegisterAuth::Email { sid: t.sid, client_secret: t.client_secret }), cx),
                },
                Next::Fallback(_) => self.register(Some(RegisterAuth::Fallback), cx),
                Next::Dummy => self.register(Some(RegisterAuth::Dummy), cx),
                Next::Impossible => {}
            },
        }
        let _ = window;
    }

    fn can_submit(&self, cx: &App) -> bool {
        let filled = |input: &Entity<InputState>| !input.read(cx).value().trim().is_empty();
        match self.stage {
            Stage::Server => filled(&self.server),
            Stage::Credentials => filled(&self.username) && !self.password.read(cx).value().is_empty(),
            Stage::Register => match self.next() {
                Next::Details => filled(&self.username) && !self.password.read(cx).value().is_empty() && !self.confirm.read(cx).value().is_empty(),
                Next::Token => filled(&self.token),
                Next::Terms => self.agreed,
                Next::Email => self.email_token.is_some() || self.email.read(cx).value().contains('@'),
                Next::Impossible => false,
                _ => true,
            },
        }
    }

    /// One turn of sign-up. A stage that asks nothing is answered here
    /// rather than shown (a few times at most, in case a server keeps on).
    fn register(&mut self, answer: Option<RegisterAuth>, cx: &mut Context<Self>) {
        let server = self.server_name(cx);
        let (username, password) = (Self::value(&self.username, cx), self.password.read(cx).value().to_string());
        let session = self.challenge.as_ref().and_then(|c| c.session.clone());
        let backend = Backend::global(cx);
        let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
        let job = async move {
            let mut step = auth::register(host.clone(), &state, &dir, &server, &username, &password, session, answer).await?;
            for _ in 0..3 {
                let RegisterStep::NeedsAuth { challenge } = &step else { break };
                if challenge.error.is_some() || next_stage(challenge).map(|s| s.kind) != Some(RegisterStageKind::Dummy) {
                    break;
                }
                let session = challenge.session.clone();
                step = auth::register(host.clone(), &state, &dir, &server, &username, &password, session, Some(RegisterAuth::Dummy)).await?;
            }
            Ok(step)
        };
        self.run(cx, job, |this, step, cx| match step {
            RegisterStep::Done { session } => this.signed_in(session, cx),
            RegisterStep::NeedsAuth { challenge } => {
                this.error = challenge.error.clone().map(Into::into);
                this.challenge = Some(challenge);
            }
        });
    }

    /// Send (or, `resend`, send again) the sign-up verification email.
    fn send_email(&mut self, resend: bool, cx: &mut Context<Self>) {
        let (server, email) = (self.server_name(cx), Self::value(&self.email, cx));
        let attempt = if resend { self.send_attempt + 1 } else { 1 };
        let secret = if resend { self.email_token.as_ref().map(|t| t.client_secret.clone()) } else { None };
        self.run(cx, async move { auth::request_register_email(&server, &email, secret, attempt).await }, move |this, token, _| {
            this.email_token = Some(token);
            this.send_attempt = attempt;
        });
    }

    fn sign_in_with_sso(&mut self, provider: Option<String>, cx: &mut Context<Self>) {
        let server = self.server_name(cx);
        let backend = Backend::global(cx);
        let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
        self.run(cx, async move { auth::login_sso(host, &state, &dir, &server, provider).await }, |this, info, cx| this.signed_in(info, cx));
    }
}

/// A rounded field with its icon, as the WebView build's `Field`.
fn field(input: &Entity<InputState>, icon_path: &'static str, masked: bool, palette: &Palette) -> impl IntoElement {
    h_flex()
        .gap(space(10.))
        .px(space(16.))
        .py(space(6.))
        .rounded_full()
        .bg(palette.inset)
        .border_1()
        .border_color(palette.border_subtle)
        .child(icon(icon_path).size(px(14.)).text_color(palette.text_tertiary))
        .child(div().flex_1().text_size(text(14.)).child(Input::new(input).appearance(false).when(masked, |i| i.mask_toggle())))
}

fn hint(words: impl Into<SharedString>, palette: &Palette) -> impl IntoElement {
    div().mb(px(10.)).text_size(text(12.5)).line_height(gpui::relative(1.5)).text_color(palette.text_secondary).child(words.into())
}

/// A small link-coloured button in the mono face.
fn text_button(id: &'static str, words: &'static str, enabled: bool, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .font_family(palette.mono_font.clone())
        .text_size(text(12.))
        .text_color(palette.accent_quaternary)
        .when(enabled, |d| d.cursor_pointer())
        .when(!enabled, |d| d.opacity(0.5))
        .child(words)
}

impl Login {
    fn register_body(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let email = Self::value(&self.email, cx);
        match self.next() {
            Next::Details => v_flex()
                .gap(px(10.))
                .child(field(&self.username, "icons/user.svg", false, palette))
                .child(field(&self.password, "icons/lock.svg", true, palette))
                .child(field(&self.confirm, "icons/lock.svg", true, palette))
                .into_any_element(),
            Next::Token => v_flex()
                .child(hint("this server is invite-only. paste the registration token you were given.", palette))
                .child(field(&self.token, "icons/ticket.svg", false, palette))
                .into_any_element(),
            Next::Terms => {
                let policies = self.challenge.as_ref().map(|c| c.policies.clone()).unwrap_or_default();
                let empty = policies.is_empty();
                v_flex()
                    .child(hint("before it makes your account, the server asks you to accept:", palette))
                    .child(
                        v_flex()
                            .gap(space(6.))
                            .mt(px(4.))
                            .mb(px(14.))
                            .children(policies.into_iter().enumerate().map(|(i, policy)| {
                                let url = policy.url.clone();
                                h_flex()
                                    .id(("policy", i))
                                    .gap(space(8.))
                                    .cursor_pointer()
                                    .text_size(text(13.))
                                    .text_color(palette.accent_quaternary)
                                    .child(icon("icons/external-link.svg").size(px(13.)))
                                    .child(policy.name)
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                            }))
                            .when(empty, |d| d.child(hint("the server's terms of service", palette))),
                    )
                    .child(Checkbox::new("agree").checked(self.agreed).label("i agree").on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.agreed = *checked;
                        cx.notify();
                    })))
                    .into_any_element()
            }
            Next::Email if self.email_token.is_none() => v_flex()
                .child(hint("this server wants an email address for your account. we'll send it a link.", palette))
                .child(field(&self.email, "icons/mail.svg", false, palette))
                .into_any_element(),
            Next::Email => {
                let busy = self.busy;
                v_flex()
                    .child(hint(format!("check your inbox at {email}, click the link in the email, then press continue."), palette))
                    .child(
                        h_flex()
                            .gap(space(14.))
                            .child(text_button("email-again", "send it again", !busy, palette).on_click(cx.listener(move |this, _, _, cx| {
                                if !busy {
                                    this.send_email(true, cx)
                                }
                            })))
                            .child(text_button("email-change", "use a different address", !busy, palette).on_click(cx.listener(|this, _, _, cx| {
                                this.email_token = None;
                                this.send_attempt = 0;
                                this.error = None;
                                cx.notify();
                            }))),
                    )
                    .into_any_element()
            }
            Next::Fallback(url) => v_flex()
                .child(hint("the server wants a quick check that you're human. it opens in your browser — come back here once it says you're done.", palette))
                .child(Button::new("fallback-open").ghost().w_full().label("open in browser").disabled(self.busy || url.is_none()).on_click(move |_, _, cx| {
                    if let Some(url) = &url {
                        cx.open_url(url);
                    }
                }))
                .into_any_element(),
            Next::Dummy => hint("the server needs one more go — press continue.", palette).into_any_element(),
            Next::Impossible => div()
                .text_size(text(13.))
                .line_height(gpui::relative(1.5))
                .text_color(palette.idle)
                .child("this server needs a phone number to sign up, which horo can't do yet. sign up on the server's website, then sign in here.")
                .into_any_element(),
        }
    }

    fn credentials_body(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(info) = self.info.clone() else { return div().into_any_element() };
        let mut body = v_flex();
        if info.supports_password {
            body = body.gap(px(10.)).child(field(&self.username, "icons/user.svg", false, palette)).child(field(&self.password, "icons/lock.svg", true, palette));
        }
        if info.supports_sso {
            let providers: Vec<(Option<String>, String)> = if info.sso_providers.is_empty() {
                vec![(None, "single sign-on".into())]
            } else {
                info.sso_providers.iter().map(|p| (Some(p.id.clone()), p.name.clone())).collect()
            };
            body = body.child(
                v_flex()
                    .when(info.supports_password, |d| {
                        d.mt(px(6.)).child(
                            h_flex()
                                .gap(space(10.))
                                .mt(px(4.))
                                .mb(px(14.))
                                .child(div().flex_1().h(px(1.)).bg(palette.border_subtle))
                                .child(div().text_size(text(11.)).text_color(palette.text_tertiary).child("or"))
                                .child(div().flex_1().h(px(1.)).bg(palette.border_subtle)),
                        )
                    })
                    .gap(space(8.))
                    .children(providers.into_iter().enumerate().map(|(i, (id, name))| {
                        Button::new(("sso", i))
                            .ghost()
                            .w_full()
                            .label(format!("continue with {}", name.to_lowercase()))
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.sign_in_with_sso(id.clone(), cx)))
                    })),
            );
        }
        if !info.supports_password && !info.supports_sso {
            body = body.child(div().text_size(text(13.)).text_color(palette.idle).child("this server doesn't offer a sign-in method horo supports yet."));
        }
        body.into_any_element()
    }
}

impl Render for Login {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        // A saved session that failed to come back says why here too.
        let restoring = matches!(self.store.read(cx).phase, Phase::Restoring);
        let error = self.error.clone().or_else(|| self.store.read(cx).error.clone());

        let caption = match self.stage {
            Stage::Server => "homeserver",
            Stage::Credentials => "sign in",
            Stage::Register => "new account",
        };
        let next = self.next();
        let submit_label = match (self.stage, &next) {
            (Stage::Server, _) => "next~",
            (Stage::Credentials, _) => "sign in~",
            (Stage::Register, Next::Details) => "create account~",
            (Stage::Register, Next::Fallback(_)) => "i've done it~",
            (Stage::Register, Next::Email) if self.email_token.is_none() => "send link~",
            _ => "continue~",
        };
        let show_submit = match self.stage {
            Stage::Server => true,
            Stage::Credentials => self.info.as_ref().is_some_and(|i| i.supports_password),
            Stage::Register => next != Next::Impossible,
        };
        let can_submit = !self.busy && !restoring && self.can_submit(cx);

        let body: AnyElement = match self.stage {
            Stage::Server => v_flex()
                .child(field(&self.server, "icons/globe.svg", false, &palette))
                .child(
                    div()
                        .mt(px(8.))
                        .text_size(text(12.))
                        .line_height(gpui::relative(1.5))
                        .text_color(palette.text_tertiary)
                        .child("where your account lives. we'll ask it how to sign you in."),
                )
                .into_any_element(),
            stage => {
                let url = self.info.as_ref().map(|i| i.homeserver_url.clone()).unwrap_or_default();
                v_flex()
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .mb(px(12.))
                            .font_family(palette.mono_font.clone())
                            .text_size(text(11.))
                            .text_color(palette.text_tertiary)
                            .child(icon("icons/circle-check.svg").size(px(13.)).text_color(palette.accent))
                            .child(div().flex_1().min_w_0().truncate().child(url))
                            .child(text_button("change-server", "change", true, &palette).on_click(cx.listener(|this, _, window, cx| this.go_to(Stage::Server, window, cx)))),
                    )
                    .child(if stage == Stage::Register { self.register_body(&palette, cx) } else { self.credentials_body(&palette, cx) })
                    .into_any_element()
            }
        };

        let switch = match self.stage {
            Stage::Credentials if self.info.as_ref().is_some_and(|i| i.supports_password) => Some(("new here?", "create an account", Stage::Register)),
            Stage::Register if self.challenge.is_some() => Some(("wrong details?", "start over", Stage::Register)),
            Stage::Register => Some(("already have one?", "sign in", Stage::Credentials)),
            _ => None,
        };

        div().size_full().flex().items_center().justify_center().bg(palette.app).child(
            v_flex()
                .w(px(396.))
                .p(space(30.))
                .rounded(corner(28.))
                .bg(palette.card)
                .border_1()
                .border_color(palette.border_subtle)
                .shadow_lg()
                .child(img(crate::views::shell::logo()).w(px(40. * 830.3 / 295.9)).h(px(40.)))
                .child(div().mt(px(10.)).text_size(text(13.5)).text_color(palette.text_secondary).child("a cute matrix client~"))
                .child(
                    v_flex()
                        .mt(px(26.))
                        .child(div().mb(px(8.)).text_size(text(11.)).font_weight(FontWeight::BOLD).text_color(palette.text_tertiary).child(caption))
                        .child(body),
                )
                .children(error.map(|e| {
                    div()
                        .mt(px(16.))
                        .px(space(14.))
                        .py(space(10.))
                        .rounded(corner(14.))
                        .bg(gpui::Hsla { a: 0.12, ..palette.danger })
                        .border_1()
                        .border_color(gpui::Hsla { a: 0.35, ..palette.danger })
                        .text_size(text(12.5))
                        .line_height(gpui::relative(1.5))
                        .text_color(palette.danger)
                        .child(e)
                }))
                .when(show_submit, |d| {
                    d.child(
                        h_flex().mt(px(22.)).child(
                            Button::new("login-submit")
                                .primary()
                                .w_full()
                                .label(if self.busy { "one moment…" } else { submit_label })
                                .loading(self.busy)
                                .disabled(!can_submit)
                                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                        ),
                    )
                })
                .children(switch.map(|(prompt, action, target)| {
                    h_flex()
                        .mt(px(14.))
                        .gap_1()
                        .text_size(text(12.5))
                        .text_color(palette.text_tertiary)
                        .child(prompt)
                        .child(
                            div()
                                .id("login-switch")
                                .cursor_pointer()
                                .text_color(palette.accent_quaternary)
                                .child(action)
                                .on_click(cx.listener(move |this, _, window, cx| this.go_to(target, window, cx))),
                        )
                }))
                .child(
                    h_flex()
                        .mt(px(20.))
                        .gap(space(8.))
                        .font_family(palette.mono_font.clone())
                        .text_size(text(10.5))
                        .text_color(palette.text_tertiary)
                        .child(icon("icons/shield-check.svg").size(px(12.)).text_color(palette.accent))
                        .child("keys stay on this device, in your system keychain"),
                ),
        )
    }
}
