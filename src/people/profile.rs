//! A colleague in a tab, for the question you ask before getting in touch:
//! is now a good time, and what are they on?
//!
//! Their own time of day and whether it's inside their working hours,
//! whether they're away, their status line, the timer they're running, and
//! their open and recently closed tasks — the last two only as far as *your*
//! task access reaches, so a colleague on a private project simply shows
//! fewer, and the page says "that you can see" rather than implying it's
//! everything (`services/profiles.py`).

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jiff::Timestamp;

use crate::api::ApiError;
use crate::api::models::{MemberProfile, ProfileTask, role_label, status_label};
use crate::assets::Lucide;
use crate::people::hours::{
    Availability, availability, back_in, convert_week, local_zone_name, read_only_grid,
    wall_clock_label, zone,
};
use crate::session::OrgCtx;
use crate::tasks::table::TableFilters;
use crate::tasks::view::{card, section_title};
use crate::ui::format::ago_long;
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;
use crate::ui::widgets::{avatar, dot, priority_color};

pub enum PersonViewEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<ItemEvent> for PersonView {}
impl EventEmitter<PersonViewEvent> for PersonView {}
impl EventEmitter<Open> for PersonView {}

pub struct PersonView {
    ctx: OrgCtx,
    user_id: String,
    profile: Option<MemberProfile>,
    /// Put together without the server's `/profile`: no status, no timer.
    partial: bool,
    load_error: Option<String>,
    loading: bool,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _tasks: Vec<Task<()>>,
}

