//! The people in the sidebar: everyone in the organisation, with their own
//! time of day and whether it's inside their working hours — the question
//! you ask before getting in touch. Click someone for the rest.
//!
//! Working hours aren't on the member list, so each person's are fetched
//! once on their own; they're visible to anyone sharing the organisation.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jiff::Timestamp;

use crate::api::ApiError;
use crate::api::models::{Absence, Member, WorkingHours, role_label};
use crate::assets::Lucide;
use crate::people::hours::{Availability, availability, back_in, wall_clock_label};
use crate::session::OrgCtx;
use crate::ui::open::Open;
use crate::ui::widgets::avatar;

pub enum PeoplePanelEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for PeoplePanel {}
impl EventEmitter<PeoplePanelEvent> for PeoplePanel {}

pub struct PeoplePanel {
    ctx: OrgCtx,
    hours: HashMap<String, WorkingHours>,
    away: HashMap<String, Vec<Absence>>,
    /// Whose hours were asked for, so a directory refresh asks only for
    /// newcomers.
    asked: Vec<String>,
    loading: bool,
    error: Option<String>,
    active_person: Option<String>,
    filter: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl PeoplePanel {
    pub fn new(ctx: OrgCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter people…")
                .clean_on_escape()
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &filter,
                window,
                |this: &mut Self, _, e: &InputEvent, _, cx| match e {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => {
                        if let Some(id) = this.matches(cx).first().and_then(|m| m.user_id.clone()) {
                            cx.emit(Open::Person {
                                id,
                                org_id: None,
                                preview: false,
                            });
                        }
                    }
                    _ => {}
                },
            ),
            cx.observe(&ctx.directory, |this, _, cx| {
                this.load_hours(false, cx);
                cx.notify();
            }),
        ];
        ctx.directory.update(cx, |d, cx| d.ensure_loaded(cx));
        // Clocks move; a list left open shouldn't keep saying 9:00.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let mut panel = Self {
            ctx,
            hours: HashMap::new(),
            away: HashMap::new(),
            asked: Vec::new(),
            loading: false,
            error: None,
            active_person: None,
            filter,
            _subscriptions: subscriptions,
            _tasks: vec![tick],
        };
        panel.load_hours(true, cx);
        panel
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter.update(cx, |f, cx| {
            f.focus(window, cx);
            f.select_all(window, cx);
        });
    }

    pub fn set_active_person(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_person != id {
            self.active_person = id;
            cx.notify();
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.ctx.directory.update(cx, |d, cx| d.refresh(cx));
        self.load_hours(true, cx);
    }

    /// Everyone's hours (or, with `all` off, only those not asked for yet)
    /// and the organisation's out-of-office list.
    fn load_hours(&mut self, all: bool, cx: &mut Context<Self>) {
        let people: Vec<String> = self
            .ctx
            .directory
            .read(cx)
            .members
            .iter()
            .filter_map(|m| m.user_id.clone())
            .filter(|id| all || !self.asked.contains(id))
            .collect();
        if people.is_empty() && !all {
            return;
        }
        self.asked.extend(people.iter().cloned());
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self.loading = true;
        cx.notify();
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let away = client.organisation_absences(&org_id);
                    let hours: Vec<(String, WorkingHours)> = people
                        .into_iter()
                        .filter_map(|id| {
                            let hours = client.member_working_hours(&org_id, &id).ok()?;
                            Some((id, hours))
                        })
                        .collect();
                    (away, hours)
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                let (away, hours) = result;
                match away {
                    Ok(list) => {
                        this.away.clear();
                        for absence in list {
                            if let Some(person) = &absence.person {
                                this.away
                                    .entry(person.id.clone())
                                    .or_default()
                                    .push(absence.clone());
                            }
                        }
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(PeoplePanelEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.hours.extend(hours);
                cx.notify();
            })
            .ok();
        }));
    }

    fn matches(&self, cx: &App) -> Vec<Member> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        self.ctx
            .directory
            .read(cx)
            .members
            .iter()
            .filter(|m| {
                let hay = format!(
                    "{} {}",
                    m.name().to_lowercase(),
                    m.email.as_deref().unwrap_or("").to_lowercase()
                );
                terms.iter().all(|term| hay.contains(term))
            })
            .cloned()
            .collect()
    }

    /// "Wed 14:05 · working", "Tue 23:10 · back in 9h", "away until 3 Nov".
    fn presence_line(&self, user_id: &str, now: Timestamp) -> (String, bool) {
        if let Some(away) = self
            .away
            .get(user_id)
            .and_then(|a| a.iter().find(|a| a.away_now))
        {
            return (format!("away until {}", away.ends_on), false);
        }
        let Some(hours) = self.hours.get(user_id) else {
            return (String::new(), false);
        };
        let (clock, state) = availability(hours.timezone.as_deref(), &hours.cells, now);
        let clock = clock.map(wall_clock_label);
        let (state, working) = match state {
            Availability::Working => ("working".to_owned(), true),
            Availability::BackIn(h) => (back_in(h), false),
            Availability::NoHours => ("no hours set".to_owned(), false),
            Availability::NoTimezone => ("no timezone yet".to_owned(), false),
        };
        (
            match clock {
                Some(clock) => format!("{clock} · {state}"),
                None => state,
            },
            working,
        )
    }
}

