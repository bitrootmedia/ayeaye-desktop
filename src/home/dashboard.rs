//! The organisation's front page in a tab, the web app's dashboard: what
//! everyone has been told and who's away first — the organisation's page,
//! not yours — then your own critical, urgent, high-priority, due-soon and
//! pinned work, and what everybody touched last.
//!
//! One request for the lot (`GET …/dashboard`), so it never draws in
//! stages. The task cards are the viewer's own (owned, or asked to act); a
//! card with nothing on it isn't drawn.

use chrono::{DateTime, NaiveDate, Utc};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, Theme, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::models::{
    Absence, Announcement, Dashboard, DashboardTask, RecentTask, status_label,
};
use crate::api::{ApiClient, ApiResult};
use crate::assets::Lucide;
use crate::home::HomeEvent;
use crate::session::OrgCtx;
use crate::tasks::view::{card, section_title};
use crate::ui::format::{ago_long, ago_short};
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;
use crate::ui::widgets::{dot, human_day, parse_day, status_color};
use gpui_kit::Task;

impl EventEmitter<ItemEvent> for DashboardView {}
impl EventEmitter<HomeEvent> for DashboardView {}
impl EventEmitter<Open> for DashboardView {}

pub struct DashboardView {
    ctx: OrgCtx,
    data: Option<Dashboard>,
    loaded_at: Option<DateTime<Utc>>,
    load_error: Option<String>,
    loading: bool,
    /// The new-announcement form is open.
    composing: bool,
    body: Entity<TextareaState>,
    until: Entity<InputState>,
    sticky: bool,
    busy: bool,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _tasks: Vec<Task<()>>,
}

