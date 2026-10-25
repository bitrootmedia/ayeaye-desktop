//! A task in a tab: its fields, description, checklists, reminders and
//! comments, and the timer.
//!
//! Every change is one request and the server's answer replaces what's on
//! screen — a task is shared work, so nothing here is optimistic about
//! another person's edits. Controls a `can_*` flag or the access level denies
//! are hidden, not shown to fail.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::api::models::Task as TaskData;
use crate::api::models::*;
use crate::api::{ApiClient, ApiError, ApiResult};
use crate::session::OrgCtx;
use crate::timer::TimerState;
use crate::ui::format::ago_long;
use crate::ui::item::{Item, ItemEvent};
use crate::ui::widgets::{
    dot, field_row, html_to_markdown, human_day, parse_day, picker, priority_color, quick_dates,
    rich_text,
};
use gpui_kit::Task;

type PickHandler = Box<dyn Fn(String, &mut Window, &mut App)>;

/// Raised so the workspace can refresh lists that show this task.
pub enum TaskViewEvent {
    Changed(Box<TaskData>),
    AuthFailed(ApiError),
}

impl EventEmitter<ItemEvent> for TaskView {}
impl EventEmitter<TaskViewEvent> for TaskView {}

pub struct TaskView {
    ctx: OrgCtx,
    task_id: String,
    timer: Entity<TimerState>,
    task: Option<TaskData>,
    checklists: Vec<Checklist>,
    thread: Option<Thread>,
    reminders: Vec<Reminder>,
    load_error: Option<String>,
    error: Option<String>,
    busy: bool,
    title: Entity<InputState>,
    description: Option<Entity<TextareaState>>,
    item_inputs: HashMap<String, Entity<InputState>>,
    new_checklist: Entity<InputState>,
    comment: Entity<TextareaState>,
    reminder_date: Entity<InputState>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl TaskView {
    pub fn new(
        ctx: OrgCtx,
        task_id: String,
        timer: Entity<TimerState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Task title"));
        let new_checklist = cx.new(|cx| InputState::new(window, cx).placeholder("New checklist…"));
        let comment = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write a comment — ⌘⏎ to send")
                .auto_grow(2, 10)
        });
        let reminder_date =
            cx.new(|cx| InputState::new(window, cx).placeholder("Date: 2026-11-03, 3.11, +7"));
        let subscriptions = vec![
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&new_checklist, window, Self::on_new_checklist_event),
            cx.subscribe_in(&comment, window, Self::on_comment_event),
            cx.subscribe_in(&reminder_date, window, Self::on_reminder_date_event),
            cx.observe(&timer, |_, _, cx| cx.notify()),
            cx.observe(&ctx.directory, |_, _, cx| cx.notify()),
        ];
        ctx.directory.update(cx, |d, cx| d.ensure_loaded(cx));
        let mut view = Self {
            ctx,
            task_id,
            timer,
            task: None,
            checklists: Vec::new(),
            thread: None,
            reminders: Vec::new(),
            load_error: None,
            error: None,
            busy: false,
            title,
            description: None,
            item_inputs: HashMap::new(),
            new_checklist,
            comment,
            reminder_date,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        view.reload(window, cx);
        view
    }

    fn org_id(&self) -> String {
        self.ctx.org_id().to_owned()
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.org_id();
        let task_id = self.task_id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let task = client.task(&org_id, &task_id)?;
                    // The rest is best-effort: a task without its comments
                    // is still worth showing.
                    let checklists = client.checklists(&org_id, &task_id).unwrap_or_default();
                    let thread = client.comments(&org_id, &task_id).ok();
                    let reminders = client.task_reminders(&org_id, &task_id).unwrap_or_default();
                    Ok::<_, ApiError>((task, checklists, thread, reminders))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((task, checklists, thread, reminders)) => {
                        this.load_error = None;
                        this.set_task(task, window, cx);
                        this.set_checklists(checklists, window, cx);
                        this.thread = thread;
                        this.reminders = reminders;
                    }
                    Err(e) if e.is_auth() => cx.emit(TaskViewEvent::AuthFailed(e)),
                    Err(e) => this.load_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn set_task(&mut self, task: TaskData, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.title.read(cx).focus_handle(cx).is_focused(window);
        if !focused && self.title.read(cx).value().as_ref() != task.title {
            let title = task.title.clone();
            self.title
                .update(cx, |t, cx| t.set_value(title, window, cx));
        }
        let editable = task.can_edit();
        self.title.update(cx, |t, cx| t.set_readonly(!editable, cx));
        self.task = Some(task);
    }

    fn set_checklists(
        &mut self,
        checklists: Vec<Checklist>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for list in &checklists {
            if !self.item_inputs.contains_key(&list.id) {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Add an item…"));
                let list_id = list.id.clone();
                self._subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            let text = input.read(cx).value().trim().to_owned();
                            if !text.is_empty() {
                                input.update(cx, |i, cx| i.set_value("", window, cx));
                                this.add_item(list_id.clone(), text, window, cx);
                            }
                        }
                    },
                ));
                self.item_inputs.insert(list.id.clone(), input);
            }
        }
        self.checklists = checklists;
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
        let org_id = self.org_id();
        let task_id = self.task_id.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { call(&client, &org_id, &task_id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(e) if e.is_auth() => cx.emit(TaskViewEvent::AuthFailed(e)),
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
            move |c, org, id| c.update_task(org, id, fields),
            |this, task, window, cx| {
                cx.emit(TaskViewEvent::Changed(Box::new(task.clone())));
                this.set_task(task, window, cx);
            },
        );
    }

    fn set_closed(&mut self, closed: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.set_task_closed(org, id, closed)?;
                c.task(org, id)
            },
            |this, task, window, cx| {
                cx.emit(TaskViewEvent::Changed(Box::new(task.clone())));
                this.set_task(task, window, cx);
            },
        );
    }

    fn set_pinned(&mut self, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.set_task_pinned(org, id, pinned)?;
                c.task(org, id)
            },
            |this, task, window, cx| this.set_task(task, window, cx),
        );
    }

    fn place_in_planner(
        &mut self,
        bucket: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                match &bucket {
                    Some(b) => c.place_in_planner(org, id, b)?,
                    None => c.remove_from_planner(org, id)?,
                }
                c.task(org, id)
            },
            |this, task, window, cx| {
                cx.emit(TaskViewEvent::Changed(Box::new(task.clone())));
                this.set_task(task, window, cx);
            },
        );
    }

    // ------------------------------------------------------------- inputs

    fn on_title_event(
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
        let Some(task) = &self.task else { return };
        if value.is_empty() || value == task.title {
            return;
        }
        cx.emit(ItemEvent::Edited);
        self.update_fields(json!({ "title": value }), window, cx);
    }

    fn on_new_checklist_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InputEvent::PressEnter { .. } = event else {
            return;
        };
        let title = input.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return;
        }
        input.update(cx, |i, cx| i.set_value("", window, cx));
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.add_checklist(org, id, &title)?;
                c.checklists(org, id)
            },
            |this, lists, window, cx| this.set_checklists(lists, window, cx),
        );
    }

    fn add_item(
        &mut self,
        checklist_id: String,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.add_checklist_item(org, id, &checklist_id, &text)?;
                c.checklists(org, id)
            },
            |this, lists, window, cx| this.set_checklists(lists, window, cx),
        );
    }

    fn toggle_item(
        &mut self,
        checklist_id: String,
        item_id: String,
        done: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Tick it at once; the server's answer confirms or reverts it.
        if let Some(item) = self
            .checklists
            .iter_mut()
            .find(|l| l.id == checklist_id)
            .and_then(|l| l.items.iter_mut().find(|i| i.id == item_id))
        {
            item.done = done;
        }
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.set_checklist_item_done(org, id, &checklist_id, &item_id, done)?;
                c.checklists(org, id)
            },
            |this, lists, window, cx| this.set_checklists(lists, window, cx),
        );
    }

    fn delete_item(
        &mut self,
        checklist_id: String,
        item_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.delete_checklist_item(org, id, &checklist_id, &item_id)?;
                c.checklists(org, id)
            },
            |this, lists, window, cx| this.set_checklists(lists, window, cx),
        );
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
        input.update(cx, |i, cx| i.set_value("", window, cx));
        self.send_comment(body, window, cx);
    }

    fn send_comment(&mut self, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let restore = body.clone();
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.post_comment(org, id, &body)?;
                c.comments(org, id)
            },
            |this, thread, _, cx| {
                this.thread = Some(thread);
                cx.notify();
            },
        );
        // If it fails, the text is still the only copy: put it back.
        let comment = self.comment.clone();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;
            loop {
                let state = this
                    .update(cx, |this, _| (this.busy, this.error.clone()))
                    .ok();
                match state {
                    Some((true, _)) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(100))
                            .await
                    }
                    Some((false, Some(_))) => {
                        comment
                            .update_in(cx, |c, window, cx| {
                                if c.value().is_empty() {
                                    c.set_value(restore.clone(), window, cx)
                                }
                            })
                            .ok();
                        break;
                    }
                    _ => break,
                }
            }
        }));
    }

    fn on_reminder_date_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InputEvent::PressEnter { .. } = event else {
            return;
        };
        let raw = input.read(cx).value().to_string();
        match parse_day(&raw) {
            Some(day) => {
                input.update(cx, |i, cx| i.set_value("", window, cx));
                self.add_reminder(day, window, cx);
            }
            None => {
                self.error = Some(format!(
                    "“{raw}” isn't a date I can read — try 2026-11-03 or +7"
                ));
                cx.notify();
            }
        }
    }

    fn add_reminder(&mut self, day: String, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.add_task_reminder(org, id, &day, None)?;
                c.task_reminders(org, id)
            },
            |this, reminders, _, _| this.reminders = reminders,
        );
    }

    fn delete_reminder(
        &mut self,
        reminder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request(
            window,
            cx,
            move |c, org, id| {
                c.delete_reminder(&reminder_id)?;
                c.task_reminders(org, id)
            },
            |this, reminders, _, _| this.reminders = reminders,
        );
    }

    fn start_editing_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let markdown = self
            .task
            .as_ref()
            .and_then(|t| t.description.as_deref())
            .map(html_to_markdown)
            .unwrap_or_default();
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe the task — markdown works")
                .auto_grow(6, 40)
                .default_value(markdown)
        });
        editor.update(cx, |e, cx| e.focus(window, cx));
        self.description = Some(editor);
        cx.notify();
    }

    fn save_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.description.take() else {
            return;
        };
        let markdown = editor.read(cx).value().to_string();
        cx.emit(ItemEvent::Edited);
        self.update_fields(json!({ "description": markdown }), window, cx);
    }

    // ------------------------------------------------------------- render

    fn render_header(&self, task: &TaskData, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let timer = self.timer.read(cx);
        let running_here = timer.running_task() == Some(task.id.as_str());
        let elapsed = timer.elapsed();
        let org_id = self.org_id();
        let task_id = task.id.clone();
        let web = self
            .ctx
            .session
            .web_url(&format!("/orgs/{}/tasks/{}", org_id, task.id));

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(dot(priority_color(&task.priority, &theme)))
                    .when(!task.is_open, |this| {
                        this.child(
                            div()
                                .px_1p5()
                                .rounded(theme.radius)
                                .bg(theme.muted)
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Closed"),
                        )
                    })
                    .when_some(task.project_name.clone(), |this, name| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(name),
                        )
                    }),
            )
            .child(
                Input::new(&self.title)
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
                        Button::new("timer")
                            .icon(if running_here {
                                IconName::Pause
                            } else {
                                IconName::Play
                            })
                            .label(if running_here {
                                format!("Stop · {}", crate::ui::widgets::clock(elapsed))
                            } else {
                                "Start timer".to_owned()
                            })
                            .small()
                            .when(running_here, |b| b.primary())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let org_id = org_id.clone();
                                let task_id = task_id.clone();
                                this.timer.update(cx, |t, cx| {
                                    if running_here {
                                        t.stop(cx)
                                    } else {
                                        t.start(org_id, task_id, cx)
                                    }
                                })
                            })),
                    )
                    .when(task.can_close, |this| {
                        let open = task.is_open;
                        this.child(
                            Button::new("close")
                                .icon(if open {
                                    IconName::Check
                                } else {
                                    IconName::Undo2
                                })
                                .label(if open { "Close" } else { "Reopen" })
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_closed(open, window, cx)
                                })),
                        )
                    })
                    .child({
                        let pinned = task.is_pinned;
                        Button::new("pin")
                            .icon(if pinned {
                                IconName::StarFill
                            } else {
                                IconName::Star
                            })
                            .label(if pinned { "Pinned" } else { "Pin" })
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_pinned(!pinned, window, cx)
                            }))
                    })
                    .child(
                        Button::new("web")
                            .icon(IconName::ExternalLink)
                            .label("Open in browser")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| cx.open_url(&web)),
                    )
                    .when(self.busy, |this| this.child(Spinner::new().small())),
            )
    }

    fn render_fields(&self, task: &TaskData, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editable = task.can_edit();
        let directory = self.ctx.directory.read(cx);
        let people: Vec<(String, String)> = std::iter::once((String::new(), "Nobody".to_owned()))
            .chain(
                directory
                    .members
                    .iter()
                    .filter_map(|m| m.user_id.clone().map(|id| (id, m.name()))),
            )
            .collect();
        let projects: Vec<(String, String)> =
            std::iter::once((String::new(), "No project".to_owned()))
                .chain(
                    directory
                        .projects
                        .iter()
                        .map(|p| (p.id.clone(), p.name.clone())),
                )
                .collect();
        let statuses: Vec<(String, String)> = STATUSES
            .iter()
            .map(|(k, l)| (k.to_string(), l.to_string()))
            .collect();
        let priorities: Vec<(String, String)> = PRIORITIES
            .iter()
            .map(|(k, l)| (k.to_string(), l.to_string()))
            .collect();
        let mut buckets: Vec<(String, String)> = BUCKETS
            .iter()
            .map(|(k, l)| (k.to_string(), l.to_string()))
            .collect();
        buckets.push((String::new(), "Not planned".into()));

        let this = cx.entity().downgrade();
        let set = move |field: &'static str| {
            let this = this.clone();
            move |value: String, window: &mut Window, cx: &mut App| {
                let value = if value.is_empty() {
                    Value::Null
                } else {
                    Value::String(value)
                };
                this.update(cx, |this, cx| {
                    this.update_fields(json!({ field: value }), window, cx)
                })
                .ok();
            }
        };
        let plan = {
            let this = cx.entity().downgrade();
            move |bucket: String, window: &mut Window, cx: &mut App| {
                let bucket = (!bucket.is_empty()).then_some(bucket);
                this.update(cx, |this, cx| this.place_in_planner(bucket, window, cx))
                    .ok();
            }
        };

        let person_label = |p: &Option<Person>| {
            p.as_ref()
                .map(Person::name)
                .unwrap_or_else(|| "Nobody".into())
        };
        let value = |id: &'static str,
                     label: String,
                     options: Vec<(String, String)>,
                     selected: Option<String>,
                     on: PickHandler|
         -> AnyElement {
            if editable {
                picker(id, label, options, selected, on).into_any_element()
            } else {
                div().text_sm().child(label).into_any_element()
            }
        };

        v_flex()
            .gap_0p5()
            .child(field_row(
                "Status",
                value(
                    "status",
                    status_label(&task.status).to_owned(),
                    statuses,
                    Some(task.status.clone()),
                    Box::new(set("status")),
                ),
                cx,
            ))
            .child(field_row(
                "Priority",
                value(
                    "priority",
                    priority_label(&task.priority).to_owned(),
                    priorities,
                    Some(task.priority.clone()),
                    Box::new(set("priority")),
                ),
                cx,
            ))
            .child(field_row(
                "Owner",
                value(
                    "owner",
                    person_label(&task.owner),
                    people.clone(),
                    task.owner.as_ref().map(|p| p.id.clone()),
                    Box::new(set("owner_user_id")),
                ),
                cx,
            ))
            .child(field_row(
                "Who acts next",
                value(
                    "action",
                    person_label(&task.action_required),
                    people,
                    task.action_required.as_ref().map(|p| p.id.clone()),
                    Box::new(set("action_required_user_id")),
                ),
                cx,
            ))
            .child(field_row(
                "Project",
                value(
                    "project",
                    task.project_name
                        .clone()
                        .unwrap_or_else(|| "No project".into()),
                    projects,
                    task.project_id.clone(),
                    Box::new(set("project_id")),
                ),
                cx,
            ))
            .child(field_row(
                "Due",
                value(
                    "due",
                    task.due_on
                        .as_deref()
                        .map(human_day)
                        .unwrap_or_else(|| "No due date".into()),
                    std::iter::once((String::new(), "No due date".to_owned()))
                        .chain(quick_dates().into_iter().map(|(l, d)| (d, l.to_owned())))
                        .collect(),
                    task.due_on.clone(),
                    Box::new(set("due_on")),
                ),
                cx,
            ))
            .child(field_row(
                "My planner",
                picker(
                    "planner",
                    task.planner_bucket
                        .as_deref()
                        .and_then(|b| BUCKETS.iter().find(|(k, _)| *k == b).map(|(_, l)| *l))
                        .unwrap_or("Not planned"),
                    buckets,
                    task.planner_bucket.clone(),
                    plan,
                ),
                cx,
            ))
    }

    fn render_description(
        &self,
        task: &TaskData,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let editable = task.can_edit();
        let body = task.description.clone().unwrap_or_default();
        v_flex()
            .gap_2()
            .child(section_title("Description", &theme).when(
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
            ))
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
                None => div()
                    .text_sm()
                    .child(rich_text("task-description", &body))
                    .into_any_element(),
            })
    }

    fn render_checklists(
        &self,
        task: &TaskData,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let editable = task.can_edit();
        v_flex()
            .gap_2()
            .when(!self.checklists.is_empty() || editable, |this| {
                this.child(section_title("Checklists", &theme))
            })
            .children(self.checklists.iter().map(|list| {
                let done = list.items.iter().filter(|i| i.done).count();
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(list.title.clone())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{done}/{}", list.items.len())),
                            ),
                    )
                    .children(list.items.iter().map(|item| {
                        let (list_id, item_id) = (list.id.clone(), item.id.clone());
                        let (list_id2, item_id2) = (list.id.clone(), item.id.clone());
                        let group: SharedString = format!("item-{}", item.id).into();
                        h_flex()
                            .group(group.clone())
                            .gap_2()
                            .child(
                                Checkbox::new(SharedString::from(format!("check-{}", item.id)))
                                    .checked(item.done)
                                    .label(item.text.clone())
                                    .on_click(cx.listener(
                                        move |this, checked: &bool, window, cx| {
                                            this.toggle_item(
                                                list_id.clone(),
                                                item_id.clone(),
                                                *checked,
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                            )
                            .when(editable, |this| {
                                this.child(
                                    div().invisible().group_hover(group, |s| s.visible()).child(
                                        Button::new(SharedString::from(format!("del-{}", item.id)))
                                            .icon(IconName::Close)
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.delete_item(
                                                    list_id2.clone(),
                                                    item_id2.clone(),
                                                    window,
                                                    cx,
                                                )
                                            })),
                                    ),
                                )
                            })
                    }))
                    .when_some(
                        self.item_inputs.get(&list.id).filter(|_| editable),
                        |this, input| {
                            this.child(div().w(px(320.)).child(Input::new(input).small()))
                        },
                    )
            }))
            .when(editable, |this| {
                this.child(
                    div()
                        .w(px(320.))
                        .child(Input::new(&self.new_checklist).small()),
                )
            })
    }

    fn render_reminders(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        v_flex()
            .gap_2()
            .child(section_title("Reminders", &theme))
            .children(self.reminders.iter().map(|r| {
                let id = r.id.clone();
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(Icon::new(IconName::Bell).small().text_color(if r.overdue {
                        theme.red
                    } else {
                        theme.muted_foreground
                    }))
                    .child(human_day(&r.remind_on))
                    .when_some(r.note.clone(), |this, note| {
                        this.child(div().text_color(theme.muted_foreground).child(note))
                    })
                    .child(
                        Button::new(SharedString::from(format!("rm-{}", r.id)))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove reminder")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_reminder(id.clone(), window, cx)
                            })),
                    )
            }))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(quick_dates().into_iter().map(|(label, day)| {
                        Button::new(SharedString::from(format!("remind-{label}")))
                            .label(label)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_reminder(day.clone(), window, cx)
                            }))
                    }))
                    .child(
                        div()
                            .w(px(220.))
                            .child(Input::new(&self.reminder_date).small()),
                    ),
            )
    }

    fn render_comments(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let Some(thread) = &self.thread else {
            return div().into_any_element();
        };
        v_flex()
            .gap_3()
            .child(section_title("Comments", &theme))
            .when(thread.messages.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No comments yet."),
                )
            })
            .children(thread.messages.iter().filter(|m| !m.deleted).map(|m| {
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .child(
                                div().font_weight(FontWeight::SEMIBOLD).child(
                                    m.author
                                        .as_ref()
                                        .map(Person::name)
                                        .unwrap_or_else(|| "ayeaye".into()),
                                ),
                            )
                            .when_some(m.created_at, |this, at| {
                                this.child(
                                    div().text_color(theme.muted_foreground).child(ago_long(at)),
                                )
                            })
                            .when(m.edited_at.is_some(), |this| {
                                this.child(div().text_color(theme.muted_foreground).child("edited"))
                            }),
                    )
                    .child(div().text_sm().child(rich_text(
                        SharedString::from(format!("msg-{}", m.id)),
                        &m.body,
                    )))
            }))
            .when(thread.can_post, |this| {
                this.child(
                    v_flex().gap_2().child(Textarea::new(&self.comment)).child(
                        h_flex().justify_end().child(
                            Button::new("send-comment")
                                .label("Comment")
                                .primary()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let body = this.comment.read(cx).value().trim().to_owned();
                                    if !body.is_empty() {
                                        this.comment
                                            .update(cx, |c, cx| c.set_value("", window, cx));
                                        this.send_comment(body, window, cx);
                                    }
                                })),
                        ),
                    ),
                )
            })
            .into_any_element()
    }
}

