//! Your account in a tab — everything the web app's Account page does:
//! how you appear to colleagues (name, status line, working hours, out of
//! office), your digests and timer check-ins, password and two-factor,
//! access tokens and connected apps, and where each notification goes.
//!
//! The account belongs to the person, not to an organisation, so this needs
//! only the session. Each control is one request; secrets the server shows
//! once (a token, a webhook's signing secret, backup codes) are shown here
//! once, with a copy button, and gone when dismissed.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use crate::api::models::*;
use crate::api::{ApiClient, ApiError, ApiResult};
use crate::assets::Lucide;
use crate::people::hours::{WEEKDAYS, grid, square};
use crate::session::Session;
use crate::tasks::view::{card, section_title};
use crate::ui::format::ago_long;
use crate::ui::item::{Item, ItemEvent};
use crate::ui::widgets::{parse_day, picker};
use gpui_kit::Task;

pub enum AccountEvent {
    /// Name or status changed: the title bar shows the name.
    MeChanged(Me),
    AuthFailed(ApiError),
}

impl EventEmitter<ItemEvent> for AccountView {}
impl EventEmitter<AccountEvent> for AccountView {}

/// Settings not yet saved; the Save button sends them all at once.
#[derive(Clone, Debug, Default, PartialEq)]
struct Prefs {
    daily_summary: bool,
    summary_hour: i64,
    weekly_report: bool,
    check_ins: String,
    check_ins_per_day: i64,
}

impl Prefs {
    fn of(me: &Me) -> Self {
        Self {
            daily_summary: me.daily_summary_enabled,
            summary_hour: me.daily_summary_hour,
            weekly_report: me.weekly_report_enabled,
            check_ins: me.timer_check_ins.clone(),
            check_ins_per_day: me.timer_check_ins_per_day.max(1),
        }
    }
}

/// Two-factor set-up in progress: the secret, its QR, the code box.
struct Enrolling {
    device: TotpDevice,
    qr: Option<Arc<Image>>,
}

/// Everything loaded at once, best-effort apart from `/me`.
struct Loaded {
    me: Me,
    mfa: Option<MfaStatus>,
    absences: Vec<Absence>,
    hours: Vec<WorkingHourCell>,
    tokens: Vec<AccessToken>,
    grants: Vec<OAuthGrant>,
    channels: Vec<NotificationChannel>,
    emails: Vec<OrganisationEmail>,
}

