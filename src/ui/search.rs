//! ⌘⇧F: the organisation's search — tasks, projects, articles, comments —
//! answered by the server as you type.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{Cancel, SelectNext, SelectPrev};
use crate::api::models::SearchHit;
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::open::Open;

const DEBOUNCE: Duration = Duration::from_millis(220);

pub enum SearchEvent {
    Open(Open),
    Dismissed,
}

impl EventEmitter<SearchEvent> for SearchView {}

pub struct SearchView {
    ctx: OrgCtx,
    query: Entity<InputState>,
    hits: Vec<SearchHit>,
    selected: usize,
    loading: bool,
    error: Option<String>,
    /// The query the hits belong to; a slower earlier answer must not
    /// replace a newer one.
    answered: String,
    _subscription: Subscription,
    _search: Option<Task<()>>,
}

impl SearchView {
    pub fn new(ctx: OrgCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx
            .new(|cx| InputState::new(window, cx).placeholder(format!("Search {}…", ctx.org.name)));
        query.update(cx, |q, cx| q.focus(window, cx));
        let subscription = cx.subscribe_in(
            &query,
            window,
            |this: &mut Self, _, e: &InputEvent, _, cx| match e {
                InputEvent::Change => this.schedule(cx),
                InputEvent::PressEnter { .. } => this.confirm(this.selected, cx),
                _ => {}
            },
        );
        Self {
            ctx,
            query,
            hits: Vec::new(),
            selected: 0,
            loading: false,
            error: None,
            answered: String::new(),
            _subscription: subscription,
            _search: None,
        }
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        let q = self.query.read(cx).value().trim().to_owned();
        if q.len() < 2 {
            self.hits.clear();
            self._search = None;
            cx.notify();
            return;
        }
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self.loading = true;
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let asked = q.clone();
            let result = cx
                .background_spawn(async move { client.search(&org_id, &asked) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(results) => {
                        this.hits = results.hits;
                        this.answered = q;
                        this.selected = 0;
                        this.error = None;
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_for(&self, hit: &SearchHit) -> Open {
        let org = self.ctx.org_id();
        match hit.kind.as_str() {
            "task" => Open::Task {
                id: hit.id.clone(),
                org_id: None,
                preview: false,
            },
            "article" => Open::Article {
                id: hit.id.clone(),
                preview: false,
            },
            "project" => Open::Web(
                self.ctx
                    .session
                    .web_url(&format!("/orgs/{org}/projects/{}", hit.id)),
            ),
            "book" => Open::Web(
                self.ctx
                    .session
                    .web_url(&format!("/orgs/{org}/kb/books/{}", hit.id)),
            ),
            _ => Open::Web(
                self.ctx
                    .session
                    .web_url(&format!("/orgs/{org}/search?q={}", self.answered)),
            ),
        }
    }

    fn confirm(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(hit) = self.hits.get(ix) {
            let open = self.open_for(hit);
            cx.emit(SearchEvent::Open(open));
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.hits.is_empty() {
            self.selected = (self.selected + 1) % self.hits.len();
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if !self.hits.is_empty() {
            self.selected = (self.selected + self.hits.len() - 1) % self.hits.len();
            cx.notify();
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SearchEvent::Dismissed);
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let typed = self.query.read(cx).value().trim().len();
        v_flex()
            .key_context("QuickOpen")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Dismissed)))
            .w(px(640.))
            .max_h(px(520.))
            .p_1()
            .gap_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.input)
            .bg(theme.popover)
            .shadow_lg()
            .child(
                Input::new(&self.query)
                    .appearance(false)
                    .prefix(
                        Icon::new(IconName::Search)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .when(self.loading, |i| i.suffix(Spinner::new().small())),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .when_some(self.error.clone(), |this, e| {
                this.child(div().p_2().text_sm().child(e))
            })
            .child(
                v_flex()
                    .id("search-hits")
                    .overflow_y_scroll()
                    .children(self.hits.iter().enumerate().map(|(ix, hit)| {
                        let icon = match hit.kind.as_str() {
                            "task" => Icon::new(Lucide::SquareCheck),
                            "article" => Icon::new(Lucide::BookText),
                            "book" => Icon::new(Lucide::BookOpen),
                            "project" => Icon::new(IconName::Folder),
                            _ => Icon::new(IconName::FileText),
                        };
                        h_flex()
                            .id(("hit", ix))
                            .px_3()
                            .py_1p5()
                            .gap_2()
                            .rounded(theme.radius)
                            .cursor_pointer()
                            .when(ix == self.selected, |s| s.bg(theme.list_active))
                            .hover(|s| s.bg(theme.list_hover))
                            .child(icon.small().text_color(theme.muted_foreground))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .truncate()
                                            .when(hit.inactive, |d| {
                                                d.line_through().text_color(theme.muted_foreground)
                                            })
                                            .child(hit.title.clone()),
                                    )
                                    .when_some(
                                        hit.context.clone().or_else(|| hit.subtitle.clone()),
                                        |this, ctx| {
                                            this.child(
                                                div()
                                                    .text_xs()
                                                    .truncate()
                                                    .text_color(theme.muted_foreground)
                                                    .child(ctx),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(hit.kind.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.confirm(ix, cx)))
                    }))
                    .when(
                        self.hits.is_empty() && typed >= 2 && !self.loading && self.error.is_none(),
                        |this| {
                            this.child(
                                div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Nothing found."),
                            )
                        },
                    )
                    .when(typed < 2, |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(
                                    "Tasks, projects, articles and comments in this organisation.",
                                ),
                        )
                    }),
            )
    }
}
