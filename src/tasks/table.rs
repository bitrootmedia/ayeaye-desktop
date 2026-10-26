//! The task table: every task you can see, as the web app's list — a column
//! per field, sorted by any of them, filtered on all of them, in a tab of its
//! own where there's room for it.
//!
//! Sorting and filtering are the server's, as in the sidebar: the rows are a
//! page, and ordering a page locally would only order the rows you happen to
//! hold. The count of everything matching comes back with each page and is
//! always on screen. Only the text filter is local, and says so.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::actions::{OpenSelected, SelectNext};
use crate::api::models::{
    PRIORITIES, Person, STATUSES, Tag, Task as TaskData, priority_label, status_label,
};
use crate::api::{ApiError, TaskQuery};
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::format::{ago_short, stamp};
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;
use crate::ui::widgets::{avatar, dot, human_day, priority_color, status_color};

/// Rows per page, and how many more each "Show more" asks for. A bigger
/// window rather than a next page: one code path, and it can't drift out of
/// order when something changes underneath.
const PAGE: usize = 100;

/// `project` for tasks in no project — the server's `loose`.
pub const NO_PROJECT: &str = "__loose__";
/// `needs` for tasks asking nothing of anyone — the Triage queue.
pub const NOBODY: &str = "__nobody__";

/// What the table is showing. Saved per organisation, so it reopens as left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableFilters {
    /// A project id, or [`NO_PROJECT`].
    pub project: Option<String>,
    pub tag: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    /// The owner's user id.
    pub owner: Option<String>,
    /// Who must act: a user id, or [`NOBODY`].
    pub needs: Option<String>,
    pub closed: bool,
    pub sort: Option<String>,
    pub descending: bool,
}

impl TableFilters {
    fn query(&self, limit: usize) -> TaskQuery {
        TaskQuery {
            project_id: self.project.clone().filter(|p| p != NO_PROJECT),
            loose: self.project.as_deref() == Some(NO_PROJECT),
            tag_id: self.tag.clone(),
            owner_user_id: self.owner.clone(),
            action_required_user_id: self.needs.clone().filter(|n| n != NOBODY),
            action_required_unset: self.needs.as_deref() == Some(NOBODY),
            include_closed: self.closed,
            status: self.status.clone(),
            priority: self.priority.clone(),
            sort: self.sort.clone(),
            descending: self.descending,
            limit,
        }
    }

    /// Anything narrowing the list beyond "open tasks".
    fn is_narrowed(&self) -> bool {
        self.project.is_some()
            || self.tag.is_some()
            || self.status.is_some()
            || self.priority.is_some()
            || self.owner.is_some()
            || self.needs.is_some()
    }
}

/// `(key, label, width)`. The key is the server's sort key too.
const COLUMNS: &[(&str, &str, f32)] = &[
    ("title", "Task", 380.),
    ("project", "Project", 150.),
    ("status", "Status", 120.),
    ("priority", "Priority", 100.),
    ("owner", "Owner", 160.),
    ("action_required", "Action required", 160.),
    ("due_on", "Due", 100.),
    ("created_at", "Created", 120.),
    ("updated_at", "Updated", 90.),
];

