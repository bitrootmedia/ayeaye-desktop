//! A knowledge-base article in a tab: read it rendered, edit it as markdown.
//!
//! Editing goes through the server's edit session: `edit-session` hands back
//! the revision to write into, and saves are debounced PATCHes of that
//! revision — the same autosave shape as the notepad. A 409 means someone
//! started a newer session; the editor stops rather than overwrite it.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::ApiError;
use crate::api::models::{Article, Revision};
use crate::api::revision_files_base;
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::files::{FilesEvent, FilesPanel, drop_target};
use crate::ui::format::ago_long;
use crate::ui::item::{Item, ItemEvent};
use crate::ui::widgets::{html_to_markdown, markdown_text, rich_text};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(1000);

pub enum ArticleEvent {
    /// Title, publication or deletion changed what the sidebar shows.
    Changed {
        book_id: String,
    },
    AuthFailed(ApiError),
}

impl EventEmitter<ItemEvent> for ArticleView {}
impl EventEmitter<ArticleEvent> for ArticleView {}

struct Editing {
    revision_id: String,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    dirty: bool,
    saving: bool,
    /// The draft rendered instead of its source, to check it reads right.
    preview: bool,
    saved_at: Option<chrono::DateTime<chrono::Utc>>,
    _subscriptions: Vec<Subscription>,
}