impl DashboardView {
    pub fn new(ctx: OrgCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Yard closed Friday…")
                .auto_grow(3, 12)
        });
        // An end date is what stops the page turning into a wall of last
        // year's paper: optional, offered every time.
        let until = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Until (optional): 2026-11-03, 3.11 or +7")
        });
        let mut view = Self {
            ctx,
            data: None,
            loaded_at: None,
            load_error: None,
            loading: false,
            composing: false,
            body,
            until,
            sticky: false,
            busy: false,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _tasks: Vec::new(),
        };
        view.reload(cx);
        view
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self.loading = true;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.dashboard(&org_id) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(data) => {
                        this.data = Some(data);
                        this.loaded_at = Some(Utc::now());
                        this.load_error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(HomeEvent::AuthFailed(e)),
                    Err(e) => this.load_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    /// Runs `call` in the background and reloads the page after; a failure
    /// is said in a notification.
    fn request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        call: impl FnOnce(&ApiClient) -> ApiResult<()> + Send + 'static,
        done: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let client = self.ctx.client();
        self.busy = true;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { call(&client) }).await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        done(this, window, cx);
                        this.reload(cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(HomeEvent::AuthFailed(e)),
                    Err(e) => window.push_notification(format!("That didn't work: {e}"), cx),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composing = true;
        self.body.update(cx, |b, cx| b.focus(window, cx));
        cx.notify();
    }

    fn post(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.body.read(cx).value().trim().to_owned();
        if body.is_empty() || self.busy {
            return;
        }
        let until = self.until.read(cx).value().trim().to_owned();
        let until = if until.is_empty() {
            None
        } else {
            match parse_day(&until) {
                Some(day) => Some(day),
                None => {
                    window.push_notification("That end date: 2026-11-03, 3.11 or +7.", cx);
                    return;
                }
            }
        };
        let org_id = self.ctx.org_id().to_owned();
        let sticky = self.sticky;
        self.request(
            window,
            cx,
            move |c| {
                c.post_announcement(&org_id, &body, sticky, until.as_deref())
                    .map(|_| ())
            },
            |this, window, cx| {
                this.composing = false;
                this.sticky = false;
                this.body.update(cx, |b, cx| b.set_value("", window, cx));
                this.until.update(cx, |i, cx| i.set_value("", window, cx));
            },
        );
    }

    /// Asked first: it comes down for everyone, and there's no undo.
    fn confirm_take_down(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let id = id.clone();
            alert
                .title("Take this announcement down?")
                .description("It disappears from everyone's front page.")
                .confirm()
                .ok_text("Take down")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let id = id.clone();
                    this.update(cx, |this, cx| {
                        let org_id = this.ctx.org_id().to_owned();
                        this.request(
                            window,
                            cx,
                            move |c| c.delete_announcement(&org_id, &id),
                            |_, _, _| {},
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    // ------------------------------------------------------------- render

    fn render_header(&self, d: &Dashboard, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let web = self
            .ctx
            .session
            .web_url(&format!("/orgs/{}", self.ctx.org_id()));
        h_flex()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(22.))
                            .line_height(px(30.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.ctx.org.name.clone()),
                    )
                    .child(
                        div().text_sm().text_color(theme.muted_foreground).child(
                            "What everyone needs to know, who isn't here, and what needs you.",
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_none()
                    .when(self.loading || self.busy, |this| {
                        this.child(Spinner::new().small())
                    })
                    .when(d.can_announce && !self.composing, |this| {
                        this.child(
                            Button::new("home-announce")
                                .icon(IconName::Plus)
                                .label("Announcement")
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.compose(window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("home-calendar")
                            .icon(IconName::Calendar)
                            .label("Calendar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Open::Calendar))),
                    )
                    .child(
                        Button::new("home-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .small()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        Button::new("home-web")
                            .icon(IconName::ExternalLink)
                            .ghost()
                            .small()
                            .tooltip("Open in browser")
                            .on_click(move |_, _, cx| cx.open_url(&web)),
                    ),
            )
    }

    fn render_compose(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let empty = self.body.read(cx).value().trim().is_empty();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.3))
            .child(Textarea::new(&self.body))
            .child(
                h_flex()
                    .gap_3()
                    .child(div().w(px(300.)).child(Input::new(&self.until).small()))
                    .child(
                        Checkbox::new("home-sticky")
                            .checked(self.sticky)
                            .label("Pin to the top")
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.sticky = *on;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("home-compose-cancel")
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.composing = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("home-compose-post")
                            .label("Post")
                            .primary()
                            .small()
                            .disabled(empty || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.post(window, cx))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Everyone in the organisation sees this on their front page."),
            )
    }

    fn render_announcements(
        &self,
        d: &Dashboard,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let can_announce = d.can_announce;
        card(&theme)
            .flex_1()
            .min_w(px(360.))
            .child(section_title(
                "Announcements",
                Lucide::Megaphone,
                theme.blue,
                &theme,
            ))
            .when(self.composing, |this| this.child(self.render_compose(cx)))
            .when(d.announcements.is_empty() && !self.composing, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(if can_announce {
                            "Nothing posted. Anything you put here is seen by everyone in the organisation."
                        } else {
                            "Nothing posted."
                        }),
                )
            })
            .children(
                d.announcements
                    .iter()
                    .map(|a| self.render_announcement(a, can_announce, cx).into_any_element())
                    .collect::<Vec<_>>(),
            )
    }

    fn render_announcement(
        &self,
        a: &Announcement,
        can_announce: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = a.id.clone();
        let author = a
            .author
            .as_ref()
            .map(|p| p.name())
            .unwrap_or_else(|| "Someone".into());
        v_flex()
            .gap_1()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    // Pinned is shape, not colour: red already means "this
                    // needs you".
                    .when(a.sticky, |this| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .px_1p5()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(theme.border)
                                .child(Icon::new(Lucide::Pin).xsmall())
                                .child("Pinned"),
                        )
                    })
                    .child(match a.created_at {
                        Some(at) => format!("{author} · {}", ago_long(at)),
                        None => author,
                    })
                    .when_some(a.expires_on.clone(), |this, until| {
                        this.child(format!("until {}", short_day(&until)))
                    })
                    .child(div().flex_1())
                    .when(can_announce, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("announcement-del-{}", a.id)))
                                .icon(Lucide::Trash)
                                .ghost()
                                .xsmall()
                                .tooltip("Take this announcement down")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_take_down(id.clone(), window, cx)
                                })),
                        )
                    }),
            )
            // Line breaks as typed, the same as a comment.
            .child(div().text_sm().child(a.body.clone()))
    }

    fn render_away(&self, d: &Dashboard, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let now: Vec<&Absence> = d.away.iter().filter(|a| a.away_now).collect();
        let soon: Vec<&Absence> = d.away.iter().filter(|a| !a.away_now).collect();
        let group = |label: &'static str, rows: &[&Absence], cx: &mut Context<Self>| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .children(
                    rows.iter()
                        .map(|a| away_row(a, &theme, cx).into_any_element())
                        .collect::<Vec<_>>(),
                )
        };
        card(&theme)
            .w(px(300.))
            .flex_none()
            .child(section_title("Away", Lucide::Plane, theme.blue, &theme))
            .when(d.away.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Everyone's here for the next fortnight."),
                )
            })
            .when(!now.is_empty(), |this| {
                this.child(group("Right now", &now, cx))
            })
            .when(!soon.is_empty(), |this| {
                this.child(group("Coming up", &soon, cx))
            })
    }

    /// One of the escalation cards: nothing drawn when it's empty — a card
    /// for a filter that matches nothing is clutter, not reassurance.
    fn render_tasks(
        &self,
        label: &'static str,
        icon: Lucide,
        color: Hsla,
        tasks: &[DashboardTask],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if tasks.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        Some(
            card(&theme)
                .child(
                    section_title(label, icon, color, &theme).child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .child(format!("{}", tasks.len())),
                    ),
                )
                .child(
                    v_flex().children(
                        tasks
                            .iter()
                            .map(|t| task_row(label, t, &theme, cx).into_any_element())
                            .collect::<Vec<_>>(),
                    ),
                )
                .into_any_element(),
        )
    }

    fn render_recent(&self, d: &Dashboard, cx: &mut Context<Self>) -> Option<AnyElement> {
        if d.recent.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        Some(
            card(&theme)
                .child(section_title(
                    "Recent activity",
                    Lucide::Activity,
                    theme.muted_foreground,
                    &theme,
                ))
                .child(
                    v_flex().children(
                        d.recent
                            .iter()
                            .map(|t| recent_row(t, &theme, cx).into_any_element())
                            .collect::<Vec<_>>(),
                    ),
                )
                .into_any_element(),
        )
    }
}