pub enum TaskTableEvent {
    FiltersChanged(TableFilters),
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for TaskTable {}
impl EventEmitter<ItemEvent> for TaskTable {}
impl EventEmitter<TaskTableEvent> for TaskTable {}

pub struct TaskTable {
    ctx: OrgCtx,
    filters: TableFilters,
    tags: Vec<Tag>,
    table: Entity<TableState<TaskRows>>,
    search: Entity<InputState>,
    limit: usize,
    total: usize,
    loading: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
    _load: Option<Task<()>>,
    _tags: Option<Task<()>>,
}

impl TaskTable {
    pub fn new(
        ctx: OrgCtx,
        filters: TableFilters,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = TaskRows {
            table: cx.entity().downgrade(),
            web_base: ctx
                .session
                .web_url(&format!("/orgs/{}/tasks/", ctx.org_id())),
            tasks: Vec::new(),
            rows: Vec::new(),
            loading: true,
            has_more: false,
            initial_sort: filters.sort.clone().map(|s| (s, filters.descending)),
        };
        let table = cx.new(|cx| {
            TableState::new(rows, window, cx)
                .col_selectable(false)
                .col_movable(false)
                .row_header(false)
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter these rows…")
                .clean_on_escape()
        });
        let subscriptions = vec![
            cx.subscribe_in(&table, window, |this, _, e: &TableEvent, _, cx| {
                if let TableEvent::DoubleClickedRow(ix) = e {
                    this.open_row(*ix, cx);
                }
            }),
            cx.subscribe_in(&search, window, |this, _, e: &InputEvent, _, cx| match e {
                InputEvent::Change => this.update_rows(cx),
                InputEvent::PressEnter { .. } => this.open_row(0, cx),
                _ => {}
            }),
            cx.observe(&ctx.directory, |_, _, cx| cx.notify()),
        ];
        ctx.directory.update(cx, |d, cx| d.ensure_loaded(cx));
        let mut view = Self {
            ctx,
            filters,
            tags: Vec::new(),
            table,
            search,
            limit: PAGE,
            total: 0,
            loading: false,
            error: None,
            _subscriptions: subscriptions,
            _load: None,
            _tags: None,
        };
        view.load_tags(cx);
        view.refresh(cx);
        view
    }

    /// Shows `filters` — the sidebar's "Open as table" — keeping this
    /// table's own sort and closed toggle, which the sidebar doesn't have.
    pub fn show(&mut self, filters: TableFilters, cx: &mut Context<Self>) {
        self.update_filters(cx, |f| {
            *f = TableFilters {
                sort: f.sort.clone(),
                descending: f.descending,
                closed: f.closed,
                ..filters
            }
        });
    }

    fn update_filters(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut TableFilters)) {
        let before = self.filters.clone();
        f(&mut self.filters);
        if self.filters != before {
            self.filters_changed(cx);
        }
    }

    fn filters_changed(&mut self, cx: &mut Context<Self>) {
        self.limit = PAGE;
        cx.emit(TaskTableEvent::FiltersChanged(self.filters.clone()));
        self.refresh(cx);
    }