impl PersonView {
    pub fn new(ctx: OrgCtx, user_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Their clock moves; a tab left open shouldn't keep saying 9:00.
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
        let mut view = Self {
            ctx,
            user_id,
            profile: None,
            partial: false,
            load_error: None,
            loading: false,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _tasks: vec![tick],
        };
        view.reload(window, cx);
        view
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let user_id = self.user_id.clone();
        self.loading = true;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match client.member_profile(&org_id, &user_id) {
                        Ok(profile) => Ok((profile, false)),
                        // A server older than `/profile` 404s it for everyone;
                        // the member list tells that apart from a stranger.
                        Err(ApiError::NotFound(m)) => client
                            .assembled_profile(&org_id, &user_id)?
                            .map(|p| (p, true))
                            .ok_or(ApiError::NotFound(m)),
                        Err(e) => Err(e),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((profile, partial)) => {
                        this.profile = Some(profile);
                        this.partial = partial;
                        this.load_error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(PersonViewEvent::AuthFailed(e)),
                    Err(ApiError::NotFound(_)) => {
                        this.load_error = Some(
                            "Nobody by that id here. They may have left this organisation.".into(),
                        )
                    }
                    Err(e) => this.load_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn is_me(&self) -> bool {
        self.ctx.me() == Some(self.user_id.as_str())
    }

    // ------------------------------------------------------------- render

    fn render_header(&self, p: &MemberProfile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let name = p.name();
        let web = self
            .ctx
            .session
            .web_url(&format!("/orgs/{}/people/{}", self.ctx.org_id(), p.id));
        let is_me = self.is_me();
        h_flex()
            .gap_4()
            .items_start()
            .child(avatar(&name, px(56.), &theme))
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
                            .child(if is_me { format!("{name} (you)") } else { name }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .when(p.display_name.is_some(), |this| {
                                this.child(
                                    div()
                                        .font_family(theme.mono_font_family.clone())
                                        .child(p.email.clone()),
                                )
                            })
                            .child(
                                div()
                                    .px_1p5()
                                    .rounded(theme.radius)
                                    .border_1()
                                    .border_color(theme.border)
                                    .child(role_label(&p.role).to_owned()),
                            )
                            .when_some(p.joined_at, |this, at| {
                                this.child(format!(
                                    "joined {}",
                                    at.with_timezone(&chrono::Local).format("%-d %b %Y")
                                ))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .pt_1()
                            .flex_wrap()
                            .when(is_me, |this| {
                                this.child(
                                    Button::new("person-account")
                                        .icon(Lucide::UserCog)
                                        .label("Edit your account")
                                        .small()
                                        .on_click(
                                            cx.listener(|_, _, _, cx| cx.emit(Open::Account)),
                                        ),
                                )
                            })
                            .child(
                                Button::new("person-web")
                                    .icon(IconName::ExternalLink)
                                    .label("Open in browser")
                                    .ghost()
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_url(&web)),
                            )
                            .when(self.loading, |this| this.child(Spinner::new().small())),
                    ),
            )
    }

    /// Their time, whether it's inside their hours, whether they're away,
    /// and what they've said about it — "message now or later".
    fn render_right_now(
        &self,
        p: &MemberProfile,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let (clock, state) =
            availability(p.timezone.as_deref(), &p.working_hours, Timestamp::now());
        let line = |icon: Lucide, text: String, muted: bool| {
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .when(muted, |this| this.text_color(theme.muted_foreground))
                .child(Icon::new(icon).small().mt(px(2.)))
                .child(div().flex_1().min_w_0().child(text))
        };
        let away_now: Vec<_> = p.away.iter().filter(|a| a.away_now).collect();
        let away_soon: Vec<_> = p.away.iter().filter(|a| !a.away_now).collect();

        card(&theme)
            .flex_1()
            .min_w(px(280.))
            .child(section_title(
                "Right now",
                Lucide::Clock,
                theme.blue,
                &theme,
            ))
            .child(match (clock, &p.timezone) {
                (Some(clock), Some(tz)) => h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_family(theme.mono_font_family.clone())
                            .child(wall_clock_label(clock)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("their time ({tz})")),
                    )
                    .into_any_element(),
                _ => div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No timezone on record yet.")
                    .into_any_element(),
            })
            .child(match state {
                Availability::Working => {
                    line(Lucide::Check, "Within their working hours".into(), false)
                        .child(dot(theme.green))
                }
                Availability::BackIn(h) => line(
                    Lucide::Clock,
                    format!("Outside their working hours — {}", back_in(h)),
                    true,
                ),
                Availability::NoHours | Availability::NoTimezone => {
                    line(Lucide::Clock, "No working hours recorded.".into(), true)
                }
            })
            .children(away_now.iter().map(|a| {
                line(
                    Lucide::Plane,
                    match &a.note {
                        Some(note) => format!("Away until {} — {note}", a.ends_on),
                        None => format!("Away until {}", a.ends_on),
                    },
                    false,
                )
            }))
            .when_some(p.status_message.clone(), |this, status| {
                this.child(line(Lucide::MessageSquareQuote, status, false))
            })
            .when(!away_soon.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .pt_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Away soon"),
                        )
                        .children(away_soon.iter().map(|a| {
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .font_family(theme.mono_font_family.clone())
                                        .child(format!("{} → {}", a.starts_on, a.ends_on)),
                                )
                                .when_some(a.note.clone(), |this, note| {
                                    this.child(div().text_color(theme.muted_foreground).child(note))
                                })
                        })),
                )
            })
    }

    /// Their running timer — only when it's on a task you can see, so "none
    /// you can see" is the honest empty state, not "idle".
    fn render_working_on(
        &self,
        p: &MemberProfile,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        card(&theme)
            .flex_1()
            .min_w(px(280.))
            .child(section_title(
                "Working on",
                Lucide::Timer,
                theme.green,
                &theme,
            ))
            .child(match &p.working_on {
                Some(on) => v_flex()
                    .gap_1()
                    .child(self.task_row("working-on", &on.task, false, cx))
                    .when_some(on.started_at, |this, at| {
                        this.child(
                            div()
                                .px_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "Timer running since {} ({})",
                                    at.with_timezone(&chrono::Local).format("%H:%M"),
                                    ago_long(at)
                                )),
                        )
                    })
                    .into_any_element(),
                None => div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if self.partial {
                        "This server doesn't share running timers yet."
                    } else {
                        "No timer running on anything you can see."
                    })
                    .into_any_element(),
            })
    }

    fn task_row(
        &self,
        prefix: &str,
        task: &ProfileTask,
        closed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = task.id.clone();
        let when = if closed {
            task.closed_at.map(|at| format!("closed {}", ago_long(at)))
        } else {
            task.updated_at.map(ago_long)
        };
        h_flex()
            .id(SharedString::from(format!("{prefix}-{}", task.id)))
            .h(px(30.))
            .px_2()
            .gap_2()
            .rounded(theme.radius)
            .text_sm()
            .cursor_pointer()
            .hover(|this| this.bg(theme.list_hover))
            .child(dot(priority_color(&task.priority, &theme)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(closed, |this| this.text_color(theme.muted_foreground))
                    .child(task.title.clone()),
            )
            .when(task.is_action_required, |this| {
                this.child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .child("their action"),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(px(80.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(status_label(&task.status).to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(110.))
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(task.project_name.clone().unwrap_or_default()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(110.))
                    .truncate()
                    .text_right()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(when.unwrap_or_default()),
            )
            .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                cx.emit(Open::Task {
                    id: id.clone(),
                    org_id: None,
                    preview: event.click_count() < 2,
                });
            }))
    }

    fn render_tasks(&self, p: &MemberProfile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let mine = self.is_me();
        let shown = p.open_tasks.len() as i64;
        let user_id = p.id.clone();
        card(&theme)
            .child(
                section_title(
                    // Without `/profile`, only what they own is known.
                    if self.partial {
                        "Open tasks they own"
                    } else {
                        "Open tasks"
                    },
                    Lucide::ListTodo,
                    theme.green,
                    &theme,
                )
                .child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .child(format!("{}", p.open_total)),
                ),
            )
            .when(p.open_tasks.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(if mine {
                            "Nothing open of yours."
                        } else {
                            "Nothing open of theirs that you can see."
                        }),
                )
            })
            .child(
                v_flex().children(
                    p.open_tasks
                        .iter()
                        .map(|t| self.task_row("open", t, false, cx).into_any_element())
                        .collect::<Vec<_>>(),
                ),
            )
            .when(p.open_total > shown, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{shown} of {} you can see, most recently updated first.",
                            p.open_total
                        ))
                        .child(
                            Button::new("person-owned")
                                .label("Tasks they own")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(Open::TaskTable {
                                        filters: Some(TableFilters {
                                            owner: Some(user_id.clone()),
                                            ..Default::default()
                                        }),
                                    })
                                })),
                        ),
                )
            })
    }

    fn render_closed(&self, p: &MemberProfile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        card(&theme)
            .child(section_title(
                "Recently closed",
                Lucide::SquareCheck,
                theme.muted_foreground,
                &theme,
            ))
            .child(
                v_flex().children(
                    p.recently_closed
                        .iter()
                        .map(|t| self.task_row("closed", t, true, cx).into_any_element())
                        .collect::<Vec<_>>(),
                ),
            )
    }

    /// Their grid as they recorded it, and again in your zone when the two
    /// differ — the same rule as the web app's.
    fn render_hours(&self, p: &MemberProfile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let mine = local_zone_name();
        let converted = match (p.timezone.as_deref(), mine.as_deref()) {
            (Some(theirs), Some(mine)) if theirs != mine && !p.working_hours.is_empty() => {
                match (zone(theirs), zone(mine)) {
                    (Some(from), Some(to)) => Some((
                        mine.to_owned(),
                        convert_week(&p.working_hours, &from, &to, Timestamp::now()),
                    )),
                    _ => None,
                }
            }
            _ => None,
        };
        card(&theme)
            .child(section_title(
                "Working hours",
                Lucide::Clock,
                theme.blue,
                &theme,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(match &p.timezone {
                        Some(tz) => format!("Recorded in {tz}."),
                        None => "No timezone on record — shown as entered.".into(),
                    }),
            )
            .child(read_only_grid("theirs", &p.working_hours, cx))
            .when(p.working_hours.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Nothing recorded yet."),
                )
            })
            .when_some(converted, |this, (mine, cells)| {
                this.child(
                    v_flex()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("In your timezone ({mine})")),
                        )
                        .child(read_only_grid("yours", &cells, cx)),
                )
            })
    }
}