/// "3 Nov" for an ISO day; anything else as it came.
fn short_day(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|d| d.format("%-d %b").to_string())
        .unwrap_or_else(|_| day.to_owned())
}

/// "Today" reads oddly mid-sentence; dates stay as they are.
fn in_sentence(day: String) -> String {
    match day.as_str() {
        "Today" | "Tomorrow" | "Yesterday" => day.to_lowercase(),
        _ => day,
    }
}

/// Red past due, amber due today, plain otherwise — the colour is spent on
/// the date, as on the web.
fn due_label(t: &DashboardTask, theme: &Theme) -> Option<(String, Hsla)> {
    let day = in_sentence(human_day(t.due_on.as_deref()?));
    Some(if t.is_overdue {
        (format!("overdue · {day}"), theme.red)
    } else if t.is_due_today {
        ("due today".into(), theme.yellow)
    } else {
        (format!("due {day}"), theme.muted_foreground)
    })
}

fn pill(theme: &Theme) -> Div {
    h_flex()
        .flex_none()
        .gap_1()
        .px_1p5()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .text_xs()
}

fn task_row(
    card: &str,
    t: &DashboardTask,
    theme: &Theme,
    cx: &mut Context<DashboardView>,
) -> impl IntoElement + use<> {
    let id = t.id.clone();
    h_flex()
        .id(SharedString::from(format!("home-{card}-{}", t.id)))
        .h(px(30.))
        .px_2()
        .gap_2()
        .rounded(theme.radius)
        .text_sm()
        .cursor_pointer()
        .hover(|this| this.bg(theme.list_hover))
        .child(dot(status_color(&t.status, theme)))
        .child(div().flex_1().min_w_0().truncate().child(t.title.clone()))
        .child(
            div()
                .flex_none()
                .w(px(80.))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(status_label(&t.status).to_owned()),
        )
        .child(
            div()
                .flex_none()
                .w(px(120.))
                .truncate()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t.project_name.clone().unwrap_or_default()),
        )
        .child(
            div()
                .flex_none()
                .w(px(130.))
                .truncate()
                .text_right()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .when_some(due_label(t, theme), |this, (label, color)| {
                    this.text_color(color).child(label)
                }),
        )
        .child(
            // Whose move it is: shape, not colour.
            div()
                .flex_none()
                .w(px(150.))
                .child(if t.is_action_required {
                    pill(theme)
                        .child(Icon::new(IconName::ArrowRight).xsmall())
                        .child("Your action")
                } else {
                    pill(theme)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Lucide::Clock).xsmall())
                        .child(
                            div().truncate().child(
                                t.waiting_on
                                    .as_ref()
                                    .map(|p| format!("Waiting on {}", p.name()))
                                    .unwrap_or_else(|| "Unassigned".into()),
                            ),
                        )
                }),
        )
        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
            cx.emit(Open::Task {
                id: id.clone(),
                org_id: None,
                preview: event.click_count() < 2,
            });
        }))
}