    /// A task changed in its tab; reflect it here without a refetch.
    pub fn task_changed(&mut self, task: &TaskData, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            if let Some(row) = t.delegate_mut().tasks.iter_mut().find(|t| t.id == task.id) {
                *row = task.clone();
                cx.notify();
            }
        });
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let query = self.filters.query(self.limit);
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.tasks_counted(&org_id, &query) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((tasks, total)) => {
                        this.total = total;
                        this.error = None;
                        let has_more = tasks.len() < total;
                        this.table.update(cx, |t, _| {
                            let rows = t.delegate_mut();
                            rows.tasks = tasks;
                            rows.has_more = has_more;
                        });
                    }
                    Err(e) if e.is_auth() => cx.emit(TaskTableEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.table
                    .update(cx, |t, _| t.delegate_mut().loading = false);
                this.update_rows(cx);
            })
            .ok();
        }));
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.table.read(cx).delegate().tasks.len() >= self.total {
            return;
        }
        self.limit += PAGE;
        self.refresh(cx);
    }

    fn load_tags(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self._tags = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.tags(&org_id) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(mut tags) = result {
                    tags.sort_by_key(|t| t.name.to_lowercase());
                    this.tags = tags;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The text filter: narrows the rows already here, nothing more.
    fn update_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let terms: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
        self.table.update(cx, |t, cx| {
            let rows = t.delegate_mut();
            rows.rows = rows
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    let hay = format!(
                        "{} {} {} {}",
                        t.title,
                        t.project_name.as_deref().unwrap_or(""),
                        t.tags
                            .iter()
                            .map(|t| t.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" "),
                        t.owner.as_ref().map(Person::name).unwrap_or_default(),
                    )
                    .to_lowercase();
                    terms.iter().all(|term| hay.contains(term.as_str()))
                })
                .map(|(ix, _)| ix)
                .collect();
            cx.notify();
        });
        cx.notify();
    }

    fn open_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let id = self
            .table
            .read(cx)
            .delegate()
            .task_at(row)
            .map(|t| t.id.clone());
        if let Some(id) = id {
            cx.emit(Open::Task {
                id,
                org_id: None,
                preview: false,
            });
        }
    }

    fn action_open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.table.read(cx).selected_row() {
            self.open_row(row, cx);
        }
    }

    /// ↓ from the text filter moves into the rows.
    fn action_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            if t.delegate().rows_count(cx) > 0 {
                t.set_selected_row(0, cx);
            }
            window.focus(&t.focus_handle(cx), cx);
        });
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let me = self.ctx.me().map(str::to_owned);
        let directory = self.ctx.directory.read(cx);
        let f = &self.filters;

        // The sidebar's views, as shortcuts over the same filters.
        let presets: [(&str, &str, Option<String>, Option<String>); 4] = [
            ("waiting", "Waiting on me", None, me.clone()),
            ("mine", "Mine", me.clone(), None),
            ("triage", "Triage", None, Some(NOBODY.to_owned())),
            ("all", "Everything", None, None),
        ];
        let preset_buttons = presets.into_iter().map(|(key, label, owner, needs)| {
            let selected = f.owner == owner && f.needs == needs;
            Button::new(SharedString::from(format!("table-preset-{key}")))
                .label(label)
                .ghost()
                .small()
                .selected(selected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let (owner, needs) = (owner.clone(), needs.clone());
                    this.update_filters(cx, |f| {
                        f.owner = owner;
                        f.needs = needs;
                    });
                }))
        });

        let people: Vec<(String, String)> = directory
            .members
            .iter()
            .filter_map(|m| Some((m.user_id.clone()?, m.name())))
            .collect();
        let person_name = |id: &str| {
            if me.as_deref() == Some(id) {
                return "Me".to_owned();
            }
            people
                .iter()
                .find(|(k, _)| k == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "Someone".into())
        };
        let project_name = |id: &str| {
            if id == NO_PROJECT {
                return "No project".to_owned();
            }
            directory
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "A project".into())
        };
        let with_me = |label: &str| -> Vec<(String, String)> {
            let mut out = vec![(String::new(), label.to_owned())];
            if let Some(me) = &me {
                out.push((me.clone(), "Me".into()));
            }
            out.extend(
                people
                    .iter()
                    .filter(|(k, _)| Some(k) != me.as_ref())
                    .cloned(),
            );
            out
        };

        let projects: Vec<(String, String)> = [
            (String::new(), "Any project".to_owned()),
            (NO_PROJECT.to_owned(), "No project".to_owned()),
        ]
        .into_iter()
        .chain(
            directory
                .projects
                .iter()
                .map(|p| (p.id.clone(), p.name.clone())),
        )
        .collect();
        let tags: Vec<(String, String)> = std::iter::once((String::new(), "Any tag".to_owned()))
            .chain(self.tags.iter().map(|t| {
                let label = if t.off_board {
                    format!("{} (off the board)", t.name)
                } else {
                    t.name.clone()
                };
                (t.id.clone(), label)
            }))
            .collect();
        let statuses: Vec<(String, String)> = std::iter::once((String::new(), "Any status".into()))
            .chain(STATUSES.iter().map(|(k, l)| (k.to_string(), l.to_string())))
            .collect();
        let priorities: Vec<(String, String)> =
            std::iter::once((String::new(), "Any priority".into()))
                .chain(
                    PRIORITIES
                        .iter()
                        .map(|(k, l)| (k.to_string(), l.to_string())),
                )
                .collect();
        let mut needs = with_me("Anyone");
        needs.insert(1, (NOBODY.to_owned(), "Nobody (triage)".into()));

        let tag_name = |id: &str| {
            self.tags
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "A tag".into())
        };
        let needs_name = |id: &str| {
            if id == NOBODY {
                "Nobody".to_owned()
            } else {
                person_name(id)
            }
        };

        let this = cx.entity().downgrade();
        let pick = |field: fn(&mut TableFilters) -> &mut Option<String>| {
            let this = this.clone();
            move |key: String, _: &mut Window, cx: &mut App| {
                this.update(cx, |t, cx| {
                    t.update_filters(cx, |f| *field(f) = (!key.is_empty()).then_some(key))
                })
                .ok();
            }
        };

        v_flex()
            .flex_none()
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_1()
                    .children(preset_buttons)
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        div().w(px(240.)).child(
                            Input::new(&self.search)
                                .small()
                                .prefix(
                                    Icon::new(IconName::Search)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                )
                                .cleanable(true),
                        ),
                    )
                    .child(
                        Button::new("table-closed")
                            .icon(if f.closed { IconName::Eye } else { IconName::EyeOff })
                            .label(if f.closed { "Hide closed" } else { "Show closed" })
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_filters(cx, |f| f.closed = !f.closed)
                            })),
                    )
                    .child(
                        Button::new("table-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .small()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_tags(cx);
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(filter_picker(
                        "table-project",
                        "Project",
                        f.project.as_deref().map(project_name),
                        projects,
                        f.project.clone(),
                        pick(|f| &mut f.project),
                    ))
                    .when(!self.tags.is_empty(), |this| {
                        this.child(filter_picker(
                            "table-tag",
                            "Tag",
                            f.tag.as_deref().map(tag_name),
                            tags,
                            f.tag.clone(),
                            pick(|f| &mut f.tag),
                        ))
                    })
                    .child(filter_picker(
                        "table-status",
                        "Status",
                        f.status.as_deref().map(|s| status_label(s).to_owned()),
                        statuses,
                        f.status.clone(),
                        pick(|f| &mut f.status),
                    ))
                    .child(filter_picker(
                        "table-priority",
                        "Priority",
                        f.priority.as_deref().map(|p| priority_label(p).to_owned()),
                        priorities,
                        f.priority.clone(),
                        pick(|f| &mut f.priority),
                    ))
                    .child(filter_picker(
                        "table-owner",
                        "Owner",
                        f.owner.as_deref().map(person_name),
                        with_me("Anyone"),
                        f.owner.clone(),
                        pick(|f| &mut f.owner),
                    ))
                    .child(filter_picker(
                        "table-needs",
                        "Action required",
                        f.needs.as_deref().map(needs_name),
                        needs,
                        f.needs.clone(),
                        pick(|f| &mut f.needs),
                    ))
                    .when(f.is_narrowed(), |this| {
                        this.child(
                            Button::new("table-clear")
                                .label("Clear filters")
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_filters(cx, |f| {
                                        *f = TableFilters {
                                            closed: f.closed,
                                            sort: f.sort.clone(),
                                            descending: f.descending,
                                            ..Default::default()
                                        }
                                    })
                                })),
                        )
                    }),
            )
            // Off-board tasks are absent unless their tag is asked for by
            // name, so say which mode this is rather than leave a silent gap.
            .when_some(
                f.tag
                    .as_ref()
                    .and_then(|id| self.tags.iter().find(|t| &t.id == id && t.off_board)),
                |this, tag| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "Showing {}, which is kept off the board — these aren't in the other views.",
                                tag.name
                            )),
                    )
                },
            )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rows = self.table.read(cx).delegate();
        let (shown, loaded) = (rows.rows.len(), rows.tasks.len());
        // Said out loud, always: a list that silently stops at a hundred is
        // one you make decisions from without knowing it.
        let count = if shown < loaded {
            format!("{shown} matching · {loaded} of {} loaded", self.total)
        } else {
            format!("{loaded} of {}", self.total)
        };
        h_flex()
            .flex_none()
            .h(px(34.))
            .px_4()
            .gap_3()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(count)
            .when(loaded < self.total, |this| {
                this.child(
                    Button::new("table-more")
                        .label("Show more")
                        .outline()
                        .xsmall()
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
                )
            })
            .child(div().flex_1())
            .child("↑↓ to move · Return or double-click to open · right-click for more")
    }
}

