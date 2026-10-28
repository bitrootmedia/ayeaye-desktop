//! The calendar in a tab, the web app's month grid: every visible task's due
//! date and who's away — the team's — and your own reminders, which nobody
//! else sees. Click a day for everything on it beside the grid.
//!
//! The grid is always six Monday-first weeks, the window the server allows
//! in one request (`GET …/calendar`, at most 42 days).

use std::collections::HashMap;

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, Theme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::models::{
    Absence, CalendarReminder, CalendarTask, CalendarWindow, priority_rank, status_label,
};
use crate::assets::Lucide;
use crate::home::HomeEvent;
use crate::people::hours::WEEKDAYS;
use crate::session::OrgCtx;
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;
use crate::ui::widgets::{dot, fmt_day, priority_color};
use gpui_kit::Task;

/// Chips in a day's cell before "+3 more"; the day's panel has the rest.
const CHIPS_PER_DAY: usize = 3;

impl EventEmitter<ItemEvent> for CalendarView {}
impl EventEmitter<HomeEvent> for CalendarView {}
impl EventEmitter<Open> for CalendarView {}

/// What falls on one day of the grid.
#[derive(Default)]
struct Day<'a> {
    tasks: Vec<&'a CalendarTask>,
    reminders: Vec<&'a CalendarReminder>,
    away: Vec<&'a Absence>,
}

impl Day<'_> {
    fn len(&self) -> usize {
        self.tasks.len() + self.reminders.len() + self.away.len()
    }
}

pub struct CalendarView {
    ctx: OrgCtx,
    /// The 1st of the month shown.
    month: NaiveDate,
    selected: NaiveDate,
    data: CalendarWindow,
    loaded: bool,
    load_error: Option<String>,
    loading: bool,
    focus_handle: FocusHandle,
    _tasks: Vec<Task<()>>,
}

