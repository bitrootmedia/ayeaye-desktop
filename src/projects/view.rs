//! A project in a tab: its name, description and group, its open tasks, and
//! its conversation.
//!
//! What the web app keeps on the project's page as settings — who it's
//! shared with, exports, handing it over, deleting it — stays there, one
//! click away. Controls the caller's access level denies are hidden, not
//! shown to fail: writers edit the details, only the owner archives.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::api::models::Task as TaskData;
use crate::api::models::*;
use crate::api::{ApiClient, ApiError, ApiResult, TaskQuery};
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::tasks::table::TableFilters;
use crate::tasks::view::{card, render_message, section_title};
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;
use crate::ui::widgets::{dot, human_day, picker, priority_color};
use gpui_kit::Task;

/// Enough to see what's going on; the table has the rest.
const TASK_LIMIT: usize = 50;

/// Raised so the workspace can refresh lists that show this project.
pub enum ProjectViewEvent {
    Changed(Box<Project>),
    AuthFailed(ApiError),
}

impl EventEmitter<ItemEvent> for ProjectView {}
impl EventEmitter<ProjectViewEvent> for ProjectView {}
impl EventEmitter<Open> for ProjectView {}

pub struct ProjectView {
    ctx: OrgCtx,
    project_id: String,
    project: Option<Project>,
    groups: Vec<ProjectGroup>,
    tasks: Vec<TaskData>,
    task_total: usize,
    thread: Option<Thread>,
    load_error: Option<String>,
    error: Option<String>,
    busy: bool,
    name: Entity<InputState>,
    description: Option<Entity<TextareaState>>,
    comment: Entity<TextareaState>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    comments_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl ProjectView {
    pub fn new(
        ctx: OrgCtx,
        project_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
        let comment = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write a comment…")
                .auto_grow(2, 10)
        });
        let subscriptions = vec![
            cx.subscribe_in(&name, window, Self::on_name_event),
            cx.subscribe_in(&comment, window, Self::on_comment_event),
        ];
        let mut view = Self {
            ctx,
            project_id,
            project: None,
            groups: Vec::new(),
            tasks: Vec::new(),
            task_total: 0,
            thread: None,
            load_error: None,
            error: None,
            busy: false,
            name,
            description: None,
            comment,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            comments_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        view.reload(window, cx);
        view
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let project_id = self.project_id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let project = client.project(&org_id, &project_id)?;
                    // The rest is best-effort: a project without its tasks
                    // or comments is still worth showing.
                    let groups = client.project_groups(&org_id).unwrap_or_default();
                    let query = TaskQuery {
                        project_id: Some(project_id.clone()),
                        limit: TASK_LIMIT,
                        ..Default::default()
                    };
                    let tasks = client.tasks_counted(&org_id, &query).ok();
                    let thread = client.project_comments(&org_id, &project_id).ok();
                    Ok::<_, ApiError>((project, groups, tasks, thread))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((project, groups, tasks, thread)) => {
                        this.load_error = None;
                        this.set_project(project, window, cx);
                        this.groups = groups;
                        if let Some((mut tasks, total)) = tasks {
                            tasks.sort_by(|a, b| {
                                priority_rank(&a.priority)
                                    .cmp(&priority_rank(&b.priority))
                                    .then(b.updated_at.cmp(&a.updated_at))
                            });
                            this.tasks = tasks;
                            this.task_total = total;
                        }
                        this.set_thread(thread);
                    }
                    Err(e) if e.is_auth() => cx.emit(ProjectViewEvent::AuthFailed(e)),
                    // No access reads as 404: gone and never shared look alike.
                    Err(ApiError::NotFound(_)) => {
                        this.load_error = Some(
                            "You don't have access to this project. It may have been deleted, \
                             or never shared with you."
                                .into(),
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

    /// Just the conversation, when a comment arrives live.
    pub fn reload_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            |c, org, id| c.project_comments(org, id),
            |this, thread, _, _| this.set_thread(Some(thread)),
        );
    }

    fn set_project(&mut self, project: Project, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.name.read(cx).focus_handle(cx).is_focused(window);
        if !focused && self.name.read(cx).value().as_ref() != project.name {
            let name = project.name.clone();
            self.name.update(cx, |n, cx| n.set_value(name, window, cx));
        }
        let editable = project.can_edit();
        self.name.update(cx, |n, cx| n.set_readonly(!editable, cx));
        self.project = Some(project);
    }

    /// The newest comment is at the bottom: keep it in view when the
    /// thread grows, as a chat would.
    fn set_thread(&mut self, thread: Option<Thread>) {
        let count = |t: &Option<Thread>| t.as_ref().map_or(0, |t| t.messages.len());
        if count(&thread) != count(&self.thread) {
            self.comments_scroll.scroll_to_bottom();
        }
        self.thread = thread;
    }

    /// Runs `call` in the background, then hands its result to `done` on the
    /// main thread; errors land in the view's error line.
    fn request<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        call: impl FnOnce(&ApiClient, &str, &str) -> ApiResult<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let project_id = self.project_id.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { call(&client, &org_id, &project_id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(e) if e.is_auth() => cx.emit(ProjectViewEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn update_fields(&mut self, fields: Value, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c, org, id| c.update_project(org, id, fields),
            |this, project, window, cx| {
                cx.emit(ProjectViewEvent::Changed(Box::new(project.clone())));
                this.set_project(project, window, cx);
            },
        );
    }

    /// The table, narrowed to this project — every task, every field.
    fn open_table(&self, cx: &mut Context<Self>) {
        cx.emit(Open::TaskTable {
            filters: Some(TableFilters {
                project: Some(self.project_id.clone()),
                ..Default::default()
            }),
        });
    }

    // ------------------------------------------------------------- inputs

    fn on_name_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
            return;
        }
        let value = input.read(cx).value().trim().to_owned();
        let Some(project) = &self.project else { return };
        if value.is_empty() || value == project.name {
            return;
        }
        cx.emit(ItemEvent::Edited);
        self.update_fields(json!({ "name": value }), window, cx);
    }

    fn start_editing_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .project
            .as_ref()
            .and_then(|p| p.description.clone())
            .unwrap_or_default();
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What this project is for")
                .auto_grow(4, 30)
                .default_value(text)
        });
        editor.update(cx, |e, cx| e.focus(window, cx));
        self.description = Some(editor);
        cx.notify();
    }

    fn save_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.description.take() else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        cx.emit(ItemEvent::Edited);
        self.update_fields(json!({ "description": text }), window, cx);
    }

    fn on_comment_event(
        &mut self,
        input: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // ⌘⏎ sends. Return alone is a new line: comments run long.
        let InputEvent::PressEnter {
            secondary: true, ..
        } = event
        else {
            return;
        };
        let body = input.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        self.send_comment(body, window, cx);
    }

    /// The composer clears once the server has the comment, so a failure
    /// leaves the text where it was.
    fn send_comment(&mut self, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let sent = body.clone();
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.post_project_comment(org, id, &body)?;
                c.project_comments(org, id)
            },
            move |this, thread, window, cx| {
                this.comment.update(cx, |c, cx| {
                    if c.value().trim() == sent {
                        c.set_value("", window, cx)
                    }
                });
                this.set_thread(Some(thread));
            },
        );
    }

    // ------------------------------------------------------------- render

    fn render_header(&self, project: &Project, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let web = self.ctx.session.web_url(&format!(
            "/orgs/{}/projects/{}",
            self.ctx.org_id(),
            project.id
        ));
        let badge = |label: String| {
            div()
                .px_1p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label)
        };
        let groups: Vec<(String, String)> = std::iter::once((String::new(), "No group".to_owned()))
            .chain(self.groups.iter().map(|g| (g.id.clone(), g.name.clone())))
            .collect();
        let group_label = project
            .project_group_name
            .clone()
            .unwrap_or_else(|| "No group".into());
        let this = cx.entity().downgrade();

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Icon::new(Lucide::FolderKanban)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(badge(project.access_label().into()))
                    .when(project.archived, |this| {
                        this.child(badge("Archived".into()))
                    })
                    // Groups are labels for tidiness: moving one grants
                    // nobody anything, so any editor may.
                    .map(|row| {
                        if project.can_edit() && !self.groups.is_empty() {
                            row.child(picker(
                                "project-group",
                                group_label,
                                groups,
                                Some(project.project_group_id.clone().unwrap_or_default()),
                                move |id, window, cx| {
                                    let id = if id.is_empty() {
                                        Value::Null
                                    } else {
                                        Value::String(id)
                                    };
                                    this.update(cx, |this, cx| {
                                        this.update_fields(
                                            json!({ "project_group_id": id }),
                                            window,
                                            cx,
                                        )
                                    })
                                    .ok();
                                },
                            ))
                        } else {
                            row.when_some(project.project_group_name.clone(), |row, name| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("in {name}")),
                                )
                            })
                        }
                    })
                    .when_some(project.owner.as_ref().map(Person::name), |this, owner| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("owned by {owner}")),
                        )
                    }),
            )
            .child(
                Input::new(&self.name)
                    .appearance(false)
                    .px_0()
                    .h(px(42.))
                    .text_size(px(22.))
                    .line_height(px(32.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Button::new("project-table")
                            .icon(Lucide::Table)
                            .label("All its tasks")
                            .small()
                            .tooltip("Open the task table on this project")
                            .on_click(cx.listener(|this, _, _, cx| this.open_table(cx))),
                    )
                    .when(project.is_owner(), |this| {
                        let archived = project.archived;
                        this.child(
                            Button::new("project-archive")
                                .icon(if archived {
                                    Lucide::ArchiveRestore
                                } else {
                                    Lucide::Archive
                                })
                                .label(if archived { "Restore" } else { "Archive" })
                                .ghost()
                                .small()
                                .tooltip(if archived {
                                    "Back on everyone's list"
                                } else {
                                    "Off everyone's list; nothing is deleted"
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.update_fields(json!({ "archived": !archived }), window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("project-web")
                            .icon(IconName::ExternalLink)
                            .label("Open in browser")
                            .ghost()
                            .small()
                            .tooltip("Sharing, export, handing over and deleting")
                            .on_click(move |_, _, cx| cx.open_url(&web)),
                    )
                    .when(self.busy, |this| this.child(Spinner::new().small())),
            )
    }

    fn render_description(
        &self,
        project: &Project,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let editable = project.can_edit();
        let body = project.description.clone().unwrap_or_default();
        card(&theme)
            .p_5()
            .border_color(theme.blue.opacity(0.35))
            .child(
                section_title("About", Lucide::TextAlignStart, theme.blue, &theme).when(
                    editable && self.description.is_none(),
                    |this| {
                        this.child(
                            Button::new("edit-desc")
                                .label("Edit")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_editing_description(window, cx)
                                })),
                        )
                    },
                ),
            )
            .child(match &self.description {
                Some(editor) => v_flex()
                    .gap_2()
                    .child(Textarea::new(editor))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("desc-cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.description = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("desc-save")
                                    .label("Save")
                                    .primary()
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_description(window, cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
                None if body.trim().is_empty() => div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No description.")
                    .into_any_element(),
                // Plain text on the server, as the web app writes it.
                None => div()
                    .max_w(px(720.))
                    .text_size(px(15.))
                    .line_height(px(24.))
                    .whitespace_normal()
                    .children(
                        body.lines()
                            .map(|line| div().min_h(px(24.)).child(line.to_owned())),
                    )
                    .into_any_element(),
            })
    }

    /// Critical, urgent or high among the open tasks. The server counts
    /// these only in the project list, but the tasks arrive most important
    /// first, so the loaded ones tell — short of a page that's all of them.
    fn important(&self) -> String {
        let n = self
            .tasks
            .iter()
            .filter(|t| priority_rank(&t.priority) < 3)
            .count();
        let partial = n == self.tasks.len() && self.task_total > n;
        format!("{n}{}", if partial { "+" } else { "" })
    }

    fn render_tasks(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let more = self.task_total.saturating_sub(self.tasks.len());
        let important = self.important();
        card(&theme)
            .child(
                section_title("Open tasks", Lucide::SquareCheck, theme.green, &theme)
                    .child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .child(format!("{}", self.task_total)),
                    )
                    .when(important != "0", |this| {
                        this.child(
                            h_flex()
                                .gap_0p5()
                                .font_weight(FontWeight::NORMAL)
                                .child(Icon::new(Lucide::CircleAlert).xsmall())
                                .child(format!("{important} important")),
                        )
                    }),
            )
            .when(self.tasks.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No open tasks."),
                )
            })
            .child(v_flex().children(self.tasks.iter().map(|task| {
                let id = task.id.clone();
                let meta = task
                    .due_on
                    .as_deref()
                    .map(human_day)
                    .or_else(|| task.owner.as_ref().map(Person::name));
                h_flex()
                    .id(SharedString::from(format!("project-task-{id}")))
                    .h(px(28.))
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
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(80.))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(status_label(&task.status).to_owned()),
                    )
                    .when_some(meta, |this, meta| {
                        this.child(
                            div()
                                .flex_none()
                                .w(px(110.))
                                .truncate()
                                .text_right()
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
            })))
            .when(more > 0, |this| {
                this.child(
                    Button::new("project-more")
                        .label(format!("{more} more in the table"))
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.open_table(cx))),
                )
            })
    }

    /// The conversation as a pane of its own, as on a task.
    fn render_comments(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let messages: Vec<Message> = self
            .thread
            .iter()
            .flat_map(|t| t.messages.iter().filter(|m| !m.deleted).cloned())
            .collect();
        let can_post = self.thread.as_ref().is_some_and(|t| t.can_post);

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_4()
                    .h(px(44.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Icon::new(Lucide::MessageSquare)
                            .small()
                            .text_color(theme.blue),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Comments"),
                    )
                    .when(!messages.is_empty(), |this| {
                        this.child(
                            div()
                                .px_1p5()
                                .rounded_full()
                                .bg(theme.blue.opacity(0.15))
                                .text_xs()
                                .text_color(theme.blue)
                                .child(messages.len().to_string()),
                        )
                    }),
            )
            .child(
                div()
                    .id("project-comments")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.comments_scroll)
                    .child(
                        v_flex()
                            .gap_4()
                            .p_4()
                            .when(self.thread.is_none(), |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child("Comments couldn't be loaded."),
                                )
                            })
                            .when(self.thread.is_some() && messages.is_empty(), |this| {
                                this.child(
                                    v_flex()
                                        .items_center()
                                        .gap_2()
                                        .py_8()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            Icon::new(Lucide::MessageSquare)
                                                .large()
                                                .text_color(theme.muted_foreground.opacity(0.5)),
                                        )
                                        .child("No comments yet — start the conversation."),
                                )
                            })
                            .children(messages.iter().map(|m| render_message(m, &theme))),
                    ),
            )
            .when(can_post, |this| {
                this.child(
                    v_flex()
                        .flex_none()
                        .gap_2()
                        .p_3()
                        .border_t_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .child(Textarea::new(&self.comment))
                        .child(
                            h_flex()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("⌘⏎ to send"),
                                )
                                .child(
                                    Button::new("send-comment")
                                        .icon(IconName::ArrowUp)
                                        .label("Comment")
                                        .primary()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let body =
                                                this.comment.read(cx).value().trim().to_owned();
                                            if !body.is_empty() {
                                                this.send_comment(body, window, cx);
                                            }
                                        })),
                                ),
                        ),
                )
            })
    }
}