/// A filter as a button that says what it's set to, highlighted when set.
fn filter_picker(
    id: &'static str,
    name: &'static str,
    value: Option<String>,
    options: Vec<(String, String)>,
    selected: Option<String>,
    on_pick: impl Fn(String, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_pick = std::rc::Rc::new(on_pick);
    let label = match &value {
        Some(v) => format!("{name}: {v}"),
        None => name.to_owned(),
    };
    let selected = selected.unwrap_or_default();
    Button::new(id)
        .label(label)
        .icon(IconName::ChevronDown)
        .outline()
        .xsmall()
        .selected(value.is_some())
        .dropdown_menu(move |mut menu, _, _| {
            for (key, label) in &options {
                let on_pick = on_pick.clone();
                let key = key.clone();
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(selected == key)
                        .on_click(move |_, window, cx| on_pick(key.clone(), window, cx)),
                );
            }
            menu.max_h(px(360.)).scrollable(true)
        })
}

impl Focusable for TaskTable {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.read(cx).focus_handle(cx)
    }
}

impl Item for TaskTable {
    fn item_key(&self, _: &App) -> SharedString {
        "tasks:table".into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        "All tasks".into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::Table)
    }

    fn is_dirty(&self, _: &App) -> bool {
        false
    }

    fn status_text(&self, cx: &App) -> Option<SharedString> {
        let loaded = self.table.read(cx).delegate().tasks.len();
        Some(format!("{loaded} of {} tasks", self.total).into())
    }
}