pub struct AccountView {
    session: Session,
    /// Administers some organisation: the weekly report goes only to them.
    is_admin: bool,
    me: Option<Me>,
    prefs: Prefs,
    load_error: Option<String>,
    busy: bool,
    display_name: Entity<InputState>,
    status: Entity<InputState>,
    current_password: Entity<InputState>,
    new_password: Entity<InputState>,
    mfa: Option<MfaStatus>,
    enrolling: Option<Enrolling>,
    totp_code: Entity<InputState>,
    fresh_codes: Option<Vec<String>>,
    absences: Vec<Absence>,
    away_from: Entity<InputState>,
    away_to: Entity<InputState>,
    away_note: Entity<InputState>,
    hours: Vec<WorkingHourCell>,
    /// Mid-drag across the grid: the value every square crossed is set to.
    painting: Option<bool>,
    tokens: Vec<AccessToken>,
    token_name: Entity<InputState>,
    token_scope: String,
    fresh_token: Option<String>,
    grants: Vec<OAuthGrant>,
    channels: Vec<NotificationChannel>,
    telegram_link: Option<String>,
    webhook_url: Entity<InputState>,
    webhook_label: Entity<InputState>,
    fresh_secret: Option<String>,
    emails: Vec<OrganisationEmail>,
    email_inputs: HashMap<String, Entity<InputState>>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl AccountView {
    pub fn new(
        session: Session,
        is_admin: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let secret = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(placeholder)
            })
        };
        let display_name = input("How your name appears to your team", window, cx);
        let status = input("Heads-down on the refit today", window, cx);
        let current_password = secret("Current password", window, cx);
        let new_password = secret("New password", window, cx);
        let totp_code = input("6-digit code", window, cx);
        let away_from = input("From: 2026-11-03, 3.11, +7", window, cx);
        let away_to = input("Until (included)", window, cx);
        let away_note = input("Sailing, annual leave…", window, cx);
        let token_name = input("What is it for — Claude on my laptop", window, cx);
        let webhook_url = input("https://…", window, cx);
        let webhook_label = input("Label", window, cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &display_name,
                window,
                |this, _, e: &InputEvent, window, cx| {
                    if entered(e) {
                        this.save_profile(window, cx)
                    }
                },
            ),
            cx.subscribe_in(&status, window, |this, _, e: &InputEvent, window, cx| {
                if entered(e) {
                    this.save_profile(window, cx)
                }
            }),
            cx.subscribe_in(
                &new_password,
                window,
                |this, _, e: &InputEvent, window, cx| {
                    if entered(e) {
                        this.change_password(window, cx)
                    }
                },
            ),
            cx.subscribe_in(&totp_code, window, |this, _, e: &InputEvent, window, cx| {
                if entered(e) {
                    this.verify_totp(window, cx)
                }
            }),
            cx.subscribe_in(&away_note, window, |this, _, e: &InputEvent, window, cx| {
                if entered(e) {
                    this.add_absence(window, cx)
                }
            }),
            cx.subscribe_in(
                &token_name,
                window,
                |this, _, e: &InputEvent, window, cx| {
                    if entered(e) {
                        this.create_token(window, cx)
                    }
                },
            ),
            cx.subscribe_in(
                &webhook_label,
                window,
                |this, _, e: &InputEvent, window, cx| {
                    if entered(e) {
                        this.add_webhook(window, cx)
                    }
                },
            ),
        ];
        let mut view = Self {
            session,
            is_admin,
            me: None,
            prefs: Prefs::default(),
            load_error: None,
            busy: false,
            display_name,
            status,
            current_password,
            new_password,
            mfa: None,
            enrolling: None,
            totp_code,
            fresh_codes: None,
            absences: Vec::new(),
            away_from,
            away_to,
            away_note,
            hours: Vec::new(),
            painting: None,
            tokens: Vec::new(),
            token_name,
            token_scope: "read".into(),
            fresh_token: None,
            grants: Vec::new(),
            channels: Vec::new(),
            telegram_link: None,
            webhook_url,
            webhook_label,
            fresh_secret: None,
            emails: Vec::new(),
            email_inputs: HashMap::new(),
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        view.reload(window, cx);
        view
    }

    pub fn set_is_admin(&mut self, is_admin: bool, cx: &mut Context<Self>) {
        if self.is_admin != is_admin {
            self.is_admin = is_admin;
            cx.notify();
        }
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.session.client.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let me = client.me()?;
                    Ok::<_, ApiError>(Loaded {
                        me,
                        mfa: client.mfa_status().ok(),
                        absences: client.my_absences().unwrap_or_default(),
                        hours: client
                            .my_working_hours()
                            .map(|h| h.cells)
                            .unwrap_or_default(),
                        tokens: client.access_tokens().unwrap_or_default(),
                        grants: client.oauth_grants().unwrap_or_default(),
                        channels: client.notification_channels().unwrap_or_default(),
                        emails: client.organisation_emails().unwrap_or_default(),
                    })
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(loaded) => {
                        this.load_error = None;
                        this.set_me(loaded.me, window, cx);
                        this.mfa = loaded.mfa;
                        this.absences = loaded.absences;
                        this.hours = loaded.hours;
                        this.tokens = loaded.tokens;
                        this.grants = loaded.grants;
                        this.channels = loaded.channels;
                        this.set_emails(loaded.emails, window, cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(AccountEvent::AuthFailed(e)),
                    Err(e) => this.load_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn set_me(&mut self, me: Me, window: &mut Window, cx: &mut Context<Self>) {
        let name = me.display_name.clone().unwrap_or_default();
        let status = me.status_message.clone().unwrap_or_default();
        self.display_name
            .update(cx, |i, cx| i.set_value(name, window, cx));
        self.status
            .update(cx, |i, cx| i.set_value(status, window, cx));
        self.prefs = Prefs::of(&me);
        self.me = Some(me);
    }

    /// One box per organisation, showing the confirmed address, saved on
    /// Return or on leaving it.
    fn set_emails(
        &mut self,
        emails: Vec<OrganisationEmail>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for row in &emails {
            let org_id = row.organisation_id.clone();
            if !self.email_inputs.contains_key(&org_id) {
                // The fallback, drawn: an empty box says where mail goes.
                let placeholder = row
                    .effective
                    .clone()
                    .unwrap_or_else(|| "your account address".into());
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                let org = org_id.clone();
                self._subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, e: &InputEvent, window, cx| {
                        if matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            this.save_email(org.clone(), window, cx)
                        }
                    },
                ));
                self.email_inputs.insert(org_id.clone(), input);
            }
            let value = row.email.clone().unwrap_or_default();
            if let Some(input) = self.email_inputs.get(&org_id) {
                input.update(cx, |i, cx| i.set_value(value, window, cx));
            }
        }
        self.emails = emails;
    }

    /// Runs `call` in the background, then hands its answer to `done`; a
    /// failure is said in a notification and changes nothing on screen.
    fn request<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        call: impl FnOnce(&ApiClient) -> ApiResult<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let client = self.session.client.clone();
        self.busy = true;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { call(&client) }).await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(e) if e.is_auth() => cx.emit(AccountEvent::AuthFailed(e)),
                    Err(e) => window.push_notification(format!("That didn't work: {e}"), cx),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn copy(text: &str, what: &str, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        window.push_notification(format!("{what} copied."), cx);
    }

    // ------------------------------------------------------------ profile

    fn save_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.prefs.clone();
        let fields = json!({
            "display_name": self.display_name.read(cx).value().trim(),
            "status_message": self.status.read(cx).value().trim(),
            "daily_summary_enabled": p.daily_summary,
            "daily_summary_hour": p.summary_hour,
            "weekly_report_enabled": p.weekly_report,
            "timer_check_ins": p.check_ins,
            "timer_check_ins_per_day": p.check_ins_per_day,
        });
        self.request(
            window,
            cx,
            move |c| c.update_me(fields),
            |this, me, window, cx| {
                cx.emit(AccountEvent::MeChanged(me.clone()));
                this.set_me(me, window, cx);
                window.push_notification("Saved.", cx);
            },
        );
    }

    fn change_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.current_password.read(cx).value().to_string();
        let new = self.new_password.read(cx).value().to_string();
        if current.is_empty() || new.is_empty() {
            return;
        }
        self.request(
            window,
            cx,
            move |c| c.change_password(&current, &new),
            |this, _, window, cx| {
                for input in [&this.current_password, &this.new_password] {
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                }
                window.push_notification("Password changed.", cx);
            },
        );
    }

    // ---------------------------------------------------------- two-factor

    fn start_totp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            |c| c.new_totp_device(),
            |this, device, window, cx| {
                let qr = device
                    .qr_data_uri
                    .split_once("base64,")
                    .and_then(|(_, b64)| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                    .map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                this.enrolling = Some(Enrolling { device, qr });
                this.totp_code
                    .update(cx, |i, cx| i.set_value("", window, cx));
                this.totp_code.update(cx, |i, cx| i.focus(window, cx));
            },
        );
    }

    /// The device counts once a code from it checks out; then ten backup
    /// codes, shown this once.
    fn verify_totp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(enrolling) = &self.enrolling else {
            return;
        };
        let secret = enrolling.device.secret.clone();
        let code = self.totp_code.read(cx).value().trim().to_owned();
        if code.is_empty() {
            return;
        }
        self.request(
            window,
            cx,
            move |c| {
                c.verify_totp(&secret, &code)?;
                let codes = c.new_backup_codes()?;
                Ok((codes, c.mfa_status()?))
            },
            |this, (codes, status), window, cx| {
                this.enrolling = None;
                this.fresh_codes = Some(codes.codes);
                this.mfa = Some(status);
                window.push_notification("Two-factor authentication is on.", cx);
            },
        );
    }

    fn regenerate_codes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            |c| Ok((c.new_backup_codes()?, c.mfa_status()?)),
            |this, (codes, status), _, _| {
                this.fresh_codes = Some(codes.codes);
                this.mfa = Some(status);
            },
        );
    }

    fn confirm_disable_totp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Turn off two-factor authentication?")
                .description("This removes your authenticator device and your backup codes.")
                .confirm()
                .ok_text("Turn off")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| {
                        this.request(
                            window,
                            cx,
                            |c| {
                                c.remove_totp()?;
                                c.mfa_status()
                            },
                            |this, status, window, cx| {
                                this.mfa = Some(status);
                                this.fresh_codes = None;
                                window.push_notification("Two-factor authentication is off.", cx);
                            },
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    // ------------------------------------------------------ out of office

    fn add_absence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let from = parse_day(&self.away_from.read(cx).value());
        let to = parse_day(&self.away_to.read(cx).value());
        let (Some(from), Some(to)) = (from, to) else {
            window.push_notification("Both dates, please: 2026-11-03, 3.11 or +7.", cx);
            return;
        };
        let note = self.away_note.read(cx).value().trim().to_owned();
        self.request(
            window,
            cx,
            move |c| {
                c.add_absence(&from, &to, (!note.is_empty()).then_some(note.as_str()))?;
                c.my_absences()
            },
            |this, absences, window, cx| {
                this.absences = absences;
                for input in [&this.away_from, &this.away_to, &this.away_note] {
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                }
            },
        );
    }

    fn delete_absence(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c| {
                c.delete_absence(&id)?;
                c.my_absences()
            },
            |this, absences, _, _| this.absences = absences,
        );
    }

    // ------------------------------------------------------ working hours

    /// Optimistic: a square changes colour at once, and a failed request
    /// puts the grid back as the server has it.
    fn set_hour(
        &mut self,
        cell: WorkingHourCell,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.hours.contains(&cell);
        if marked == on {
            return;
        }
        if on {
            self.hours.push(cell);
        } else {
            self.hours.retain(|c| *c != cell);
        }
        cx.notify();
        let client = self.session.client.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.set_working_hour(cell, on)?;
                    Ok::<_, ApiError>(())
                })
                .await;
            if let Err(e) = result {
                let client_err = e.to_string();
                this.update_in(cx, |this, window, cx| {
                    window.push_notification(format!("Couldn't save that hour: {client_err}"), cx);
                    this.reload(window, cx);
                })
                .ok();
            }
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    // ----------------------------------------------------- access tokens

    fn create_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.token_name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let scope = self.token_scope.clone();
        self.request(
            window,
            cx,
            move |c| {
                let made = c.create_token(&name, &scope)?;
                Ok((made.token, c.access_tokens()?))
            },
            |this, (token, tokens), window, cx| {
                this.fresh_token = Some(token);
                this.tokens = tokens;
                this.token_name
                    .update(cx, |i, cx| i.set_value("", window, cx));
            },
        );
    }

    fn revoke_token(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c| {
                c.revoke_token(&id)?;
                c.access_tokens()
            },
            |this, tokens, _, _| this.tokens = tokens,
        );
    }

    fn revoke_grant(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c| {
                c.revoke_oauth_grant(&id)?;
                c.oauth_grants()
            },
            |this, grants, _, _| this.grants = grants,
        );
    }

    // ------------------------------------------------------ notifications

    fn link_telegram(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            |c| c.telegram_link(),
            |this, link, _, cx| {
                cx.open_url(&link);
                this.telegram_link = Some(link);
            },
        );
    }

    fn add_webhook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.webhook_url.read(cx).value().trim().to_owned();
        if url.is_empty() {
            return;
        }
        let label = self.webhook_label.read(cx).value().trim().to_owned();
        self.request(
            window,
            cx,
            move |c| {
                let made = c.add_webhook(&url, &label)?;
                Ok((made.secret, c.notification_channels()?))
            },
            |this, (secret, channels), window, cx| {
                this.fresh_secret = Some(secret);
                this.channels = channels;
                for input in [&this.webhook_url, &this.webhook_label] {
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                }
            },
        );
    }

    fn remove_channel(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c| {
                c.delete_channel(&id)?;
                c.notification_channels()
            },
            |this, channels, _, _| this.channels = channels,
        );
    }

    fn route(
        &mut self,
        channel_id: String,
        kind: &'static str,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.channels.iter().find(|c| c.id == channel_id) else {
            return;
        };
        let mut kinds = channel.enabled_kinds.clone();
        kinds.retain(|k| k != kind);
        if on {
            kinds.push(kind.to_owned());
        }
        self.request(
            window,
            cx,
            move |c| c.set_channel_kinds(&channel_id, &kinds),
            |this, channel, _, _| {
                if let Some(row) = this.channels.iter_mut().find(|c| c.id == channel.id) {
                    *row = channel;
                }
            },
        );
    }

    /// Saved only when it moved — or while something is pending, so that
    /// clearing the box cancels a request and retyping resends the link.
    fn save_email(&mut self, org_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.emails.iter().find(|r| r.organisation_id == org_id) else {
            return;
        };
        let Some(input) = self.email_inputs.get(&org_id) else {
            return;
        };
        let value = input.read(cx).value().trim().to_owned();
        if value == row.email.clone().unwrap_or_default() && row.pending.is_none() {
            return;
        }
        let org_name = row.organisation_name.clone();
        self.request(
            window,
            cx,
            move |c| {
                c.set_organisation_email(&org_id, &value)?;
                Ok((value, c.organisation_emails()?))
            },
            move |this, (value, emails), window, cx| {
                this.set_emails(emails, window, cx);
                window.push_notification(
                    if value.is_empty() {
                        format!("{org_name} mail is back to your account address.")
                    } else {
                        format!("Confirm it first: a link went to {value}. {org_name} mail moves there once it's opened.")
                    },
                    cx,
                );
            },
        );
    }

    // ------------------------------------------------------------- render

    fn render_profile(&self, me: &Me, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let p = self.prefs.clone();
        let this = cx.entity().downgrade();
        let set = move |f: fn(&mut Prefs, String)| {
            let this = this.clone();
            move |value: String, _: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    f(&mut this.prefs, value);
                    cx.notify();
                })
                .ok();
            }
        };
        let hours: Vec<(String, String)> = (0..24)
            .map(|h| (h.to_string(), format!("{h:02}:00")))
            .collect();
        let modes: Vec<(String, String)> = [
            ("telegram", "Ask in Telegram and the apps"),
            ("apps", "Ask only in the apps"),
            ("off", "Don't ask"),
        ]
        .iter()
        .map(|(k, l)| (k.to_string(), l.to_string()))
        .collect();
        let mode_label = modes
            .iter()
            .find(|(k, _)| *k == p.check_ins)
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| p.check_ins.clone());
        let per_day: Vec<(String, String)> = (1..=24)
            .map(|n| {
                (
                    n.to_string(),
                    if n == 1 {
                        "At most once a day".to_owned()
                    } else {
                        format!("At most {n} times a day")
                    },
                )
            })
            .collect();
        let per_day_label = per_day
            .iter()
            .find(|(k, _)| *k == p.check_ins_per_day.to_string())
            .map(|(_, l)| l.clone())
            .unwrap_or_default();
        let zone = me.timezone.clone().unwrap_or_else(|| "UTC".into());
        let mono = theme.mono_font_family.clone();

        card(&theme)
            .child(section_title("Profile", Lucide::CircleUser, theme.blue, &theme))
            .child(field("Display name", Input::new(&self.display_name).small(), None, cx))
            .child(field(
                "Status",
                Input::new(&self.status).small(),
                Some("A line about what you're on with. Anyone you work with can see it."),
                cx,
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Checkbox::new("daily-summary")
                            .checked(p.daily_summary)
                            .label("Daily summary")
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.prefs.daily_summary = *on;
                                cx.notify();
                            })),
                    )
                    .child(hint(
                        "A morning nudge, per organisation, with what's planned for today and what closed yesterday.",
                        cx,
                    ))
                    .when(p.daily_summary, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child("Sent at")
                                .child(picker(
                                    "summary-hour",
                                    format!("{:02}:00", p.summary_hour),
                                    hours,
                                    Some(p.summary_hour.to_string()),
                                    set(|p, v| p.summary_hour = v.parse().unwrap_or(6)),
                                ))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("your own time, in {zone}")),
                                ),
                        )
                    }),
            )
            .when(self.is_admin, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            Checkbox::new("weekly-report")
                                .checked(p.weekly_report)
                                .label("Weekly report")
                                .on_click(cx.listener(|this, on: &bool, _, cx| {
                                    this.prefs.weekly_report = *on;
                                    cx.notify();
                                })),
                        )
                        .child(hint(
                            "Monday morning, per organisation you administer: hours by person, tasks opened and closed, articles and facts added.",
                            cx,
                        )),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child("Timer check-ins")
                            .child(picker(
                                "check-ins",
                                mode_label,
                                modes,
                                Some(p.check_ins.clone()),
                                set(|p, v| p.check_ins = v),
                            ))
                            .when(p.check_ins != "off", |this| {
                                this.child(picker(
                                    "check-ins-per-day",
                                    per_day_label,
                                    per_day,
                                    Some(p.check_ins_per_day.to_string()),
                                    set(|p, v| p.check_ins_per_day = v.parse().unwrap_or(6)),
                                ))
                            }),
                    )
                    .child(hint(
                        "While a timer runs you're asked now and then whether you're still on it. No answer within 15–30 minutes pauses it at the moment it asked.",
                        cx,
                    )),
            )
            .child(
                h_flex().child(
                    Button::new("save-profile")
                        .label("Save")
                        .primary()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.save_profile(window, cx))),
                ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_sm()
                    .children(
                        [
                            ("Email", me.email.clone().unwrap_or_else(|| "—".into())),
                            ("Timezone", zone.clone()),
                            ("User id", me.id.clone()),
                        ]
                        .into_iter()
                        .map(|(label, value)| {
                            h_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .w(px(80.))
                                        .text_color(theme.muted_foreground)
                                        .child(label),
                                )
                                .child(div().font_family(mono.clone()).text_xs().child(value))
                        }),
                    )
                    .child(hint(
                        "This app keeps your timezone in step with the machine's. Your email is how invitations find you, so it can't be changed here.",
                        cx,
                    )),
            )
    }

    fn render_password(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        card(&theme)
            .child(section_title("Password", Lucide::KeyRound, theme.blue, &theme))
            .child(Input::new(&self.current_password).small())
            .child(Input::new(&self.new_password).small())
            .child(
                h_flex().child(
                    Button::new("change-password")
                        .label("Change password")
                        .small()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.change_password(window, cx)),
                        ),
                ),
            )
            .child(hint(
                "The current one is asked for so that a session left open on a shared machine isn't enough to lock you out.",
                cx,
            ))
    }

    fn render_two_factor(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let mut body = v_flex().gap_3();
        match (&self.mfa, &self.enrolling) {
            (_, Some(enrolling)) => {
                let secret = enrolling.device.secret.clone();
                body = body
                    .child(hint(
                        "Scan the code with an authenticator app, then type the code it shows.",
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_4()
                            .items_start()
                            .when_some(enrolling.qr.clone(), |this, qr| {
                                this.child(
                                    div()
                                        .size(px(168.))
                                        .flex_none()
                                        .rounded(theme.radius)
                                        .bg(gpui_kit::white())
                                        .p_1()
                                        .child(img(qr).size_full()),
                                )
                            })
                            .child(
                                v_flex()
                                    .gap_2()
                                    .flex_1()
                                    .child(hint("Or enter this key by hand:", cx))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .font_family(mono)
                                                    .text_xs()
                                                    .child(secret.clone()),
                                            )
                                            .child(
                                                Button::new("copy-secret")
                                                    .icon(Lucide::Copy)
                                                    .ghost()
                                                    .xsmall()
                                                    .on_click(move |_, window, cx| {
                                                        Self::copy(&secret, "Key", window, cx)
                                                    }),
                                            ),
                                    )
                                    .child(Input::new(&self.totp_code).small())
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Button::new("verify-totp")
                                                    .label("Verify")
                                                    .primary()
                                                    .small()
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.verify_totp(window, cx)
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("cancel-totp")
                                                    .label("Cancel")
                                                    .ghost()
                                                    .small()
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.enrolling = None;
                                                        cx.notify();
                                                    })),
                                            ),
                                    ),
                            ),
                    );
            }
            (Some(status), None) if status.enrolled => {
                let left = status.codes_remaining;
                body = body
                    .child(div().text_sm().child(format!(
                        "Enabled. {left} backup code{} left.",
                        if left == 1 { "" } else { "s" }
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("regenerate-codes")
                                    .label("Regenerate backup codes")
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.regenerate_codes(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("disable-totp")
                                    .label("Turn off")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_disable_totp(window, cx)
                                    })),
                            ),
                    );
            }
            (Some(_), None) => {
                body = body
                    .child(hint(
                        "Not enabled. Turning it on asks for a code from your authenticator app at every sign-in — and an organisation that requires it will ask you to set it up anyway.",
                        cx,
                    ))
                    .child(
                        h_flex().child(
                            Button::new("enable-totp")
                                .label("Turn on")
                                .primary()
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start_totp(window, cx)),
                                ),
                        ),
                    );
            }
            (None, None) => {
                body = body.child(hint("Couldn't find out whether it's on.", cx));
            }
        }
        if let Some(codes) = self.fresh_codes.clone() {
            let all = codes.join("\n");
            body = body.child(shown_once(
                "New backup codes — your old ones stopped working. Keep these somewhere safe; they won't be shown again.",
                v_flex()
                    .gap_0p5()
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .children(codes.into_iter().map(|c| div().child(c))),
                ("copy-codes", "Copy all", all, "Backup codes"),
                cx.listener(|this, _, _, cx| {
                    this.fresh_codes = None;
                    cx.notify();
                }),
                cx,
            ));
        }
        card(&theme)
            .child(section_title(
                "Two-factor authentication",
                Lucide::ShieldCheck,
                theme.blue,
                &theme,
            ))
            .child(body)
    }

    fn render_away(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        card(&theme)
            .child(section_title("Out of office", Lucide::Plane, theme.blue, &theme))
            .children(self.absences.iter().map(|a| {
                let id = a.id.clone();
                row(&theme)
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} → {}", a.starts_on, a.ends_on)),
                    )
                    .when(a.away_now, |this| this.child(badge("Away now", &theme)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(a.note.clone().unwrap_or_default()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("away-del-{}", a.id)))
                            .icon(Lucide::Trash)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_absence(id.clone(), window, cx)
                            })),
                    )
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(170.)).child(Input::new(&self.away_from).small()))
                    .child(div().w(px(170.)).child(Input::new(&self.away_to).small()))
                    .child(div().flex_1().child(Input::new(&self.away_note).small()))
                    .child(
                        Button::new("add-away")
                            .label("Add")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add_absence(window, cx))),
                    ),
            )
            .child(hint(
                "Everyone in your organisations sees this — that's the point of recording it rather than remembering it.",
                cx,
            ))
    }

    fn render_hours(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let zone = self
            .me
            .as_ref()
            .and_then(|m| m.timezone.clone())
            .unwrap_or_else(|| "UTC".into());
        let view: &Context<Self> = cx;
        let grid = grid(&self.hours, view, |c, marked| {
            square(
                SharedString::from(format!("hour-{}-{}", c.weekday, c.hour)),
                marked,
                view,
            )
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                view.listener(move |this, _, window, cx| {
                    this.painting = Some(!marked);
                    this.set_hour(c, !marked, window, cx);
                }),
            )
            .on_mouse_move(view.listener(move |this, e: &MouseMoveEvent, window, cx| {
                match (this.painting, e.pressed_button) {
                    (Some(on), Some(MouseButton::Left)) => this.set_hour(c, on, window, cx),
                    (Some(_), _) => this.painting = None,
                    _ => {}
                }
            }))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!(
                    "{} {:02}:00",
                    WEEKDAYS[c.weekday as usize], c.hour
                ))
                .build(window, cx)
            })
        });
        card(&theme)
            .child(section_title("Working hours", Lucide::Clock, theme.blue, &theme))
            .child(
                div()
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.painting = None),
                    )
                    .child(grid),
            )
            .child(hint(
                &format!(
                    "Click a square, or drag across several, to mark when you're around — in {zone}. Anyone in your organisations sees it, converted to their own time."
                ),
                cx,
            ))
    }

    fn render_tokens(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let scopes = vec![
            ("read".to_owned(), "Read only".to_owned()),
            ("write".to_owned(), "Can change things".to_owned()),
        ];
        let current_scope = scope_label(&self.token_scope).to_owned();
        let this = cx.entity().downgrade();
        let base = self.session.base_url.clone();
        card(&theme)
            .child(section_title("Access tokens", Lucide::KeyRound, theme.blue, &theme))
            .child(hint(
                "Connect your own assistant over MCP. A token acts as you — it reaches exactly what you can reach.",
                cx,
            ))
            .when_some(self.fresh_token.clone(), |this, token| {
                let command = format!(
                    "claude mcp add --transport http ayeayecaptain {base}/mcp --header \"Authorization: Bearer {token}\""
                );
                this.child(
                    shown_once(
                        "Copy it now — it won't be shown again.",
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .child(token.clone()),
                        ("copy-token", "Copy token", token, "Token"),
                        cx.listener(|this, _, _, cx| {
                            this.fresh_token = None;
                            cx.notify();
                        }),
                        cx,
                    )
                    .child(
                        h_flex().child(
                            Button::new("copy-mcp")
                                .icon(Lucide::Copy)
                                .label("Copy the `claude mcp add` command")
                                .ghost()
                                .small()
                                .on_click(move |_, window, cx| {
                                    Self::copy(&command, "Command", window, cx)
                                }),
                        ),
                    ),
                )
            })
            .children(self.tokens.iter().map(|t| {
                let id = t.id.clone();
                row(&theme)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(t.name.clone()),
                    )
                    .child(badge(scope_label(&t.scope), &theme))
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{}…", t.prefix)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match t.last_used_at {
                                Some(at) => format!("used {}", ago_long(at)),
                                None => "never used".into(),
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("token-del-{}", t.id)))
                            .icon(Lucide::Trash)
                            .ghost()
                            .xsmall()
                            .tooltip("Revoke")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.revoke_token(id.clone(), window, cx)
                            })),
                    )
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.token_name).small()))
                    .child(picker(
                        "token-scope",
                        current_scope,
                        scopes,
                        Some(self.token_scope.clone()),
                        move |scope, _, cx| {
                            this.update(cx, |this, cx| {
                                this.token_scope = scope;
                                cx.notify();
                            })
                            .ok();
                        },
                    ))
                    .child(
                        Button::new("create-token")
                            .label("Create token")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.create_token(window, cx))),
                    ),
            )
            .child(hint(
                "Start with read-only. A write token lets an assistant create tasks and comment as you.",
                cx,
            ))
    }

    fn render_grants(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        card(&theme)
            .child(section_title("Connected apps", Lucide::Plug, theme.blue, &theme))
            .child(hint(
                "Apps you've authorised over OAuth — Claude.ai, ChatGPT, anything that connected itself rather than using a token.",
                cx,
            ))
            .children(self.grants.iter().map(|g| {
                let id = g.id.clone();
                row(&theme)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(g.client_name.clone()),
                    )
                    .child(badge(scope_label(&g.scope), &theme))
                    .when_some(g.created_at, |this, at| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("connected {}", ago_long(at))),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("grant-del-{}", g.id)))
                            .icon(Lucide::Trash)
                            .ghost()
                            .xsmall()
                            .tooltip("Revoke")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.revoke_grant(id.clone(), window, cx)
                            })),
                    )
            }))
    }

    fn render_channels(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let has_telegram = self.channels.iter().any(|c| c.kind == "telegram");
        card(&theme)
            .child(section_title(
                "Notification channels",
                Lucide::Bell,
                theme.blue,
                &theme,
            ))
            .child(hint(
                "Where a notification can reach you beyond the inbox. Pick which kinds go where below.",
                cx,
            ))
            .children(self.channels.iter().map(|c| {
                let id = c.id.clone();
                let icon = match c.kind.as_str() {
                    "telegram" => Lucide::Send,
                    "webhook" => Lucide::Webhook,
                    _ => Lucide::Mail,
                };
                row(&theme)
                    .child(Icon::new(icon).small().text_color(theme.muted_foreground))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(c.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(c.url.clone().unwrap_or_default()),
                    )
                    .when(c.kind == "telegram" && c.verified_at.is_none(), |this| {
                        this.child(badge("Waiting for /start", &theme))
                    })
                    .when(c.kind != "email", |this| {
                        this.child(
                            Button::new(SharedString::from(format!("channel-del-{}", c.id)))
                                .icon(Lucide::Trash)
                                .ghost()
                                .xsmall()
                                .tooltip("Remove")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_channel(id.clone(), window, cx)
                                })),
                        )
                    })
            }))
            .child(
                h_flex().child(
                    Button::new("link-telegram")
                        .icon(Lucide::Send)
                        .label(if has_telegram {
                            "Re-link Telegram"
                        } else {
                            "Link Telegram"
                        })
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.link_telegram(window, cx))),
                ),
            )
            .when_some(self.telegram_link.clone(), |this, link| {
                this.child(shown_once(
                    "Opened in your browser — tap Start in Telegram to finish. If it didn't open:",
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .child(link.clone()),
                    ("copy-telegram", "Copy link", link, "Link"),
                    cx.listener(|this, _, window, cx| {
                        this.telegram_link = None;
                        this.reload(window, cx);
                    }),
                    cx,
                ))
            })
            .when_some(self.fresh_secret.clone(), |this, secret| {
                this.child(shown_once(
                    "Copy the signing secret now — it won't be shown again. Every delivery carries X-Ayeaye-Signature: sha256=…, signed over the raw body with it.",
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .child(secret.clone()),
                    ("copy-webhook-secret", "Copy secret", secret, "Secret"),
                    cx.listener(|this, _, _, cx| {
                        this.fresh_secret = None;
                        cx.notify();
                    }),
                    cx,
                ))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.webhook_url).small()))
                    .child(div().w(px(140.)).child(Input::new(&self.webhook_label).small()))
                    .child(
                        Button::new("add-webhook")
                            .icon(Lucide::Webhook)
                            .label("Add webhook")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add_webhook(window, cx))),
                    ),
            )
    }

    /// Kinds down, channels across, a box where they meet.
    fn render_routing(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let col = px(96.);
        card(&theme)
            .child(section_title(
                "Which notification goes where",
                Lucide::Bell,
                theme.blue,
                &theme,
            ))
            .child(
                h_flex()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child("Notification"))
                    .children(
                        self.channels
                            .iter()
                            .map(|c| div().w(col).truncate().text_center().child(c.label.clone())),
                    ),
            )
            .child(
                v_flex().children(NOTIFICATION_KINDS.iter().map(|(kind, label)| {
                    h_flex()
                        .h(px(28.))
                        .border_t_1()
                        .border_color(theme.border)
                        .text_sm()
                        .child(div().flex_1().min_w_0().truncate().child(*label))
                        .children(self.channels.iter().map(|c| {
                            let id = c.id.clone();
                            let on = c.enabled_kinds.iter().any(|k| k == kind);
                            h_flex().w(col).justify_center().child(
                                Checkbox::new(SharedString::from(format!("route-{}-{kind}", c.id)))
                                    .checked(on)
                                    .on_click(cx.listener(move |this, on: &bool, window, cx| {
                                        this.route(id.clone(), kind, *on, window, cx)
                                    })),
                            )
                        }))
                })),
            )
    }

    fn render_emails(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        card(&theme)
            .child(section_title("Email per organisation", Lucide::Mail, theme.blue, &theme))
            .child(hint(
                "Notifications go to your account address unless you name another one here. A new address works once you've opened the link sent to it. Saved on Return.",
                cx,
            ))
            .children(self.emails.iter().filter_map(|row| {
                let input = self.email_inputs.get(&row.organisation_id)?;
                Some(
                    h_flex()
                        .gap_3()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .child(row.organisation_name.clone()),
                                )
                                .when_some(row.pending.clone(), |this, pending| {
                                    this.child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(format!(
                                                "Waiting on {pending} — check that inbox for the link"
                                            )),
                                    )
                                }),
                        )
                        .child(div().w(px(300.)).child(Input::new(input).small())),
                )
            }))
    }
}