fn first_of(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

/// The Monday on or before the 1st, where the six weeks start.
fn grid_start(month: NaiveDate) -> NaiveDate {
    month - Duration::days(month.weekday().num_days_from_monday() as i64)
}

fn shift_month(month: NaiveDate, by: i32) -> NaiveDate {
    let index = month.year() * 12 + month.month0() as i32 + by;
    NaiveDate::from_ymd_opt(index.div_euclid(12), index.rem_euclid(12) as u32 + 1, 1)
        .unwrap_or(month)
}

fn parse(day: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
}

impl CalendarView {
    pub fn new(ctx: OrgCtx, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let mut view = Self {
            ctx,
            month: first_of(today),
            selected: today,
            data: CalendarWindow::default(),
            loaded: false,
            load_error: None,
            loading: false,
            focus_handle: cx.focus_handle(),
            _tasks: Vec::new(),
        };
        view.reload(cx);
        view
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let month = self.month;
        let start = grid_start(month);
        let (from, to) = (fmt_day(start), fmt_day(start + Duration::days(41)));
        self.loading = true;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.calendar(&org_id, &from, &to) })
                .await;
            this.update(cx, |this, cx| {
                // Paged on meanwhile: that month's answer is on its way.
                if this.month != month {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(data) => {
                        this.data = data;
                        this.loaded = true;
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

    fn go_to(&mut self, month: NaiveDate, cx: &mut Context<Self>) {
        if month == self.month {
            return;
        }
        self.month = month;
        let today = Local::now().date_naive();
        self.selected = if first_of(today) == month {
            today
        } else {
            month
        };
        self.reload(cx);
    }

    /// Everything loaded, by day. An absence spans days, so it's on each.
    fn by_day(&self) -> HashMap<NaiveDate, Day<'_>> {
        let start = grid_start(self.month);
        let end = start + Duration::days(41);
        let mut days: HashMap<NaiveDate, Day> = HashMap::new();
        for t in &self.data.tasks {
            if let Some(d) = parse(&t.due_on) {
                days.entry(d).or_default().tasks.push(t);
            }
        }
        for r in &self.data.reminders {
            if let Some(d) = parse(&r.remind_on) {
                days.entry(d).or_default().reminders.push(r);
            }
        }
        for a in &self.data.away {
            let (Some(from), Some(to)) = (parse(&a.starts_on), parse(&a.ends_on)) else {
                continue;
            };
            let mut d = from.max(start);
            while d <= to.min(end) {
                days.entry(d).or_default().away.push(a);
                d += Duration::days(1);
            }
        }
        for day in days.values_mut() {
            day.tasks.sort_by_key(|t| priority_rank(&t.priority));
        }
        days
    }

    // ------------------------------------------------------------- render

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let month = self.month;
        let today = first_of(Local::now().date_naive());
        let web = self.ctx.session.web_url(&format!(
            "/orgs/{}/calendar?month={}",
            self.ctx.org_id(),
            month.format("%Y-%m")
        ));
        h_flex()
            .flex_none()
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("cal-prev")
                    .icon(IconName::ChevronLeft)
                    .ghost()
                    .small()
                    .tooltip("Previous month")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.go_to(shift_month(month, -1), cx)),
                    ),
            )
            .child(
                Button::new("cal-today")
                    .label("Today")
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.go_to(today, cx);
                        this.selected = Local::now().date_naive();
                    })),
            )
            .child(
                Button::new("cal-next")
                    .icon(IconName::ChevronRight)
                    .ghost()
                    .small()
                    .tooltip("Next month")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.go_to(shift_month(month, 1), cx)),
                    ),
            )
            .child(
                div()
                    .ml_2()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(month.format("%B %Y").to_string()),
            )
            .when(self.loading, |this| {
                this.child(div().ml_2().child(Spinner::new().small()))
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Tasks and who's away are everyone's; reminders are yours."),
            )
            .child(
                Button::new("cal-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .small()
                    .tooltip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                Button::new("cal-web")
                    .icon(IconName::ExternalLink)
                    .ghost()
                    .small()
                    .tooltip("Open in browser")
                    .on_click(move |_, _, cx| cx.open_url(&web)),
            )
    }

    fn render_grid(
        &self,
        days: &HashMap<NaiveDate, Day<'_>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let start = grid_start(self.month);
        let today = Local::now().date_naive();
        let empty = Day::default();
        let weeks: Vec<AnyElement> = (0..6)
            .map(|week| {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .children((0..7).map(|weekday| {
                        let date = start + Duration::days(week * 7 + weekday);
                        let day = days.get(&date).unwrap_or(&empty);
                        self.render_cell(date, day, date == today, weekday == 6, &theme, cx)
                            .into_any_element()
                    }))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .flex_none()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.muted.opacity(0.4))
                    .children(WEEKDAYS.iter().map(|w| {
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child(*w)
                    })),
            )
            .children(weeks)
    }

    fn render_cell(
        &self,
        date: NaiveDate,
        day: &Day<'_>,
        is_today: bool,
        last_column: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let in_month = date.month() == self.month.month();
        let selected = date == self.selected;
        let mut chips: Vec<AnyElement> = Vec::new();
        for t in &day.tasks {
            chips.push(task_chip(t, theme, cx).into_any_element());
        }
        for r in &day.reminders {
            chips.push(reminder_chip(r, theme, cx).into_any_element());
        }
        for a in &day.away {
            chips.push(absence_chip(a, date, theme, cx).into_any_element());
        }
        let overflow = chips.len().saturating_sub(CHIPS_PER_DAY);
        chips.truncate(CHIPS_PER_DAY);
        v_flex()
            .id(SharedString::from(format!("cal-day-{date}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .p_1()
            .gap(px(2.))
            .overflow_hidden()
            .border_b_1()
            .when(!last_column, |this| this.border_r_1())
            .border_color(theme.border)
            .when(!in_month, |this| this.bg(theme.muted.opacity(0.25)))
            .when(selected, |this| this.bg(theme.list_active))
            .cursor_pointer()
            .child(
                h_flex().child(
                    div()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_xs()
                        .when(!in_month, |this| this.text_color(theme.muted_foreground))
                        .when(is_today, |this| {
                            this.bg(theme.primary)
                                .text_color(theme.primary_foreground)
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .child(date.day().to_string()),
                ),
            )
            .children(chips)
            .when(overflow > 0, |this| {
                this.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("+{overflow} more")),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = date;
                cx.notify();
            }))
    }

    /// Everything on the selected day, untruncated.
    fn render_day(
        &self,
        day: Option<&Day<'_>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let date = self.selected;
        let heading = |label: &'static str| {
            div()
                .pt_2()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(label.to_uppercase())
        };
        let mut body = v_flex().gap_1();
        match day.filter(|d| d.len() > 0) {
            None => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Nothing on this day."),
                )
            }
            Some(day) => {
                if !day.tasks.is_empty() {
                    body = body.child(heading("Due"));
                    for t in &day.tasks {
                        body = body.child(task_line(t, &theme, cx));
                    }
                }
                if !day.reminders.is_empty() {
                    body = body.child(heading("Your reminders"));
                    for r in &day.reminders {
                        body = body.child(reminder_chip(r, &theme, cx).text_sm().h(px(26.)));
                    }
                }
                if !day.away.is_empty() {
                    body = body.child(heading("Away"));
                    for a in &day.away {
                        body = body.child(absence_line(a, &theme, cx));
                    }
                }
            }
        }
        v_flex()
            .id("cal-day-panel")
            .w(px(300.))
            .flex_none()
            .h_full()
            .p_3()
            .gap_1()
            .border_l_1()
            .border_color(theme.border)
            .overflow_y_scroll()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(date.format("%A %-d %B").to_string()),
            )
            .child(body)
    }
}

