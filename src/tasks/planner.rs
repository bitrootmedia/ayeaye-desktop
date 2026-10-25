//! My planner: one person's plan for their own week — Today, Tomorrow, This
//! week, Next week, Someday — in the order it was arranged on the web.
//! Moving a task between days is a right-click; arranging within a day
//! stays on the web's board, which has the room for dragging.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::ApiError;
use crate::api::models::{BUCKETS, Planner, PlannerTask};
use crate::session::OrgCtx;
use crate::ui::open::Open;
use crate::ui::widgets::{dot, priority_color, section_label};

pub enum PlannerEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for PlannerPanel {}
impl EventEmitter<PlannerEvent> for PlannerPanel {}

pub struct PlannerPanel {
    ctx: OrgCtx,
    planner: Planner,
    loading: bool,
    error: Option<String>,
    active_task: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl PlannerPanel {
    pub fn new(ctx: OrgCtx, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            ctx,
            planner: Planner::default(),
            loading: false,
            error: None,
            active_task: None,
            _tasks: Vec::new(),
        };
        panel.refresh(cx);
        panel
    }

    pub fn set_active_task(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_task != id {
            self.active_task = id;
            cx.notify();
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self.loading = true;
        cx.notify();
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.planner(&org_id) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(planner) => {
                        this.planner = planner;
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(PlannerEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn move_to(&mut self, task_id: String, bucket: Option<String>, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match &bucket {
                        Some(b) => client.place_in_planner(&org_id, &task_id, b)?,
                        None => client.remove_from_planner(&org_id, &task_id)?,
                    }
                    client.planner(&org_id)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(planner) => this.planner = planner,
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn row(&self, task: &PlannerTask, current: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = task.id.clone();
        let active = self.active_task.as_deref() == Some(id.as_str());
        let this = cx.entity().downgrade();
        let current = current.map(str::to_owned);
        h_flex()
            .id(SharedString::from(format!(
                "plan-{}-{}",
                current.as_deref().unwrap_or("pool"),
                task.id
            )))
            .w_full()
            .h(px(26.))
            .px_3()
            .gap_2()
            .text_sm()
            .cursor_pointer()
            .when(active, |s| s.bg(theme.list_active))
            .hover(|s| s.bg(theme.list_hover))
            .child(dot(priority_color(&task.priority, &theme)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(!task.is_open, |d| {
                        d.line_through().text_color(theme.muted_foreground)
                    })
                    .child(task.title.clone()),
            )
            .on_click(cx.listener({
                let id = id.clone();
                move |_, event: &ClickEvent, _, cx| {
                    cx.emit(Open::Task {
                        id: id.clone(),
                        org_id: None,
                        preview: event.click_count() < 2,
                    })
                }
            }))
            .context_menu(move |mut menu, _, _| {
                for (key, label) in BUCKETS {
                    if current.as_deref() == Some(*key) {
                        continue;
                    }
                    let this = this.clone();
                    let id = id.clone();
                    let key = key.to_string();
                    menu = menu.item(PopupMenuItem::new(format!("Move to {label}")).on_click(
                        move |_, _, cx| {
                            this.update(cx, |this, cx| {
                                this.move_to(id.clone(), Some(key.clone()), cx)
                            })
                            .ok();
                        },
                    ));
                }
                if current.is_some() {
                    let this = this.clone();
                    let id = id.clone();
                    menu =
                        menu.separator()
                            .item(PopupMenuItem::new("Take off my planner").on_click(
                                move |_, _, cx| {
                                    this.update(cx, |this, cx| this.move_to(id.clone(), None, cx))
                                        .ok();
                                },
                            ));
                }
                menu
            })
            .into_any_element()
    }
}

impl Render for PlannerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut sections: Vec<AnyElement> = Vec::new();
        for (key, label) in BUCKETS {
            let mut entries = self.planner.buckets.get(*key).cloned().unwrap_or_default();
            entries.sort_by_key(|e| e.position);
            sections.push(
                section_label(
                    format!(
                        "{} {}",
                        label.to_uppercase(),
                        if entries.is_empty() {
                            String::new()
                        } else {
                            entries.len().to_string()
                        }
                    ),
                    cx,
                )
                .into_any_element(),
            );
            if entries.is_empty() {
                sections.push(
                    div()
                        .px_3()
                        .h(px(22.))
                        .text_xs()
                        .text_color(theme.muted_foreground.opacity(0.6))
                        .child("—")
                        .into_any_element(),
                );
            }
            for entry in &entries {
                sections.push(self.row(&entry.task, Some(key), cx));
            }
        }
        let pool: Vec<PlannerTask> = self.planner.pool.clone();
        if !pool.is_empty() {
            sections.push(
                section_label("NOT PLANNED YET — RIGHT-CLICK TO PLAN", cx).into_any_element(),
            );
            for task in &pool {
                sections.push(self.row(task, None, cx));
            }
        }

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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("MY PLANNER"))
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("planner-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
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
                    .id("planner-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .pb_4()
                    .children(sections),
            )
    }
}
