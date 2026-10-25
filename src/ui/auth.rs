//! Signing in, and the two session gates.
//!
//! The flow is the mobile app's (and its `NOTES-API.md`): SuperTokens in
//! header mode, then `GET /api/me` before anything else. Two claims can make
//! every request 403 while the password was right — two-factor and email
//! verification — and each gets its own screen rather than a dead window.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::{ApiClient, ApiError, SignInOutcome, normalise_base_url};
use crate::app_state::AppState;
use crate::assets::LOGO;
use crate::store::profiles::Profile;
use crate::store::vault;

pub enum AuthEvent {
    /// Signed in and `/api/me` answered: open the workspace.
    Ready,
    /// Signed in, but a gate stands between the session and the API.
    Gated(Gate),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gate {
    Mfa,
    EmailVerification,
}

// ------------------------------------------------------------------ sign in

pub struct SignInView {
    /// Signing back in to a profile we already have; otherwise a new one.
    existing: Option<Profile>,
    server: Entity<InputState>,
    email: Entity<InputState>,
    password: Entity<InputState>,
    busy: bool,
    error: Option<String>,
    can_cancel: bool,
    _subscriptions: Vec<Subscription>,
    _task: Option<Task<()>>,
}

impl EventEmitter<AuthEvent> for SignInView {}

impl SignInView {
    pub fn new(
        existing: Option<Profile>,
        can_cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let server = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("https://tasks.example.com");
            match &existing {
                Some(p) => state.default_value(p.base_url.clone()),
                None => state,
            }
        });
        let email = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("you@example.com");
            match existing.as_ref().and_then(|p| p.email.clone()) {
                Some(email) => state.default_value(email),
                None => state,
            }
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password")
        });

        let submit_on_enter = |this: &mut Self,
                               _: &Entity<InputState>,
                               e: &InputEvent,
                               _: &mut Window,
                               cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = e {
                this.submit(cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&server, window, submit_on_enter),
            cx.subscribe_in(&email, window, submit_on_enter),
            cx.subscribe_in(&password, window, submit_on_enter),
        ];

        // Focus the first field that needs typing.
        let first_empty = if existing.is_none() {
            server.clone()
        } else if existing.as_ref().and_then(|p| p.email.as_ref()).is_none() {
            email.clone()
        } else {
            password.clone()
        };
        first_empty.update(cx, |s, cx| s.focus(window, cx));

        Self {
            existing,
            server,
            email,
            password,
            busy: false,
            error: None,
            can_cancel,
            _subscriptions: subscriptions,
            _task: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let base_url = normalise_base_url(&self.server.read(cx).value());
        let email = self.email.read(cx).value().trim().to_owned();
        let password = self.password.read(cx).value().to_string();
        if base_url.is_empty() || email.is_empty() || password.is_empty() {
            self.error = Some("Server, email and password are all needed.".into());
            cx.notify();
            return;
        }

        let mut profile = match &self.existing {
            Some(p) if p.base_url == base_url => p.clone(),
            _ => Profile::new(&host_label(&base_url), &base_url),
        };
        profile.email = Some(email.clone());

        self.busy = true;
        self.error = None;
        cx.notify();

        let client = {
            let id = profile.id.clone();
            ApiClient::new(&base_url, Default::default(), move |tokens| {
                vault::save(&id, tokens)
            })
        };
        let worker = client.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { sign_in(&worker, &email, &password) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(signed_in) => this.finish(profile, client, signed_in, cx),
                    Err(message) => this.error = Some(message),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn finish(
        &mut self,
        mut profile: Profile,
        client: Arc<ApiClient>,
        signed_in: SignedIn,
        cx: &mut Context<Self>,
    ) {
        if let Some(me) = &signed_in.me {
            profile.user_id = Some(me.id.clone());
            profile.display_name = me.display_name.clone();
            if let Some(email) = &me.email {
                profile.email = Some(email.clone());
            }
        }
        let profile_id = profile.id.clone();
        AppState::update(cx, |state, _| {
            state.save_profile(profile);
            state.set_active(&profile_id);
            state.install_client(&profile_id, client);
        });
        cx.emit(match signed_in.gate {
            Some(gate) => AuthEvent::Gated(gate),
            None => AuthEvent::Ready,
        });
    }
}

struct SignedIn {
    me: Option<crate::api::models::Me>,
    gate: Option<Gate>,
}

/// Runs on a background thread: sign in, then `/api/me`, which both creates
/// the user row on first sight and reveals whether a gate is up.
fn sign_in(client: &ApiClient, email: &str, password: &str) -> Result<SignedIn, String> {
    match client.sign_in(email, password).map_err(|e| e.to_string())? {
        SignInOutcome::SignedIn => {}
        SignInOutcome::WrongCredentials => {
            return Err("That email and password don't match.".into());
        }
        SignInOutcome::NotAllowed(reason) => return Err(reason),
        SignInOutcome::FieldErrors(errors) if !errors.is_empty() => return Err(errors.join(" ")),
        SignInOutcome::FieldErrors(_) => return Err("Check the email and password.".into()),
    }
    match client.me() {
        Ok(me) => {
            client.sync_timezone(&me);
            Ok(SignedIn {
                me: Some(me),
                gate: None,
            })
        }
        Err(ApiError::MfaRequired) => Ok(SignedIn {
            me: None,
            gate: Some(Gate::Mfa),
        }),
        Err(ApiError::EmailVerificationRequired) => Ok(SignedIn {
            me: None,
            gate: Some(Gate::EmailVerification),
        }),
        Err(e) => Err(e.to_string()),
    }
}

fn host_label(base_url: &str) -> String {
    let rest = base_url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(base_url);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

impl Render for SignInView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let insecure = self.server.read(cx).value().trim().starts_with("http://");
        let returning = self.existing.is_some();

        card(
            &theme,
            v_flex()
                .gap_4()
                .child(heading(
                    &theme,
                    if returning {
                        "Sign back in"
                    } else {
                        "Sign in to ayeaye"
                    },
                    if returning {
                        "Your session ended. Your notes are waiting."
                    } else {
                        "Any ayeaye server: your company's, or your own."
                    },
                ))
                .child(labelled(&theme, "Server", Input::new(&self.server)))
                .when(insecure, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.warning)
                            .child("http:// sends your password unencrypted. Fine for localhost."),
                    )
                })
                .child(labelled(&theme, "Email", Input::new(&self.email)))
                .child(labelled(
                    &theme,
                    "Password",
                    Input::new(&self.password).mask_toggle(),
                ))
                .when_some(self.error.clone(), |this, error| {
                    this.child(error_box(&theme, error))
                })
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .when(self.can_cancel, |this| {
                            this.child(
                                Button::new("cancel").label("Cancel").ghost().on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(AuthEvent::Cancelled)),
                                ),
                            )
                        })
                        .child(
                            Button::new("sign-in")
                                .label("Sign in")
                                .primary()
                                .loading(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                        ),
                ),
        )
    }
}