fn entered(e: &InputEvent) -> bool {
    matches!(e, InputEvent::PressEnter { .. })
}

/// A label over its control, with an optional line of explanation.
fn field(
    label: &'static str,
    control: impl IntoElement,
    help: Option<&'static str>,
    cx: &App,
) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(control)
        .when_some(help, |this, help| this.child(hint(help, cx)))
}

fn hint(text: &str, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_owned())
}

/// A bordered line in a list: a token, an absence, a channel.
fn row(theme: &gpui_kit::component::Theme) -> Div {
    h_flex()
        .gap_3()
        .px_3()
        .py_1p5()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
}

fn badge(label: &str, theme: &gpui_kit::component::Theme) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(label.to_owned())
}

fn scope_label(scope: &str) -> &'static str {
    if scope == "write" {
        "Can change things"
    } else {
        "Read only"
    }
}

/// Something the server shows once: what it is, the value, a copy button
/// and a Done that forgets it.
fn shown_once(
    title: &'static str,
    value: impl IntoElement,
    (id, copy_label, text, what): (&'static str, &'static str, String, &'static str),
    done: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_2()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.primary.opacity(0.4))
        .bg(theme.primary.opacity(0.06))
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(value)
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(id)
                        .icon(Lucide::Copy)
                        .label(copy_label)
                        .small()
                        .on_click(move |_, window, cx| AccountView::copy(&text, what, window, cx)),
                )
                .child(
                    Button::new(SharedString::from(format!("{id}-done")))
                        .label("Done")
                        .ghost()
                        .small()
                        .on_click(done),
                ),
        )
}