impl Render for TaskTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .key_context("TaskTable")
            .on_action(cx.listener(Self::action_open_selected))
            .on_action(cx.listener(Self::action_select_next))
            .size_full()
            .bg(theme.background)
            .child(self.render_toolbar(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .mx_4()
                        .mt_2()
                        .px_2()
                        .py_1p5()
                        .rounded(theme.radius)
                        .bg(theme.danger.opacity(0.12))
                        .text_xs()
                        .child(error),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.table)
                        .bordered(false)
                        .stripe(true)
                        .small(),
                ),
            )
            .child(self.render_footer(cx))
    }
}

// ------------------------------------------------------------------ rows

pub struct TaskRows {
    table: WeakEntity<TaskTable>,
    /// `<server>/orgs/<org>/tasks/`, for "Open in browser" and "Copy link".
    web_base: String,
    /// The page, in the server's order.
    tasks: Vec<TaskData>,
    /// Indices into `tasks` that pass the text filter.
    rows: Vec<usize>,
    loading: bool,
    has_more: bool,
    /// The sort the header shows when the columns are (re)built.
    initial_sort: Option<(String, bool)>,
}

impl TaskRows {
    fn task_at(&self, row: usize) -> Option<&TaskData> {
        self.tasks.get(*self.rows.get(row)?)
    }

    /// Runs `f` on the table view once the table's own update is over:
    /// refetching touches the table again, which can't happen inside it.
    fn defer(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut TaskTable, &mut Context<TaskTable>) + 'static,
    ) {
        let table = self.table.clone();
        cx.defer(move |cx| {
            table.update(cx, f).ok();
        });
    }
}