impl Render for PeoplePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let me = self.ctx.me().map(str::to_owned);
        let now = Timestamp::now();
        let matches = self.matches(cx);
        let count = matches.len();
        let rows: Vec<AnyElement> = matches
            .iter()
            .filter_map(|member| {
                let id = member.user_id.clone()?;
                let name = member.name();
                let active = self.active_person.as_deref() == Some(id.as_str());
                let (line, working) = self.presence_line(&id, now);
                let is_me = me.as_deref() == Some(id.as_str());
                Some(
                    h_flex()
                        .id(SharedString::from(format!("person-row-{id}")))
                        .px_3()
                        .py_1()
                        .gap_2()
                        .cursor_pointer()
                        .when(active, |this| this.bg(theme.list_active))
                        .hover(|this| this.bg(theme.list_hover))
                        .child(div().relative().child(avatar(&name, px(26.), &theme)).when(
                            working,
                            |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .bottom(px(-1.))
                                        .right(px(-1.))
                                        .size(px(9.))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(theme.sidebar)
                                        .bg(theme.green),
                                )
                            },
                        ))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .text_sm()
                                        .child(div().min_w_0().truncate().child(name))
                                        .when(is_me, |this| {
                                            this.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child("you"),
                                            )
                                        }),
                                )
                                .when(!line.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(line),
                                    )
                                }),
                        )
                        .when(member.role == "owner" || member.role == "admin", |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(role_label(&member.role).to_owned()),
                            )
                        })
                        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                            cx.emit(Open::Person {
                                id: id.clone(),
                                org_id: None,
                                preview: event.click_count() < 2,
                            });
                        }))
                        .into_any_element(),
                )
            })
            .collect();

        v_flex()
            .key_context("PeoplePanel")
            .size_full()
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_none()
                    .pl_3()
                    .pr_1()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("PEOPLE"))
                    .child(div().child(format!("{count}")))
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("people-account")
                            .icon(Lucide::UserCog)
                            .ghost()
                            .xsmall()
                            .tooltip("Your account")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Open::Account))),
                    )
                    .child(
                        Button::new("people-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                div().px_2().pb_2().flex_none().child(
                    Input::new(&self.filter)
                        .small()
                        .prefix(
                            Icon::new(IconName::Search)
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        )
                        .cleanable(true),
                ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1p5()
                        .rounded(theme.radius)
                        .bg(theme.danger.opacity(0.12))
                        .text_xs()
                        .child(error),
                )
            })
            .child(
                v_flex()
                    .id("people-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(count == 0, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Nobody here."),
                        )
                    }),
            )
    }
}