impl Focusable for ProjectView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ProjectView {
    fn item_key(&self, _: &App) -> SharedString {
        format!("project:{}", self.project_id).into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        self.project
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Project".into())
            .into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::FolderKanban)
    }

    fn is_dirty(&self, _: &App) -> bool {
        self.description.is_some()
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let project = self.project.as_ref()?;
        Some(
            format!(
                "{} · {} open{}",
                project.access_label(),
                self.task_total,
                if project.archived { " · archived" } else { "" }
            )
            .into(),
        )
    }
}

impl Render for ProjectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Side by side when there's room; on a narrow window the comments
        // follow the project in one scroll.
        let split = window.viewport_size().width >= px(980.);
        let content = match (&self.project, &self.load_error) {
            (Some(project), _) => {
                let project = project.clone();
                v_flex()
                    .gap_4()
                    .child(self.render_header(&project, cx))
                    .when_some(self.error.clone(), |this, e| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded(theme.radius)
                                .bg(theme.danger.opacity(0.12))
                                .text_sm()
                                .child(e),
                        )
                    })
                    .child(self.render_description(&project, cx))
                    .child(self.render_tasks(cx))
                    .when(!split, |this| {
                        this.child(
                            div()
                                .h(px(560.))
                                .rounded(theme.radius_lg)
                                .border_1()
                                .border_color(theme.border)
                                .overflow_hidden()
                                .child(self.render_comments(cx)),
                        )
                    })
                    .into_any_element()
            }
            (None, Some(e)) => v_flex()
                .gap_2()
                .child(div().text_lg().child("Couldn't open this project"))
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
        let loaded = self.project.is_some();

        h_flex()
            .id("project-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .items_start()
            .bg(theme.background)
            .child(
                div()
                    .id("project-main")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().max_w(px(860.)).px_6().py_5().child(content)),
            )
            .when(split && loaded, |this| {
                this.child(
                    div()
                        .flex_none()
                        .w(px(400.))
                        .h_full()
                        .border_l_1()
                        .border_color(theme.border)
                        .child(self.render_comments(cx)),
                )
            })
    }
}
