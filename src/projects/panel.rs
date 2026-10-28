//! The projects in the sidebar: everything this person can see, under the
//! group each is filed in, filtered by typing.
//!
//! "Can see" is the server's call — a project is private to its owner until
//! it's shared — so the list is different for everyone, and the empty state
//! says so.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::ApiError;
use crate::api::models::Project;
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::open::Open;

pub enum ProjectsPanelEvent {
    NewProject,
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for ProjectsPanel {}
impl EventEmitter<ProjectsPanelEvent> for ProjectsPanel {}

pub struct ProjectsPanel {
    ctx: OrgCtx,
    projects: Vec<Project>,
    show_archived: bool,
    loading: bool,
    error: Option<String>,
    active_project: Option<String>,
    filter: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    _load: Option<Task<()>>,
}

impl ProjectsPanel {
    pub fn new(ctx: OrgCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter projects…")
                .clean_on_escape()
        });
        let subscriptions = vec![cx.subscribe_in(
            &filter,
            window,
            |this: &mut Self, _, e: &InputEvent, _, cx| match e {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => {
                    if let Some(project) = this.matches(cx).first() {
                        cx.emit(Open::Project {
                            id: project.id.clone(),
                            preview: false,
                        });
                    }
                }
                _ => {}
            },
        )];
        let mut panel = Self {
            ctx,
            projects: Vec::new(),
            show_archived: false,
            loading: false,
            error: None,
            active_project: None,
            filter,
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

    pub fn set_active_project(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_project != id {
            self.active_project = id;
            cx.notify();
        }
    }

    /// A project changed in its tab; reflect it here without a refetch.
    /// Task counts come only with the list, so the row keeps its own.
    pub fn project_changed(&mut self, project: &Project, cx: &mut Context<Self>) {
        if let Some(row) = self.projects.iter_mut().find(|p| p.id == project.id) {
            *row = Project {
                open_task_count: row.open_task_count,
                important_task_count: row.important_task_count,
                ..project.clone()
            };
            if project.archived && !self.show_archived {
                self.projects.retain(|p| p.id != project.id);
            }
            cx.notify();
        } else {
            self.refresh(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let archived = self.show_archived;
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.projects(&org_id, archived) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(projects) => {
                        this.projects = projects;
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(ProjectsPanelEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// What the filter leaves, by group (A–Z, ungrouped last), then by name.
    fn matches(&self, cx: &App) -> Vec<&Project> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        let mut matches: Vec<&Project> = self
            .projects
            .iter()
            .filter(|p| {
                let hay = format!(
                    "{} {}",
                    p.name.to_lowercase(),
                    p.project_group_name.as_deref().unwrap_or("").to_lowercase()
                );
                terms.iter().all(|term| hay.contains(term))
            })
            .collect();
        matches.sort_by_key(|p| {
            (
                p.project_group_name.is_none(),
                p.project_group_name.as_deref().map(str::to_lowercase),
                p.name.to_lowercase(),
            )
        });
        matches
    }
}

impl Render for ProjectsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let matches: Vec<Project> = self.matches(cx).into_iter().cloned().collect();
        let grouped = matches.iter().any(|p| p.project_group_name.is_some());
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut group: Option<Option<String>> = None;
        for project in &matches {
            if grouped && group.as_ref() != Some(&project.project_group_name) {
                group = Some(project.project_group_name.clone());
                rows.push(
                    h_flex()
                        .h(px(26.))
                        .px_3()
                        .gap_1p5()
                        .mt_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Lucide::Folder).xsmall())
                        .child(
                            project
                                .project_group_name
                                .clone()
                                .unwrap_or_else(|| "Ungrouped".into()),
                        )
                        .into_any_element(),
                );
            }
            let id = project.id.clone();
            let active = self.active_project.as_deref() == Some(id.as_str());
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("project-row-{id}")))
                    .h(px(26.))
                    .px_3()
                    .gap_2()
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |this| this.bg(theme.list_active))
                    .hover(|this| this.bg(theme.list_hover))
                    .child(
                        Icon::new(Lucide::FolderKanban)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(project.archived, |this| {
                                this.text_color(theme.muted_foreground)
                            })
                            .child(project.name.clone()),
                    )
                    .when(project.archived, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("archived"),
                        )
                    })
                    .when(project.important_task_count > 0, |this| {
                        this.child(
                            h_flex()
                                .id(SharedString::from(format!("project-important-{id}")))
                                .gap_0p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(Icon::new(Lucide::CircleAlert).xsmall())
                                .child(project.important_task_count.to_string())
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Open and critical, urgent or high",
                                    )
                                    .build(window, cx)
                                }),
                        )
                    })
                    .when(project.open_task_count > 0, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(project.open_task_count.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                        cx.emit(Open::Project {
                            id: id.clone(),
                            preview: event.click_count() < 2,
                        });
                    }))
                    .into_any_element(),
            );
        }
        let count = matches.len();
        let filtered = !self.filter.read(cx).value().trim().is_empty();

        v_flex()
            .key_context("ProjectsPanel")
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("PROJECTS"))
                    .child(div().child(format!("{count}")))
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("projects-archived")
                            .icon(Lucide::Archive)
                            .ghost()
                            .xsmall()
                            .selected(self.show_archived)
                            .tooltip(if self.show_archived {
                                "Hide archived projects"
                            } else {
                                "Show archived projects"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_archived = !this.show_archived;
                                this.refresh(cx);
                            })),
                    )
                    .child(
                        Button::new("projects-new")
                            .icon(IconName::Plus)
                            .ghost()
                            .xsmall()
                            .tooltip("New project")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ProjectsPanelEvent::NewProject)),
                            ),
                    )
                    .child(
                        Button::new("projects-refresh")
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
                    .id("projects-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(count == 0 && !self.loading, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(if filtered {
                                    "No projects match."
                                } else {
                                    "Nothing here yet. Projects are private to whoever creates \
                                     them, so this shows only what you own or what someone has \
                                     shared with you."
                                }),
                        )
                    }),
            )
    }
}
