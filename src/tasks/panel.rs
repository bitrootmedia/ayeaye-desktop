//! The task list in the sidebar: a few views that answer "what's mine to do",
//! narrowed by project, filtered by typing.
//!
//! Every view is a server query — sorting and filtering are the server's
//! job, and the web app's views are the same queries. The text filter only
//! narrows what has already arrived.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::models::Task as TaskData;
use crate::api::models::priority_rank;
use crate::api::{ApiError, TaskQuery};
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::tasks::table::{NOBODY, TableFilters};
use crate::ui::open::Open;
use crate::ui::widgets::{dot, human_day, picker, priority_color};

const ROW_HEIGHT: f32 = 26.;
const LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskView {
    /// Tasks I own.
    Mine,
    /// Tasks where I am the one being asked to act.
    WaitingOnMe,
    /// Open tasks nobody is being asked to act on — the web app's triage.
    Triage,
    All,
}

impl TaskView {
    const ALL: [TaskView; 4] = [
        TaskView::WaitingOnMe,
        TaskView::Mine,
        TaskView::Triage,
        TaskView::All,
    ];

    fn key(self) -> &'static str {
        match self {
            TaskView::Mine => "mine",
            TaskView::WaitingOnMe => "waiting",
            TaskView::Triage => "triage",
            TaskView::All => "all",
        }
    }

    fn label(self) -> &'static str {
        match self {
            TaskView::Mine => "Mine",
            TaskView::WaitingOnMe => "Waiting on me",
            TaskView::Triage => "Triage",
            TaskView::All => "All open",
        }
    }
}