fn section_title(label: &'static str, theme: &gpui_kit::component::Theme) -> Div {
    h_flex()
        .gap_2()
        .pt_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground)
        .child(label.to_uppercase())
}

impl Focusable for TaskView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for TaskView {
    fn item_key(&self, _: &App) -> SharedString {
        format!("task:{}", self.task_id).into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        self.task
            .as_ref()
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "Task".into())
            .into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(crate::assets::Lucide::SquareCheck)
    }

    fn is_dirty(&self, _: &App) -> bool {
        self.description.is_some()
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let task = self.task.as_ref()?;
        Some(
            format!(
                "{} · {}{}",
                status_label(&task.status),
                priority_label(&task.priority),
                if task.is_open { "" } else { " · closed" }
            )
            .into(),
        )
    }
}

impl Render for TaskView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match (&self.task, &self.load_error) {
            (Some(task), _) => {
                let task = task.clone();
                v_flex()
                    .gap_5()
                    .child(self.render_header(&task, cx))
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
                    .child(self.render_fields(&task, cx))
                    .child(self.render_description(&task, cx))
                    .child(self.render_checklists(&task, cx))
                    .child(self.render_reminders(cx))
                    .child(self.render_comments(cx))
                    .into_any_element()
            }
            (None, Some(e)) => v_flex()
                .gap_2()
                .child(div().text_lg().child("Couldn't open this task"))
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
            .id("task-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(div().max_w(px(860.)).px_6().py_5().child(content))
    }
}