impl TableDelegate for TaskRows {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, label, width) = COLUMNS[col_ix];
        let sort = match &self.initial_sort {
            Some((k, false)) if k == key => ColumnSort::Ascending,
            Some((k, true)) if k == key => ColumnSort::Descending,
            _ => ColumnSort::Default,
        };
        let column = Column::new(key, label).width(px(width)).sort(sort);
        if col_ix == 0 {
            column.fixed_left().min_width(px(160.))
        } else {
            column
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let key = COLUMNS[col_ix].0.to_owned();
        let (sort, descending) = match sort {
            ColumnSort::Default => (None, false),
            ColumnSort::Ascending => (Some(key), false),
            ColumnSort::Descending => (Some(key), true),
        };
        self.initial_sort = sort.clone().map(|s| (s, descending));
        self.defer(cx, move |table, cx| {
            table.update_filters(cx, |f| {
                f.sort = sort;
                f.descending = descending;
            })
        });
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id(("task-row", row_ix)).cursor_pointer()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(task) = self.task_at(row_ix) else {
            return div().into_any_element();
        };
        let muted = |text: String| {
            div()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        };
        let person = |p: &Option<Person>| match p {
            Some(p) => h_flex()
                .gap_1p5()
                .min_w_0()
                .child(avatar(&p.name(), px(16.), &theme))
                .child(div().truncate().child(p.name()))
                .into_any_element(),
            None => muted("—".into()),
        };
        match COLUMNS[col_ix].0 {
            "title" => h_flex()
                .gap_1p5()
                .min_w_0()
                .child(
                    div()
                        .flex_shrink(1.)
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .when(!task.is_open, |this| {
                            this.line_through().text_color(theme.muted_foreground)
                        })
                        .child(task.title.clone()),
                )
                .when(task.is_hidden, |this| {
                    this.child(
                        Icon::new(IconName::EyeOff)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                })
                .when(task.is_pinned, |this| {
                    this.child(Icon::new(IconName::Star).xsmall().text_color(theme.yellow))
                })
                .children(task.tags.iter().take(3).map(|tag| {
                    div()
                        .flex_none()
                        .px_1p5()
                        .rounded(theme.radius)
                        .bg(theme.secondary)
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(tag.name.clone())
                }))
                .into_any_element(),
            "project" => match &task.project_name {
                Some(name) => muted(name.clone()),
                None => div()
                    .italic()
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child("No project")
                    .into_any_element(),
            },
            "status" => {
                let color = status_color(&task.status, &theme);
                // The pill as wide as its words, not the column.
                h_flex()
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .px_1p5()
                            .rounded(theme.radius)
                            .bg(color.opacity(0.14))
                            .text_color(color)
                            .text_xs()
                            .child(dot(color))
                            .child(status_label(&task.status).to_owned())
                            .when(!task.is_open, |this| this.child("· closed")),
                    )
                    .into_any_element()
            }
            "priority" => h_flex()
                .gap_1p5()
                .child(dot(priority_color(&task.priority, &theme)))
                .child(priority_label(&task.priority).to_owned())
                .into_any_element(),
            "owner" => person(&task.owner),
            "action_required" => person(&task.action_required),
            "due_on" => match task.due_on.as_deref() {
                Some(due) => {
                    let today = chrono::Local::now()
                        .date_naive()
                        .format("%Y-%m-%d")
                        .to_string();
                    let color = match due {
                        d if task.is_open && d < today.as_str() => theme.red,
                        d if task.is_open && d == today => theme.yellow,
                        _ => theme.foreground,
                    };
                    div()
                        .text_color(color)
                        .child(human_day(due))
                        .into_any_element()
                }
                None => muted("—".into()),
            },
            "created_at" => muted(task.created_at.map(stamp).unwrap_or_default()),
            "updated_at" => muted(task.updated_at.map(ago_short).unwrap_or_default()),
            _ => div().into_any_element(),
        }
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(task) = self.task_at(row_ix) else {
            return menu;
        };
        let id = task.id.clone();
        let url = format!("{}{}", self.web_base, task.id);
        let table = self.table.clone();
        let link = url.clone();
        menu.item(PopupMenuItem::new("Open").on_click(move |_, _, cx| {
            let id = id.clone();
            table
                .update(cx, |_, cx| {
                    cx.emit(Open::Task {
                        id,
                        org_id: None,
                        preview: false,
                    })
                })
                .ok();
        }))
        .item(PopupMenuItem::new("Open in browser").on_click(move |_, _, cx| cx.open_url(&url)))
        .item(PopupMenuItem::new("Copy link").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
        }))
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(if self.tasks.is_empty() {
                "No tasks match these filters. There may be others you can't see."
            } else {
                "Nothing here matches the text filter."
            })
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    /// Not while the text filter is on: scrolling to the end of a few
    /// matches would page in the whole organisation.
    fn has_more(&self, _: &App) -> bool {
        self.has_more && self.rows.len() == self.tasks.len()
    }

    fn load_more_threshold(&self) -> usize {
        30
    }

    fn load_more(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.has_more = false;
        self.defer(cx, |table, cx| table.load_more(cx));
    }
}