fn chip(id: String, theme: &Theme) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(id))
        .h(px(18.))
        .px_1()
        .gap_1()
        .flex_none()
        .rounded(px(3.))
        .text_xs()
        .overflow_hidden()
        .hover(|this| this.bg(theme.accent))
}

fn open_task(id: String) -> Open {
    Open::Task {
        id,
        org_id: None,
        preview: true,
    }
}

fn task_chip(t: &CalendarTask, theme: &Theme, cx: &mut Context<CalendarView>) -> Stateful<Div> {
    let id = t.id.clone();
    chip(format!("cal-task-{}", t.id), theme)
        .child(dot(priority_color(&t.priority, theme)).size(px(6.)))
        .child(div().min_w_0().truncate().child(t.title.clone()))
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(open_task(id.clone()))))
}

/// A standalone reminder has no task to open.
fn reminder_chip(
    r: &CalendarReminder,
    theme: &Theme,
    cx: &mut Context<CalendarView>,
) -> Stateful<Div> {
    let task = r.task_id.clone();
    chip(format!("cal-rem-{}", r.id), theme)
        .text_color(theme.muted_foreground)
        .child(Icon::new(IconName::Bell).xsmall().flex_none())
        .child(div().min_w_0().truncate().child(r.label()))
        .when_some(task, |this, id| {
            this.on_click(cx.listener(move |_, _, _, cx| cx.emit(open_task(id.clone()))))
        })
}

fn absence_chip(
    a: &Absence,
    date: NaiveDate,
    theme: &Theme,
    cx: &mut Context<CalendarView>,
) -> Stateful<Div> {
    let person = a.person.clone();
    chip(format!("cal-away-{}-{date}", a.id), theme)
        .text_color(theme.muted_foreground)
        .child(Icon::new(Lucide::Plane).xsmall().flex_none())
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(person.as_ref().map(|p| p.name()).unwrap_or_default()),
        )
        .when_some(person, |this, p| {
            this.on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(Open::Person {
                    id: p.id.clone(),
                    org_id: None,
                    preview: true,
                })
            }))
        })
}