pub enum TasksPanelEvent {
    NewTask,
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for TasksPanel {}
impl EventEmitter<TasksPanelEvent> for TasksPanel {}

pub struct TasksPanel {
    ctx: OrgCtx,
    view: TaskView,
    project: Option<String>,
    tasks: Vec<TaskData>,
    matches: Vec<usize>,
    loading: bool,
    error: Option<String>,
    active_task: Option<String>,
    filter: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
    _load: Option<Task<()>>,
}

impl TasksPanel {
    pub fn new(ctx: OrgCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter tasks…")
                .clean_on_escape()
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &filter,
                window,
                |this: &mut Self, _, e: &InputEvent, _, cx| match e {
                    InputEvent::Change => this.update_matches(cx),
                    InputEvent::PressEnter { .. } => {
                        if let Some(task) = this.matches.first().and_then(|ix| this.tasks.get(*ix))
                        {
                            cx.emit(Open::Task {
                                id: task.id.clone(),
                                org_id: None,
                                preview: false,
                            });
                        }
                    }
                    _ => {}
                },
            ),
            cx.observe(&ctx.directory, |_, _, cx| cx.notify()),
        ];
        ctx.directory.update(cx, |d, cx| d.ensure_loaded(cx));
        let mut panel = Self {
            ctx,
            view: TaskView::WaitingOnMe,
            project: None,
            tasks: Vec::new(),
            matches: Vec::new(),
            loading: false,
            error: None,
            active_task: None,
            filter,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
            _load: None,
        };
        panel.refresh(cx);
        panel
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter.update(cx, |f, cx| {
            f.focus(window, cx);
            f.select_all(window, cx);
        });
    }

    pub fn set_active_task(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_task != id {
            self.active_task = id;
            cx.notify();
        }
    }

    /// A task changed in its tab; reflect it here without a refetch.
    pub fn task_changed(&mut self, task: &TaskData, cx: &mut Context<Self>) {
        if let Some(row) = self.tasks.iter_mut().find(|t| t.id == task.id) {
            *row = task.clone();
            self.update_matches(cx);
        }
    }

    /// This view in the task table, where there's room for every field.
    fn table_filters(&self) -> TableFilters {
        let me = self.ctx.me().map(str::to_owned);
        let (owner, needs) = match self.view {
            TaskView::Mine => (me, None),
            TaskView::WaitingOnMe => (None, me),
            TaskView::Triage => (None, Some(NOBODY.to_owned())),
            TaskView::All => (None, None),
        };
        TableFilters {
            owner,
            needs,
            project: self.project.clone(),
            ..Default::default()
        }
    }

    fn query(&self) -> TaskQuery {
        let me = self.ctx.me().map(str::to_owned);
        let mut q = TaskQuery {
            project_id: self.project.clone(),
            limit: LIMIT,
            ..Default::default()
        };
        match self.view {
            TaskView::Mine => q.owner_user_id = me,
            TaskView::WaitingOnMe => q.action_required_user_id = me,
            TaskView::Triage => q.action_required_unset = true,
            TaskView::All => {}
        }
        q
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // "Mine" and "Waiting on me" mean nothing until /api/me has answered.
        if matches!(self.view, TaskView::Mine | TaskView::WaitingOnMe) && self.ctx.me().is_none() {
            return;
        }
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let query = self.query();
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.tasks(&org_id, &query) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(mut tasks) => {
                        // Priority first, then most recently touched — the
                        // order a "what next" list wants.
                        tasks.sort_by(|a, b| {
                            priority_rank(&a.priority)
                                .cmp(&priority_rank(&b.priority))
                                .then(b.updated_at.cmp(&a.updated_at))
                        });
                        this.tasks = tasks;
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(TasksPanelEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.update_matches(cx);
            })
            .ok();
        }));
    }

    /// The session learned who "me" is; views that depend on it can load.
    pub fn set_me(&mut self, me: Option<String>, cx: &mut Context<Self>) {
        if self.ctx.session.me != me {
            self.ctx.session.me = me;
            self.refresh(cx);
        }
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        self.matches = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                let hay = format!(
                    "{} {}",
                    t.title.to_lowercase(),
                    t.project_name.as_deref().unwrap_or("").to_lowercase()
                );
                terms.iter().all(|term| hay.contains(term))
            })
            .map(|(ix, _)| ix)
            .collect();
        cx.notify();
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        range
            .filter_map(|row| {
                let task = self.tasks.get(*self.matches.get(row)?)?.clone();
                let id = task.id.clone();
                let active = self.active_task.as_deref() == Some(id.as_str());
                let meta = task
                    .due_on
                    .as_deref()
                    .map(human_day)
                    .or_else(|| task.project_name.clone().filter(|_| self.project.is_none()));
                Some(
                    h_flex()
                        .id(SharedString::from(format!("task-row-{id}")))
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px_3()
                        .gap_2()
                        .text_sm()
                        .cursor_pointer()
                        .when(active, |this| this.bg(theme.list_active))
                        .hover(|this| this.bg(theme.list_hover))
                        .child(dot(priority_color(&task.priority, &theme)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(!task.is_open, |this| {
                                    this.line_through().text_color(theme.muted_foreground)
                                })
                                .child(task.title.clone()),
                        )
                        .when_some(meta, |this, meta| {
                            this.child(
                                div()
                                    .flex_none()
                                    .max_w(px(90.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(meta),
                            )
                        })
                        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                            cx.emit(Open::Task {
                                id: id.clone(),
                                org_id: None,
                                preview: event.click_count() < 2,
                            });
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for TasksPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let directory = self.ctx.directory.read(cx);
        let projects: Vec<(String, String)> =
            std::iter::once((String::new(), "All projects".to_owned()))
                .chain(
                    directory
                        .projects
                        .iter()
                        .map(|p| (p.id.clone(), p.name.clone())),
                )
                .collect();
        let project_label = self
            .project
            .as_ref()
            .and_then(|id| directory.projects.iter().find(|p| &p.id == id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "All projects".into());
        let count = self.matches.len();
        let this = cx.entity().downgrade();

        v_flex()
            .key_context("TasksPanel")
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("TASKS"))
                    .child(div().child(format!("{count}")))
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("tasks-table")
                            .icon(Lucide::Table)
                            .ghost()
                            .xsmall()
                            .tooltip("Open as a table, with every field (⌘⌥T)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.emit(Open::TaskTable {
                                    filters: Some(this.table_filters()),
                                })
                            })),
                    )
                    .child(
                        Button::new("tasks-new")
                            .icon(IconName::Plus)
                            .ghost()
                            .xsmall()
                            .tooltip("New task (⌘⇧N)")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TasksPanelEvent::NewTask))),
                    )
                    .child(
                        Button::new("tasks-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                v_flex()
                    .px_1()
                    .pb_1()
                    .children(TaskView::ALL.iter().map(|view| {
                        let view = *view;
                        let selected = view == self.view;
                        h_flex()
                            .id(SharedString::from(format!("view-{}", view.key())))
                            .h(px(24.))
                            .px_2()
                            .gap_2()
                            .rounded(theme.radius)
                            .text_sm()
                            .cursor_pointer()
                            .when(selected, |this| this.bg(theme.list_active))
                            .hover(|this| this.bg(theme.list_hover))
                            .child(
                                Icon::new(match view {
                                    TaskView::WaitingOnMe => IconName::Inbox,
                                    TaskView::Mine => IconName::User,
                                    TaskView::Triage => IconName::CircleAlert,
                                    TaskView::All => IconName::LayoutDashboard,
                                })
                                .xsmall()
                                .text_color(theme.muted_foreground),
                            )
                            .child(view.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.view = view;
                                this.refresh(cx);
                            }))
                    })),
            )
            .child(h_flex().px_2().pb_1().child(picker(
                "tasks-project",
                project_label,
                projects,
                Some(self.project.clone().unwrap_or_default()),
                move |id, _, cx| {
                    this.update(cx, |this, cx| {
                        this.project = (!id.is_empty()).then_some(id);
                        this.refresh(cx);
                    })
                    .ok();
                },
            )))
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
            .child(if count > 0 {
                uniform_list("tasks-list", count, cx.processor(Self::render_rows))
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .into_any_element()
            } else {
                div()
                    .px_4()
                    .pt_6()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if self.loading {
                        "Loading…"
                    } else {
                        match self.view {
                            TaskView::WaitingOnMe => "Nothing is waiting on you.",
                            TaskView::Triage => "Every open task has someone on it.",
                            _ => "No tasks here.",
                        }
                    })
                    .into_any_element()
            })
    }
}