impl Focusable for PersonView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for PersonView {
    fn item_key(&self, _: &App) -> SharedString {
        format!("person:{}", self.user_id).into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        self.profile
            .as_ref()
            .map(MemberProfile::name)
            .unwrap_or_else(|| "Person".into())
            .into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(IconName::User)
    }

    fn is_dirty(&self, _: &App) -> bool {
        false
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let p = self.profile.as_ref()?;
        let (clock, _) = availability(p.timezone.as_deref(), &p.working_hours, Timestamp::now());
        Some(
            match clock {
                Some(clock) => format!("Their time {}", wall_clock_label(clock)),
                None => role_label(&p.role).to_owned(),
            }
            .into(),
        )
    }
}

impl Render for PersonView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match (&self.profile, &self.load_error) {
            (Some(p), _) => {
                let p = p.clone();
                v_flex()
                    .gap_4()
                    .child(self.render_header(&p, cx))
                    .child(
                        h_flex()
                            .gap_4()
                            .items_start()
                            .flex_wrap()
                            .child(self.render_right_now(&p, cx))
                            .child(self.render_working_on(&p, cx)),
                    )
                    .child(self.render_tasks(&p, cx))
                    .when(!p.recently_closed.is_empty(), |this| {
                        this.child(self.render_closed(&p, cx))
                    })
                    .child(self.render_hours(&p, cx))
                    .into_any_element()
            }
            (None, Some(e)) => v_flex()
                .gap_2()
                .child(div().text_lg().child("Couldn't open this person"))
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
            .id("person-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(div().max_w(px(900.)).px_6().py_5().child(content))
    }
}
