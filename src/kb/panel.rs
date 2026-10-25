//! The knowledge base in the sidebar: books, each opening to its articles.
//! Articles load per book on first expand — a big knowledge base is many
//! books nobody opens today.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::ApiError;
use crate::api::models::{Article, Book};
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::open::Open;

pub enum KbPanelEvent {
    NewBook,
    NewArticle { book_id: String },
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for KbPanel {}
impl EventEmitter<KbPanelEvent> for KbPanel {}

pub struct KbPanel {
    ctx: OrgCtx,
    books: Vec<Book>,
    articles: HashMap<String, Vec<Article>>,
    expanded: HashSet<String>,
    loading: bool,
    error: Option<String>,
    active_article: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl KbPanel {
    pub fn new(ctx: OrgCtx, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            ctx,
            books: Vec::new(),
            articles: HashMap::new(),
            expanded: HashSet::new(),
            loading: false,
            error: None,
            active_article: None,
            _tasks: Vec::new(),
        };
        panel.refresh(cx);
        panel
    }

    pub fn set_active_article(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_article != id {
            self.active_article = id;
            cx.notify();
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let expanded: Vec<String> = self.expanded.iter().cloned().collect();
        self.loading = true;
        cx.notify();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let books = client.books(&org_id)?;
                    let articles: Vec<(String, Vec<Article>)> = expanded
                        .into_iter()
                        .filter_map(|b| client.articles(&org_id, &b).ok().map(|a| (b, a)))
                        .collect();
                    Ok::<_, ApiError>((books, articles))
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((mut books, articles)) => {
                        books.retain(|b| !b.archived);
                        books.sort_by_key(|b| b.name.to_lowercase());
                        this.books = books;
                        this.articles.extend(articles);
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(KbPanelEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn reload_book(&mut self, book_id: &str, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let book_id = book_id.to_owned();
        self.expanded.insert(book_id.clone());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let id = book_id.clone();
            let result = cx
                .background_spawn(async move { client.articles(&org_id, &id) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(articles) => {
                        this.articles.insert(book_id, articles);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle(&mut self, book_id: String, cx: &mut Context<Self>) {
        if self.expanded.remove(&book_id) {
            cx.notify();
        } else {
            self.reload_book(&book_id, cx);
        }
    }
}

impl Render for KbPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for book in &self.books {
            let open = self.expanded.contains(&book.id);
            let book_id = book.id.clone();
            let new_id = book.id.clone();
            let group: SharedString = format!("book-{}", book.id).into();
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("book-{}", book.id)))
                    .group(group.clone())
                    .h(px(26.))
                    .px_2()
                    .gap_1p5()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover))
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(theme.muted_foreground),
                    )
                    .child(
                        Icon::new(Lucide::BookOpen)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(book.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .group_hover(group.clone(), |s| s.invisible())
                            .when(book.article_count > 0, |d| {
                                d.child(format!("{}", book.article_count))
                            }),
                    )
                    .when(book.access != "read", |this| {
                        this.child(
                            div()
                                .absolute()
                                .right_1()
                                .invisible()
                                .group_hover(group, |s| s.visible())
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "new-article-{}",
                                        book.id
                                    )))
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("New article")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.emit(KbPanelEvent::NewArticle {
                                            book_id: new_id.clone(),
                                        })
                                    })),
                                ),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(book_id.clone(), cx)))
                    .into_any_element(),
            );
            if !open {
                continue;
            }
            match self.articles.get(&book.id) {
                None => rows.push(
                    div()
                        .pl(px(42.))
                        .h(px(24.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Loading…")
                        .into_any_element(),
                ),
                Some(articles) if articles.is_empty() => rows.push(
                    div()
                        .pl(px(42.))
                        .h(px(24.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("No articles yet")
                        .into_any_element(),
                ),
                Some(articles) => {
                    for article in articles {
                        let id = article.id.clone();
                        let active = self.active_article.as_deref() == Some(id.as_str());
                        rows.push(
                            h_flex()
                                .id(SharedString::from(format!("article-{}", article.id)))
                                .h(px(26.))
                                .pl(px(34.))
                                .pr_2()
                                .gap_1p5()
                                .text_sm()
                                .cursor_pointer()
                                .when(active, |this| this.bg(theme.list_active))
                                .hover(|this| this.bg(theme.list_hover))
                                .child(
                                    Icon::new(IconName::FileText)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(article.title.clone()),
                                )
                                .when(article.is_private, |this| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child("draft"),
                                    )
                                })
                                .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                                    cx.emit(Open::Article {
                                        id: id.clone(),
                                        preview: event.click_count() < 2,
                                    })
                                }))
                                .into_any_element(),
                        );
                    }
                }
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
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("KNOWLEDGE BASE"),
                    )
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("kb-new-book")
                            .icon(IconName::Plus)
                            .ghost()
                            .xsmall()
                            .tooltip("New book")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(KbPanelEvent::NewBook))),
                    )
                    .child(
                        Button::new("kb-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
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
                    .id("kb-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(self.books.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No books yet."),
                        )
                    }),
            )
    }
}