fn recent_row(
    t: &RecentTask,
    theme: &Theme,
    cx: &mut Context<DashboardView>,
) -> impl IntoElement + use<> {
    let id = t.id.clone();
    h_flex()
        .id(SharedString::from(format!("home-recent-{}", t.id)))
        .h(px(30.))
        .px_2()
        .gap_2()
        .rounded(theme.radius)
        .text_sm()
        .cursor_pointer()
        .hover(|this| this.bg(theme.list_hover))
        .child(dot(status_color(&t.status, theme)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when(!t.is_open, |this| this.text_color(theme.muted_foreground))
                .child(t.title.clone()),
        )
        .when(!t.is_open, |this| {
            this.child(
                pill(theme)
                    .text_color(theme.muted_foreground)
                    .child("Closed"),
            )
        })
        .child(
            div()
                .flex_none()
                .w(px(80.))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(status_label(&t.status).to_owned()),
        )
        .child(
            div()
                .flex_none()
                .w(px(120.))
                .truncate()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t.project_name.clone().unwrap_or_default()),
        )
        .child(
            div()
                .flex_none()
                .w(px(130.))
                .truncate()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t.owner.as_ref().map(|p| p.name()).unwrap_or_default()),
        )
        .child(
            div()
                .flex_none()
                .w(px(50.))
                .text_right()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t.updated_at.map(ago_short).unwrap_or_default()),
        )
        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
            cx.emit(Open::Task {
                id: id.clone(),
                org_id: None,
                preview: event.click_count() < 2,
            });
        }))
}

fn away_row(
    a: &Absence,
    theme: &Theme,
    cx: &mut Context<DashboardView>,
) -> impl IntoElement + use<> {
    let person = a.person.clone();
    let name = person
        .as_ref()
        .map(|p| p.name())
        .unwrap_or_else(|| "Someone".into());
    v_flex()
        .id(SharedString::from(format!("home-away-{}", a.id)))
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .when(person.is_some(), |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(theme.list_hover))
        })
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} → {}",
                            short_day(&a.starts_on),
                            short_day(&a.ends_on)
                        )),
                ),
        )
        .when_some(a.note.clone(), |this, note| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note),
            )
        })
        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
            if let Some(p) = &person {
                cx.emit(Open::Person {
                    id: p.id.clone(),
                    org_id: None,
                    preview: event.click_count() < 2,
                });
            }
        }))
}

impl Focusable for DashboardView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for DashboardView {
    fn item_key(&self, _: &App) -> SharedString {
        "home:dashboard".into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        "Home".into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::House)
    }

    fn is_dirty(&self, _: &App) -> bool {
        false
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let d = self.data.as_ref()?;
        let yours = [&d.critical, &d.urgent, &d.high, &d.due_soon]
            .iter()
            .flat_map(|cards| cards.iter())
            .filter(|t| t.is_action_required)
            .map(|t| t.id.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len();
        let updated = self.loaded_at.map(ago_long).unwrap_or_default();
        Some(
            match yours {
                0 => format!("Updated {updated}"),
                n => format!("{n} waiting on you · updated {updated}"),
            }
            .into(),
        )
    }
}

impl Render for DashboardView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match (&self.data, &self.load_error) {
            (Some(d), _) => {
                let d = d.clone();
                let cards: Vec<AnyElement> = [
                    self.render_tasks(
                        "Critical",
                        Lucide::TriangleAlert,
                        theme.red,
                        &d.critical,
                        cx,
                    ),
                    self.render_tasks("Urgent", Lucide::ArrowUp, theme.yellow, &d.urgent, cx),
                    // Neutral: Critical and Urgent already spend red and amber.
                    self.render_tasks(
                        "High priority",
                        Lucide::ChevronUp,
                        theme.muted_foreground,
                        &d.high,
                        cx,
                    ),
                    self.render_tasks(
                        "Due soon",
                        Lucide::CalendarClock,
                        theme.muted_foreground,
                        &d.due_soon,
                        cx,
                    ),
                    self.render_tasks("Pinned", Lucide::Pin, theme.muted_foreground, &d.pinned, cx),
                ]
                .into_iter()
                .flatten()
                .collect();
                let nothing_yours = cards.is_empty();
                v_flex()
                    .gap_4()
                    .child(self.render_header(&d, cx))
                    .child(
                        h_flex()
                            .gap_4()
                            .items_start()
                            .flex_wrap()
                            .child(self.render_announcements(&d, cx))
                            .child(self.render_away(&d, cx)),
                    )
                    .children(cards)
                    .when(nothing_yours, |this| {
                        this.child(card(&theme).child(
                            div().text_sm().text_color(theme.muted_foreground).child(
                                "Nothing critical, urgent, high or due soon is yours right now.",
                            ),
                        ))
                    })
                    .children(self.render_recent(&d, cx))
                    .into_any_element()
            }
            (None, Some(e)) => v_flex()
                .gap_2()
                .child(div().text_lg().child("Couldn't load the front page"))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(e.clone()),
                )
                .child(
                    Button::new("home-retry")
                        .label("Try again")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
                .into_any_element(),
            (None, None) => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child("Loading…")
                .into_any_element(),
        };
        div()
            .id("home-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(div().max_w(px(1100.)).px_6().py_5().child(content))
    }
}
