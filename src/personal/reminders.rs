//! Reminders, across every organisation: overdue first, then by day. Ticking
//! one marks it done. New ones are standalone — a title and a day — filed in
//! the current organisation; a task's reminders are set from the task.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use crate::api::models::Reminder;
use crate::api::{ApiClient, ApiError};
use crate::ui::open::Open;
use crate::ui::widgets::{human_day, parse_day, picker, quick_dates, section_label};

pub enum RemindersEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for RemindersPanel {}
impl EventEmitter<RemindersEvent> for RemindersPanel {}

pub struct RemindersPanel {
    client: Arc<ApiClient>,
    org_id: Option<String>,
    reminders: Vec<Reminder>,
    loading: bool,
    error: Option<String>,
    title: Entity<InputState>,
    date: Entity<InputState>,
    day: String,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl RemindersPanel {
    pub fn new(client: Arc<ApiClient>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Remind me to… ⏎"));
        let date = cx.new(|cx| InputState::new(window, cx).placeholder("or a date: 3.11, +7"));
        let submit = |this: &mut Self,
                      _: &Entity<InputState>,
                      e: &InputEvent,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = e {
                this.create(window, cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&title, window, submit),
            cx.subscribe_in(&date, window, submit),
        ];
        let tomorrow = quick_dates()[1].1.clone();
        let mut panel = Self {
            client,
            org_id: None,
            reminders: Vec::new(),
            loading: false,
            error: None,
            title,
            date,
            day: tomorrow,
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        panel.refresh(cx);
        panel
    }

    /// Where a new standalone reminder is filed.
    pub fn set_org(&mut self, org_id: Option<String>) {
        self.org_id = org_id;
    }

    pub fn due_count(&self) -> usize {
        let today = quick_dates()[0].1.clone();
        self.reminders
            .iter()
            .filter(|r| r.overdue || r.remind_on <= today)
            .count()
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.loading = true;
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { client.reminders() }).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(mut reminders) => {
                        reminders.sort_by(|a, b| {
                            b.overdue
                                .cmp(&a.overdue)
                                .then(a.remind_on.cmp(&b.remind_on))
                        });
                        this.reminders = reminders;
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(RemindersEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let typed = self.date.read(cx).value().trim().to_owned();
        let day = if typed.is_empty() {
            self.day.clone()
        } else {
            match parse_day(&typed) {
                Some(d) => d,
                None => {
                    self.error = Some(format!(
                        "“{typed}” isn't a date I can read — try 2026-11-03 or +7"
                    ));
                    cx.notify();
                    return;
                }
            }
        };
        let Some(org_id) = self.org_id.clone() else {
            self.error = Some("Pick an organisation first — a reminder is filed in one.".into());
            cx.notify();
            return;
        };
        self.title.update(cx, |i, cx| i.set_value("", window, cx));
        self.date.update(cx, |i, cx| i.set_value("", window, cx));
        let client = self.client.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.create_reminder(&org_id, &title, &day, None)?;
                    client.reminders()
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(reminders) => {
                        this.reminders = reminders;
                        this.error = None;
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn mark_done(&mut self, id: String, cx: &mut Context<Self>) {
        self.reminders.retain(|r| r.id != id);
        cx.notify();
        let client = self.client.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { client.update_reminder(&id, json!({ "done": true })) },
                )
                .await;
            if let Err(e) = result {
                this.update(cx, |this, cx| {
                    this.error = Some(format!("Couldn't mark it done: {e}"));
                    this.refresh(cx);
                })
                .ok();
            }
        }));
    }

    fn snooze(&mut self, id: String, day: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.update_reminder(&id, json!({ "remind_on": day }))?;
                    client.reminders()
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(mut reminders) => {
                        reminders.sort_by(|a, b| {
                            b.overdue
                                .cmp(&a.overdue)
                                .then(a.remind_on.cmp(&b.remind_on))
                        });
                        this.reminders = reminders;
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for RemindersPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let today = quick_dates()[0].1.clone();
        let (due, later): (Vec<&Reminder>, Vec<&Reminder>) = self
            .reminders
            .iter()
            .partition(|r| r.overdue || r.remind_on <= today);
        let orgs: std::collections::HashSet<_> = self
            .reminders
            .iter()
            .filter_map(|r| r.organisation_id.clone())
            .collect();
        let show_org = orgs.len() > 1;
        let day_label = quick_dates()
            .into_iter()
            .find(|(_, d)| *d == self.day)
            .map(|(l, _)| l.to_owned())
            .unwrap_or_else(|| human_day(&self.day));
        let this = cx.entity().downgrade();

        let row = |r: &Reminder, cx: &mut Context<Self>| {
            let id = r.id.clone();
            let snooze_id = r.id.clone();
            let open = r.task_id.clone().map(|task_id| Open::Task {
                id: task_id,
                org_id: r.organisation_id.clone(),
                preview: false,
            });
            let this = cx.entity().downgrade();
            h_flex()
                .id(SharedString::from(format!("reminder-{}", r.id)))
                .min_h(px(28.))
                .px_3()
                .gap_2()
                .text_sm()
                .hover(|s| s.bg(theme.list_hover))
                .child(
                    Checkbox::new(SharedString::from(format!("done-{}", r.id)))
                        .tooltip("Done")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.mark_done(id.clone(), cx)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(r.label()))
                        .when_some(r.note.clone().filter(|n| !n.is_empty()), |this, note| {
                            this.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(note),
                            )
                        })
                        .when(show_org, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(r.organisation_name.clone().unwrap_or_default()),
                            )
                        }),
                )
                .child(picker(
                    SharedString::from(format!("snooze-{}", r.id)),
                    human_day(&r.remind_on),
                    quick_dates()
                        .into_iter()
                        .skip(1)
                        .map(|(l, d)| (d, format!("Snooze to {}", l.to_lowercase())))
                        .collect(),
                    None,
                    move |day, _, cx| {
                        this.update(cx, |this, cx| this.snooze(snooze_id.clone(), day, cx))
                            .ok();
                    },
                ))
                .when_some(open, |this, open| {
                    this.cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(open.clone())))
                })
                .when(r.overdue, |this| this.border_l_2().border_color(theme.red))
        };
        let due_rows: Vec<_> = due.iter().map(|r| row(r, cx)).collect();
        let later_rows: Vec<_> = later.iter().map(|r| row(r, cx)).collect();

        v_flex()
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("REMINDERS"))
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("reminders-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                v_flex()
                    .px_2()
                    .pb_2()
                    .gap_1()
                    .child(Input::new(&self.title).small())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(picker(
                                "reminder-day",
                                day_label,
                                quick_dates()
                                    .into_iter()
                                    .map(|(l, d)| (d, l.to_owned()))
                                    .collect(),
                                Some(self.day.clone()),
                                move |day, _, cx| {
                                    this.update(cx, |this, cx| {
                                        this.day = day;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                            ))
                            .child(div().flex_1().child(Input::new(&self.date).small())),
                    ),
            )
            .when_some(self.error.clone(), |this, e| {
                this.child(
                    div()
                        .mx_2()
                        .mb_2()
                        .px_2()
                        .py_1p5()
                        .rounded(theme.radius)
                        .bg(theme.danger.opacity(0.12))
                        .text_xs()
                        .child(e),
                )
            })
            .child(
                v_flex()
                    .id("reminders-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(!due_rows.is_empty(), |this| {
                        this.child(section_label("DUE", cx)).children(due_rows)
                    })
                    .when(!later_rows.is_empty(), |this| {
                        this.child(section_label("UPCOMING", cx))
                            .children(later_rows)
                    })
                    .when(self.reminders.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No reminders."),
                        )
                    }),
            )
    }
}
