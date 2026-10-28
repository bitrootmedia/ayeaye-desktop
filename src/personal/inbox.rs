//! Notifications: addressed to a person, not filed in an organisation, so the
//! list and the count ignore which organisation is selected — the same number
//! as the bell in the web app.
//!
//! The count is polled every minute, as the web app polls its badge; there
//! is no push channel on the server.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::models::{LinkTarget, Notification, parse_link};
use crate::api::{ApiClient, ApiError};
use crate::ui::format::ago_short;
use crate::ui::open::Open;

const POLL: Duration = Duration::from_secs(60);

pub enum InboxEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for InboxPanel {}
impl EventEmitter<InboxEvent> for InboxPanel {}

pub struct InboxPanel {
    client: Arc<ApiClient>,
    base_url: String,
    notifications: Vec<Notification>,
    unread: i64,
    loading: bool,
    error: Option<String>,
    _poll: Task<()>,
    _tasks: Vec<Task<()>>,
}

impl InboxPanel {
    pub fn new(client: Arc<ApiClient>, base_url: String, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        Self {
            client,
            base_url,
            notifications: Vec::new(),
            unread: 0,
            loading: false,
            error: None,
            _poll: poll,
            _tasks: Vec::new(),
        }
    }

    pub fn unread(&self) -> i64 {
        self.unread
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.loading = true;
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let list = client.notifications(50)?;
                    let unread = client.unread_count()?;
                    Ok::<_, ApiError>((list, unread))
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((list, unread)) => {
                        this.notifications = list;
                        this.unread = unread;
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(InboxEvent::AuthFailed(e)),
                    // A failed poll keeps the last count: "couldn't check"
                    // is not "nothing new".
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open(&mut self, n: Notification, cx: &mut Context<Self>) {
        if n.read_at.is_none() {
            self.mark_read(n.id.clone(), cx);
        }
        let Some(path) = n.link_path.clone() else {
            return;
        };
        let open = match parse_link(&path) {
            Some(LinkTarget::Task { org_id, task_id }) => Open::Task {
                id: task_id,
                org_id: Some(org_id),
                preview: false,
            },
            Some(LinkTarget::Person { org_id, user_id }) => Open::Person {
                id: user_id,
                org_id: Some(org_id),
                preview: false,
            },
            _ => Open::Web(format!("{}{}", self.base_url, path)),
        };
        cx.emit(open);
    }

    fn mark_read(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(n) = self
            .notifications
            .iter_mut()
            .find(|n| n.id == id && n.read_at.is_none())
        {
            n.read_at = Some(chrono::Utc::now());
            self.unread = (self.unread - 1).max(0);
        }
        cx.notify();
        let client = self.client.clone();
        self._tasks.push(cx.background_spawn(async move {
            let _ = client.mark_read(&id);
        }));
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let now = chrono::Utc::now();
        for n in &mut self.notifications {
            n.read_at.get_or_insert(now);
        }
        self.unread = 0;
        cx.notify();
        let client = self.client.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.mark_all_read() })
                .await;
            this.update(cx, |this, cx| {
                if result.is_err() {
                    this.refresh(cx);
                }
            })
            .ok();
        }));
    }
}

impl Render for InboxPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("INBOX"))
                    .when(self.unread > 0, |this| {
                        this.child(div().child(format!("{} unread", self.unread)))
                    })
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .when(self.unread > 0, |this| {
                        this.child(
                            Button::new("inbox-read-all")
                                .icon(IconName::Check)
                                .ghost()
                                .xsmall()
                                .tooltip("Mark all read")
                                .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                        )
                    })
                    .child(
                        Button::new("inbox-refresh")
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
                    .id("inbox-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(self.notifications.iter().map(|n| {
                        let unread = n.read_at.is_none();
                        let item = n.clone();
                        v_flex()
                            .id(SharedString::from(format!("n-{}", n.id)))
                            .px_3()
                            .py_1p5()
                            .gap_0p5()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(
                                        div()
                                            .flex_none()
                                            .size(px(6.))
                                            .rounded_full()
                                            .when(unread, |d| d.bg(theme.blue)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                            .when(!unread, |d| d.text_color(theme.muted_foreground))
                                            .child(n.title.clone()),
                                    )
                                    .when_some(n.created_at, |this, at| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(ago_short(at)),
                                        )
                                    }),
                            )
                            .when_some(
                                n.body.clone().filter(|b| !b.trim().is_empty()),
                                |this, body| {
                                    this.child(
                                        div()
                                            .pl(px(14.))
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .line_clamp(2)
                                            .child(body),
                                    )
                                },
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open(item.clone(), cx)),
                            )
                    }))
                    .when(self.notifications.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Nothing new."),
                        )
                    }),
            )
    }
}