impl Focusable for AccountView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for AccountView {
    fn item_key(&self, _: &App) -> SharedString {
        "account:me".into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        "Your account".into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::UserCog)
    }

    fn is_dirty(&self, _: &App) -> bool {
        false
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let me = self.me.as_ref()?;
        Some(me.email.clone().unwrap_or_default().into())
    }
}

impl Render for AccountView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match (&self.me, &self.load_error) {
            (Some(me), _) => {
                let me = me.clone();
                v_flex()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Your account"),
                            )
                            .when(self.busy, |this| this.child(Spinner::new().small())),
                    )
                    .child(hint(
                        "How you appear to everyone you work with, and how you're reached.",
                        cx,
                    ))
                    .child(self.render_profile(&me, cx))
                    .child(self.render_away(cx))
                    .child(self.render_hours(cx))
                    .child(self.render_password(cx))
                    .child(self.render_two_factor(cx))
                    .child(self.render_tokens(cx))
                    .when(!self.grants.is_empty(), |this| {
                        this.child(self.render_grants(cx))
                    })
                    .child(self.render_channels(cx))
                    .when(!self.channels.is_empty(), |this| {
                        this.child(self.render_routing(cx))
                    })
                    .when(!self.emails.is_empty(), |this| {
                        this.child(self.render_emails(cx))
                    })
                    .into_any_element()
            }
            (None, Some(e)) => v_flex()
                .gap_2()
                .child(div().text_lg().child("Couldn't load your account"))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(e.clone()),
                )
                .into_any_element(),
            (None, None) => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child("Loading…")
                .into_any_element(),
        };
        div()
            .id("account-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(div().max_w(px(860.)).px_6().py_5().pb_12().child(content))
    }
}