pub struct ArticleView {
    ctx: OrgCtx,
    article_id: String,
    article: Option<Article>,
    revision: Option<Revision>,
    error: Option<String>,
    editing: Option<Editing>,
    busy: bool,
    /// The files on the revision on screen: the one being read, or the one
    /// this edit session writes into.
    files: Entity<FilesPanel>,
    focus_handle: FocusHandle,
    debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl ArticleView {
    pub fn new(
        ctx: OrgCtx,
        article_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let files = cx.new(|_| FilesPanel::new(ctx.clone(), "article"));
        let subscriptions =
            vec![
                cx.subscribe(&files, |_, _, event: &FilesEvent, cx| match event {
                    FilesEvent::AuthFailed(e) => cx.emit(ArticleEvent::AuthFailed(e.clone())),
                }),
            ];
        let mut view = Self {
            ctx,
            article_id,
            article: None,
            revision: None,
            error: None,
            editing: None,
            busy: false,
            files,
            focus_handle: cx.focus_handle(),
            debounce: None,
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        view.reload(window, cx);
        view
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let id = self.article_id.clone();
        self.busy = true;
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let article = client.article(&org_id, &id)?;
                    let revision = client.current_revision(&org_id, &id)?;
                    Ok::<_, ApiError>((article, revision))
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((article, revision)) => {
                        this.article = Some(article);
                        this.revision = revision;
                        this.error = None;
                        this.sync_files(cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(ArticleEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let id = self.article_id.clone();
        self.busy = true;
        cx.notify();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.start_edit_session(&org_id, &id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(revision) => this.open_editor(revision, window, cx),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_editor(&mut self, revision: Revision, window: &mut Window, cx: &mut Context<Self>) {
        let markdown = html_to_markdown(&revision.body);
        let title_text = if revision.title.is_empty() {
            self.article
                .as_ref()
                .map(|a| a.title.clone())
                .unwrap_or_default()
        } else {
            revision.title.clone()
        };
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Title")
                .default_value(title_text)
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write — markdown works: ## headings, **bold**, - lists, `code`")
                .soft_wrap(true)
                .searchable(true)
                .default_value(markdown)
        });
        body.update(cx, |b, cx| b.focus(window, cx));
        let changed =
            |this: &mut Self, _: &_, e: &InputEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let InputEvent::Change = e {
                    this.edited(cx);
                }
            };
        let subscriptions = vec![
            cx.subscribe_in(&title, window, changed),
            cx.subscribe_in(
                &body,
                window,
                move |this: &mut Self, _, e: &InputEvent, _, cx| {
                    if let InputEvent::Change = e {
                        this.edited(cx);
                    }
                },
            ),
        ];
        self.editing = Some(Editing {
            revision_id: revision.id.clone(),
            title,
            body,
            dirty: false,
            saving: false,
            preview: false,
            saved_at: revision.updated_at,
            _subscriptions: subscriptions,
        });
        self.sync_files(cx);
        cx.emit(ItemEvent::Edited);
    }

    fn sync_files(&mut self, cx: &mut Context<Self>) {
        let revision_id = match &self.editing {
            Some(e) => Some(e.revision_id.clone()),
            None => self.revision.as_ref().map(|r| r.id.clone()),
        };
        let base = revision_id.map(|id| revision_files_base(self.ctx.org_id(), &id));
        let can_edit = self.article.as_ref().is_some_and(|a| a.can_edit());
        self.files
            .update(cx, |files, cx| files.set_target(base, can_edit, cx));
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &mut self.editing else {
            return;
        };
        editing.dirty = true;
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| this.save(cx)).ok();
        }));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &mut self.editing else {
            return;
        };
        if !editing.dirty || editing.saving {
            return;
        }
        editing.dirty = false;
        editing.saving = true;
        let title = editing.title.read(cx).value().trim().to_owned();
        let markdown = editing.body.read(cx).value().to_string();
        let revision_id = editing.revision_id.clone();
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        cx.notify();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.save_revision(&org_id, &revision_id, &title, &markdown) })
                .await;
            this.update(cx, |this, cx| {
                let Some(editing) = &mut this.editing else { return };
                editing.saving = false;
                match result {
                    Ok(revision) => {
                        editing.saved_at = revision.updated_at;
                        this.revision = Some(revision);
                        if this.editing.as_ref().is_some_and(|e| e.dirty) {
                            this.save(cx);
                        }
                    }
                    Err(ApiError::Server { status: 409, .. }) => {
                        // Superseded: stop writing into a revision that is no
                        // longer current. The text stays on screen.
                        editing.dirty = true;
                        this.error = Some(
                            "Someone started a newer edit of this article. Copy what you need, then reload.".into(),
                        );
                    }
                    Err(e) => {
                        editing.dirty = true;
                        this.error = Some(format!("Not saved — {e}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = &mut self.editing else {
            return;
        };
        editing.preview = !editing.preview;
        if !editing.preview {
            editing.body.update(cx, |b, cx| b.focus(window, cx));
        }
        cx.notify();
    }

    fn finish_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = None;
        self.save(cx);
        if self.editing.as_ref().is_some_and(|e| e.dirty || e.saving) {
            // Let the save land, then leave edit mode.
            self._tasks.push(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let pending = this
                        .update(cx, |this, _| {
                            this.editing.as_ref().is_some_and(|e| e.saving)
                        })
                        .unwrap_or(false);
                    if !pending {
                        break;
                    }
                }
                this.update_in(cx, |this, window, cx| this.close_editor(window, cx))
                    .ok();
            }));
        } else {
            self.close_editor(window, cx);
        }
    }

    fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_ref().is_some_and(|e| e.dirty) {
            return; // a failed save: keep the editor and its text
        }
        self.editing = None;
        if let Some(article) = &self.article {
            cx.emit(ArticleEvent::Changed {
                book_id: article.book_id.clone(),
            });
        }
        self.reload(window, cx);
    }

    fn set_private(&mut self, private: bool, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let id = self.article_id.clone();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.set_article_private(&org_id, &id, private) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(article) => {
                        cx.emit(ArticleEvent::Changed {
                            book_id: article.book_id.clone(),
                        });
                        this.article = Some(article);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(article) = self.article.clone() else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(format!("Delete “{}”?", article.title))
                .description(
                    "The article and its revision history are deleted for everyone who can see it.",
                )
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    this.update(cx, |this, cx| this.delete(cx)).ok();
                    true
                })
        });
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let id = self.article_id.clone();
        let book_id = self
            .article
            .as_ref()
            .map(|a| a.book_id.clone())
            .unwrap_or_default();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.delete_article(&org_id, &id) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(()) | Err(ApiError::NotFound(_)) => {
                    cx.emit(ArticleEvent::Changed { book_id });
                    cx.emit(ItemEvent::CloseRequested);
                }
                Err(e) => {
                    this.error = Some(e.to_string());
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

/// The title above a read article: the web app's `text-xl font-semibold`.
pub fn read_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(20.))
        .line_height(px(28.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}

impl Focusable for ArticleView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editing {
            Some(e) => e.body.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl Item for ArticleView {
    fn item_key(&self, _: &App) -> SharedString {
        format!("article:{}", self.article_id).into()
    }

    fn tab_title(&self, cx: &App) -> SharedString {
        if let Some(e) = &self.editing {
            let t = e.title.read(cx).value().trim().to_owned();
            if !t.is_empty() {
                return t.into();
            }
        }
        self.article
            .as_ref()
            .map(|a| a.title.clone())
            .unwrap_or_else(|| "Article".into())
            .into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::BookText)
    }

    fn is_dirty(&self, _: &App) -> bool {
        self.editing.as_ref().is_some_and(|e| e.dirty || e.saving)
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let state = match &self.editing {
            Some(e) if e.saving => "Saving…".to_owned(),
            Some(e) if e.dirty => "Edited".to_owned(),
            Some(e) => e
                .saved_at
                .map(|at| format!("Saved {}", ago_long(at)))
                .unwrap_or_else(|| "Editing".into()),
            None => match &self.article {
                Some(a) if a.is_private => "Draft — only you can see it".into(),
                Some(_) => "Published".into(),
                None => return None,
            },
        };
        Some(state.into())
    }

    fn flush(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.debounce = None;
        self.save(cx);
    }
}

impl Render for ArticleView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let header = self.article.clone().map(|article| {
            let can_edit = article.can_edit();
            let is_owner = article.access == "owner";
            let web = self.ctx.session.web_url(&format!(
                "/orgs/{}/kb/books/{}",
                self.ctx.org_id(),
                article.book_id
            ));
            h_flex()
                .w_full()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .px_1p5()
                        .rounded(theme.radius)
                        .bg(if article.is_private {
                            theme.warning.opacity(0.2)
                        } else {
                            theme.success.opacity(0.2)
                        })
                        .text_xs()
                        .child(if article.is_private {
                            "Draft"
                        } else {
                            "Published"
                        }),
                )
                .when_some(article.owner.clone(), |this, owner| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(owner.name()),
                    )
                })
                .when_some(article.updated_at, |this, at| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("· updated {}", ago_long(at))),
                    )
                })
                .child(div().w(px(12.)))
                .when(self.editing.is_none() && can_edit, |this| {
                    this.child(
                        Button::new("edit")
                            .icon(Lucide::SquarePen)
                            .label("Edit")
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.start_editing(window, cx)),
                            ),
                    )
                })
                .when_some(self.editing.as_ref().map(|e| e.preview), |this, preview| {
                    this.child(
                        Button::new("preview")
                            .icon(if preview {
                                Lucide::SquarePen
                            } else {
                                Lucide::Eye
                            })
                            .label(if preview { "Write" } else { "Preview" })
                            .ghost()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_preview(window, cx)),
                            ),
                    )
                })
                .when(self.editing.is_some(), |this| {
                    this.child(
                        Button::new("done")
                            .icon(IconName::Check)
                            .label("Done")
                            .primary()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.finish_editing(window, cx)),
                            ),
                    )
                })
                .when(is_owner && article.is_private, |this| {
                    this.child(
                        Button::new("publish")
                            .label("Publish")
                            .small()
                            .tooltip("Make it visible to everyone the book is shared with")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_private(false, window, cx)
                            })),
                    )
                })
                .when(article.can_make_private && !article.is_private, |this| {
                    this.child(
                        Button::new("unpublish")
                            .label("Unpublish")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_private(true, window, cx)
                            })),
                    )
                })
                .child(
                    Button::new("web")
                        .icon(IconName::ExternalLink)
                        .ghost()
                        .small()
                        .tooltip("Open in browser")
                        .on_click(move |_, _, cx| cx.open_url(&web)),
                )
                .when(is_owner, |this| {
                    this.child(
                        Button::new("delete")
                            .icon(Lucide::Trash)
                            .ghost()
                            .small()
                            .tooltip("Delete article")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_delete(window, cx)),
                            ),
                    )
                })
                .when(self.busy, |this| this.child(Spinner::new().small()))
        });

        let body = match (&self.editing, &self.revision, &self.article) {
            (Some(e), _, _) => v_flex()
                .flex_1()
                .min_h_0()
                .gap_2()
                .child(
                    Input::new(&e.title)
                        .appearance(false)
                        .px_0()
                        .h(px(46.))
                        .text_size(px(26.))
                        .line_height(px(36.))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(if e.preview {
                    let draft = e.body.read(cx).value().to_string();
                    div()
                        .id("article-preview")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(
                            div()
                                .max_w(px(780.))
                                .pb_8()
                                .child(if draft.trim().is_empty() {
                                    div()
                                        .text_color(theme.muted_foreground)
                                        .child("Nothing to preview yet.")
                                        .into_any_element()
                                } else {
                                    markdown_text("article-preview-body", &draft)
                                }),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Textarea::new(&e.body)
                                .appearance(false)
                                .h_full()
                                .font_family(mono)
                                .text_size(px(14.))
                                .line_height(relative(1.6)),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .id("article-files")
                        .flex_none()
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .child(self.files.clone()),
                )
                .into_any_element(),
            (None, Some(rev), Some(article)) => div()
                .id("article-read")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .max_w(px(780.))
                        .gap_4()
                        .pb_8()
                        .child(read_title(if rev.title.is_empty() {
                            article.title.clone()
                        } else {
                            rev.title.clone()
                        }))
                        .child(if rev.body.trim().is_empty() {
                            div()
                                .text_color(theme.muted_foreground)
                                .child("This article is empty.")
                                .into_any_element()
                        } else {
                            rich_text(SharedString::from(format!("article-{}", rev.id)), &rev.body)
                        })
                        .child(self.files.clone()),
                )
                .into_any_element(),
            (None, None, Some(article)) => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_size(px(26.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(article.title.clone()),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("This article is empty."),
                )
                .into_any_element(),
            _ => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child("Loading…")
                .into_any_element(),
        };

        let view = v_flex()
            .id("article-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .px_6()
            .py_4()
            .gap_3()
            .children(header)
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
            .child(body);
        drop_target(view, &self.files, cx)
    }
}