fn task_line(t: &CalendarTask, theme: &Theme, cx: &mut Context<CalendarView>) -> impl IntoElement {
    let id = t.id.clone();
    v_flex()
        .id(SharedString::from(format!("cal-line-{}", t.id)))
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .hover(|this| this.bg(theme.list_hover))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(dot(priority_color(&t.priority, theme)))
                .child(div().flex_1().min_w_0().truncate().child(t.title.clone())),
        )
        .child(
            div()
                .pl_4()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(match &t.project_name {
                    Some(p) => format!("{} · {p}", status_label(&t.status)),
                    None => status_label(&t.status).to_owned(),
                }),
        )
        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
            cx.emit(Open::Task {
                id: id.clone(),
                org_id: None,
                preview: event.click_count() < 2,
            })
        }))
}

fn absence_line(a: &Absence, theme: &Theme, cx: &mut Context<CalendarView>) -> impl IntoElement {
    let person = a.person.clone();
    v_flex()
        .id(SharedString::from(format!("cal-away-line-{}", a.id)))
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .hover(|this| this.bg(theme.list_hover))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    Icon::new(Lucide::Plane)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(person.as_ref().map(|p| p.name()).unwrap_or_default()),
                ),
        )
        .child(
            div()
                .pl_6()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(match &a.note {
                    Some(note) => format!("{} → {} · {note}", a.starts_on, a.ends_on),
                    None => format!("{} → {}", a.starts_on, a.ends_on),
                }),
        )
        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
            if let Some(p) = &person {
                cx.emit(Open::Person {
                    id: p.id.clone(),
                    org_id: None,
                    preview: event.click_count() < 2,
                })
            }
        }))
}

impl Focusable for CalendarView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for CalendarView {
    fn item_key(&self, _: &App) -> SharedString {
        "calendar:month".into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        "Calendar".into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(IconName::Calendar)
    }

    fn is_dirty(&self, _: &App) -> bool {
        false
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        if !self.loaded {
            return None;
        }
        let month = self.month.month();
        let due = self
            .data
            .tasks
            .iter()
            .filter(|t| parse(&t.due_on).is_some_and(|d| d.month() == month))
            .count();
        Some(format!("{due} due in {}", self.month.format("%B")).into())
    }
}

impl Render for CalendarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let days = self.by_day();
        let grid = self.render_grid(&days, cx);
        let day = self.render_day(days.get(&self.selected), cx);
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(self.render_header(cx))
            .when_some(self.load_error.clone(), |this, e| {
                this.child(
                    div()
                        .mx_4()
                        .my_2()
                        .px_2()
                        .py_1p5()
                        .rounded(theme.radius)
                        .bg(theme.danger.opacity(0.12))
                        .text_xs()
                        .child(format!("Couldn't load the calendar: {e}")),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(grid)
                    .child(day),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        parse(s).unwrap()
    }

    #[::core::prelude::v1::test]
    fn the_grid_starts_on_the_monday_before_the_first() {
        // 1 Oct 2026 was a Thursday; 1 Jun 2026 a Monday.
        assert_eq!(grid_start(day("2026-10-01")), day("2026-09-28"));
        assert_eq!(grid_start(day("2026-06-01")), day("2026-06-01"));
        // Six weeks always cover the month, however it falls.
        for m in 1..=12 {
            let first = NaiveDate::from_ymd_opt(2026, m, 1).unwrap();
            let last = shift_month(first, 1) - Duration::days(1);
            assert!(grid_start(first) + Duration::days(41) >= last);
        }
    }

    #[::core::prelude::v1::test]
    fn months_page_across_years() {
        assert_eq!(shift_month(day("2026-12-01"), 1), day("2027-01-01"));
        assert_eq!(shift_month(day("2026-01-01"), -1), day("2025-12-01"));
        assert_eq!(shift_month(day("2026-10-01"), -13), day("2025-09-01"));
    }
}