// -------------------------------------------------------------------- gates

pub struct GateView {
    profile: Profile,
    client: Arc<ApiClient>,
    gate: Gate,
    code: Entity<InputState>,
    backup: bool,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
    _task: Option<Task<()>>,
}

pub enum GateEvent {
    Unlocked,
    SignOut,
}

impl EventEmitter<GateEvent> for GateView {}

impl GateView {
    pub fn new(profile: Profile, gate: Gate, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let client = AppState::update(cx, |state, _| state.client(&profile));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("123456"));
        code.update(cx, |c, cx| c.focus(window, cx));
        let subscriptions =
            vec![
                cx.subscribe_in(&code, window, |this, _, e: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = e {
                        this.submit(cx);
                    }
                }),
            ];
        Self {
            profile,
            client,
            gate,
            code,
            backup: false,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
            _task: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let code = self.code.read(cx).value().trim().replace(' ', "");
        if self.gate == Gate::Mfa && code.is_empty() {
            return;
        }
        let client = self.client.clone();
        let gate = self.gate;
        let backup = self.backup;
        self.busy = true;
        self.error = None;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match gate {
                        Gate::Mfa if backup => client.redeem_backup_code(&code)?,
                        Gate::Mfa => client.totp_challenge(&code)?,
                        // Verification is recorded against the account, but
                        // the access token carries the stale claim until a
                        // refresh — without it, "I've verified" loops forever.
                        Gate::EmailVerification => client.refresh_session()?,
                    }
                    let me = client.me()?;
                    client.sync_timezone(&me);
                    Ok::<_, ApiError>(me)
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(me) => {
                        let id = this.profile.id.clone();
                        AppState::update(cx, |state, _| {
                            state.update_profile(&id, |p| {
                                p.user_id = Some(me.id.clone());
                                p.display_name = me.display_name.clone();
                                if me.email.is_some() {
                                    p.email = me.email.clone();
                                }
                            })
                        });
                        cx.emit(GateEvent::Unlocked);
                    }
                    Err(ApiError::EmailVerificationRequired) => {
                        this.gate = Gate::EmailVerification;
                        this.error = Some(
                            "Not verified yet — open the link in the email, then try again.".into(),
                        );
                    }
                    Err(ApiError::MfaRequired) => {
                        this.gate = Gate::Mfa;
                        this.error = Some("That code didn't work. Try the current one.".into());
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for GateView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let who = self
            .profile
            .email
            .clone()
            .unwrap_or_else(|| self.profile.label.clone());
        let body = match self.gate {
            Gate::Mfa => v_flex()
                .gap_4()
                .child(heading(
                    &theme,
                    "Two-factor authentication",
                    if self.backup {
                        "Enter one of your backup codes. Each works once."
                    } else {
                        "Enter the 6-digit code from your authenticator app."
                    },
                ))
                .child(labelled(
                    &theme,
                    if self.backup { "Backup code" } else { "Code" },
                    Input::new(&self.code),
                ))
                .child(
                    Button::new("toggle-backup")
                        .label(if self.backup { "Use an authenticator code" } else { "Use a backup code" })
                        .link()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.backup = !this.backup;
                            cx.notify();
                        })),
                ),
            Gate::EmailVerification => v_flex().gap_4().child(heading(
                &theme,
                "Verify your email",
                "This server wants your address confirmed first. Open the link it emailed you, then come back.",
            )),
        };
        card(
            &theme,
            body.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{who} on {}", self.profile.host())),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(error_box(&theme, error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("gate-sign-out")
                            .label("Sign out")
                            .ghost()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GateEvent::SignOut))),
                    )
                    .child(
                        Button::new("gate-submit")
                            .label(match self.gate {
                                Gate::Mfa => "Verify",
                                Gate::EmailVerification => "I've verified it",
                            })
                            .primary()
                            .loading(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            ),
        )
    }
}

// ------------------------------------------------------------------- pieces

fn card(theme: &gpui_kit::component::Theme, content: impl IntoElement) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .bg(theme.background)
        .child(
            v_flex()
                .w(px(380.))
                .gap_5()
                .child(
                    h_flex()
                        .justify_center()
                        .child(Icon::new(LOGO).size(px(40.)).text_color(theme.foreground)),
                )
                .child(
                    div()
                        .p_6()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.sidebar)
                        .child(content),
                ),
        )
        .into_any_element()
}

fn heading(
    theme: &gpui_kit::component::Theme,
    title: &'static str,
    subtitle: &'static str,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(subtitle),
        )
        .into_any_element()
}

fn labelled(
    theme: &gpui_kit::component::Theme,
    label: &'static str,
    input: impl IntoElement,
) -> AnyElement {
    v_flex()
        .gap_1p5()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(input)
        .into_any_element()
}

fn error_box(theme: &gpui_kit::component::Theme, error: String) -> AnyElement {
    div()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .bg(theme.danger.opacity(0.12))
        .text_sm()
        .child(error)
        .into_any_element()
}
