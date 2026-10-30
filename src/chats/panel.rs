//! The chat list in the sidebar, Telegram's left column: pinned first, then
//! whatever moved last, each with its newest line and what's unread. The
//! filter tabs and the search are the server's — a search also finds what
//! was said, not just who it was with.
//!
//! The list and the rail's count refetch on any message event (the
//! workspace calls `refresh`), and once a minute besides, which is also what
//! keeps the green dots current.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{chat_avatar, preview};
use crate::actions::{SelectNext, SelectPrev};
use crate::api::ApiError;
use crate::api::models::{CHAT_FILTERS, Chat, ChatUnread};
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::format::ago_short;
use crate::ui::open::Open;

const POLL: Duration = Duration::from_secs(60);
const PAGE: usize = 100;
/// Typing in the search waits this long before asking the server.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

pub enum ChatsPanelEvent {
    NewGroup,
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for ChatsPanel {}
impl EventEmitter<ChatsPanelEvent> for ChatsPanel {}

pub struct ChatsPanel {
    ctx: OrgCtx,
    chats: Vec<Chat>,
    total: usize,
    unread: ChatUnread,
    filter: &'static str,
    search: Entity<InputState>,
    selected: Option<usize>,
    active_chat: Option<String>,
    loading: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
    _search: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
}

impl ChatsPanel {
    pub fn new(ctx: OrgCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search chats and messages…")
                .clean_on_escape()
        });
        let subscriptions = vec![cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, e: &InputEvent, _, cx| match e {
                InputEvent::Change => this.search_soon(cx),
                InputEvent::PressEnter { .. } => this.open_selected(cx),
                _ => {}
            },
        )];
        ctx.directory.update(cx, |d, cx| d.ensure_loaded(cx));
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        Self {
            ctx,
            chats: Vec::new(),
            total: 0,
            unread: ChatUnread::default(),
            filter: "all",
            search,
            selected: None,
            active_chat: None,
            loading: false,
            error: None,
            _subscriptions: subscriptions,
            _poll: poll,
            _search: None,
            _tasks: Vec::new(),
        }
    }

    /// Unread messages across the list, muted chats left out — the rail's
    /// number.
    pub fn unread(&self) -> i64 {
        self.unread.messages
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }

    pub fn set_active_chat(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_chat != id {
            self.active_chat = id;
            cx.notify();
        }
    }

    /// A chat on screen has been read: its badge goes now, the total with
    /// the next refetch.
    pub fn chat_read(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == id)
            && chat.unread > 0
        {
            if !chat.muted {
                self.unread.messages = (self.unread.messages - chat.unread).max(0);
            }
            chat.unread = 0;
            cx.notify();
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load(self.chats.len().clamp(PAGE, PAGE * 5), cx);
    }

    fn load(&mut self, limit: usize, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        let filter = self.filter;
        let q = self.search.read(cx).value().trim().to_owned();
        self.loading = true;
        cx.notify();
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let list = client.chats(&org_id, filter, &q, limit, 0)?;
                    let unread = client.chats_unread(&org_id)?;
                    Ok::<_, ApiError>((list, unread))
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    // A late answer for a tab no longer showing is dropped.
                    Ok(_) if this.filter != filter => {}
                    Ok(((chats, total), unread)) => {
                        this.chats = chats;
                        this.total = total;
                        this.unread = unread;
                        this.error = None;
                        if this.selected.is_some_and(|ix| ix >= this.chats.len()) {
                            this.selected = None;
                        }
                    }
                    Err(e) if e.is_auth() => cx.emit(ChatsPanelEvent::AuthFailed(e)),
                    // A failed poll keeps the last list: "couldn't check" is
                    // not "nothing new".
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn search_soon(&mut self, cx: &mut Context<Self>) {
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.selected = None;
                this.load(PAGE, cx);
            })
            .ok();
        }));
    }

    fn set_filter(&mut self, filter: &'static str, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.chats.clear();
            self.selected = None;
            self.load(PAGE, cx);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.chats.is_empty() {
            self.selected = Some(
                self.selected
                    .map_or(0, |ix| (ix + 1).min(self.chats.len() - 1)),
            );
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.selected = Some(ix.saturating_sub(1));
            cx.notify();
        }
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get(self.selected.unwrap_or(0)) {
            cx.emit(Open::Chat {
                id: chat.id.clone(),
                org_id: None,
                preview: false,
            });
        }
    }

    /// Your DM with somebody, found or started, then opened.
    fn message(&mut self, user_id: String, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.open_direct(&org_id, &user_id) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(chat) => cx.emit(Open::Chat {
                    id: chat.id,
                    org_id: None,
                    preview: false,
                }),
                Err(e) if e.is_auth() => cx.emit(ChatsPanelEvent::AuthFailed(e)),
                Err(e) => {
                    this.error = Some(e.to_string());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn render_row(&self, ix: usize, chat: &Chat, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let me = self.ctx.me().map(str::to_owned);
        let active = self.active_chat.as_deref() == Some(chat.id.as_str());
        let selected = self.selected == Some(ix);
        let unread = chat.unread > 0;
        let id = chat.id.clone();
        let when = chat
            .last_message
            .as_ref()
            .and_then(|m| m.created_at)
            .or(chat.last_message_at);
        let line = chat
            .last_message
            .as_ref()
            .map(|m| preview(chat, m))
            .unwrap_or_else(|| "No messages yet".into());
        let has_file = chat
            .last_message
            .as_ref()
            .is_some_and(|m| m.attachments > 0 && !m.deleted);
        let deleted = chat.last_message.as_ref().is_some_and(|m| m.deleted);
        h_flex()
            .id(SharedString::from(format!("chat-row-{}", chat.id)))
            .px_2()
            .py_1p5()
            .gap_2()
            .cursor_pointer()
            .when(active, |this| this.bg(theme.list_active))
            .when(selected && !active, |this| this.bg(theme.list_hover))
            .hover(|this| this.bg(theme.list_hover))
            .child(chat_avatar(chat, me.as_deref(), px(34.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1()
                            .text_sm()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .when(chat.task.as_ref().is_some_and(|t| t.closed), |d| {
                                        d.line_through().text_color(theme.muted_foreground)
                                    })
                                    .child(chat.title.clone()),
                            )
                            .when(chat.muted, |this| {
                                this.child(
                                    Icon::new(Lucide::BellOff)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                )
                            })
                            .when_some(when, |this, at| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(ago_short(at)),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .when(has_file, |this| {
                                this.child(Icon::new(Lucide::Paperclip).xsmall())
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .when(deleted, |d| d.italic())
                                    .child(line),
                            )
                            .when(chat.pinned && !unread, |this| {
                                this.child(Icon::new(Lucide::Pin).xsmall())
                            })
                            .when(unread, |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .min_w(px(18.))
                                        .px_1()
                                        .rounded_full()
                                        .text_center()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.background)
                                        .bg(if chat.muted {
                                            theme.muted_foreground
                                        } else {
                                            theme.blue
                                        })
                                        .child(if chat.unread > 99 {
                                            "99+".to_owned()
                                        } else {
                                            chat.unread.to_string()
                                        }),
                                )
                            }),
                    ),
            )
            .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                cx.emit(Open::Chat {
                    id: id.clone(),
                    org_id: None,
                    preview: event.click_count() < 2,
                });
            }))
            .into_any_element()
    }
}

impl Render for ChatsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let me = self.ctx.me().map(str::to_owned);
        let people: Vec<(String, String)> = self
            .ctx
            .directory
            .read(cx)
            .members
            .iter()
            .filter_map(|m| {
                let id = m.user_id.clone()?;
                let name = if me.as_deref() == Some(id.as_str()) {
                    "Saved messages".to_owned()
                } else {
                    m.name()
                };
                Some((id, name))
            })
            .collect();
        let this = cx.entity().downgrade();
        let new_menu = Button::new("chats-new")
            .icon(Lucide::SquarePen)
            .ghost()
            .xsmall()
            .tooltip("New message")
            .dropdown_menu(move |mut menu, _, _| {
                let group = this.clone();
                menu = menu
                    .item(PopupMenuItem::new("New group…").on_click(move |_, _, cx| {
                        group
                            .update(cx, |_, cx| cx.emit(ChatsPanelEvent::NewGroup))
                            .ok();
                    }))
                    .separator();
                for (id, name) in &people {
                    let this = this.clone();
                    let id = id.clone();
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.message(id.clone(), cx))
                            .ok();
                    }));
                }
                menu.max_h(px(420.)).scrollable(true)
            });
        let rows: Vec<AnyElement> = self
            .chats
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, chat)| self.render_row(ix, chat, cx))
            .collect();
        let more = self.total > self.chats.len();
        let empty = match (self.filter, self.search.read(cx).value().trim().is_empty()) {
            (_, false) => "Nothing matches.",
            ("unread", _) => "All read.",
            ("archived", _) => "Nothing archived.",
            _ => "No conversations yet — start one with the pen above.",
        };

        v_flex()
            .key_context("ChatsPanel")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("CHATS"))
                    .when(self.unread.messages > 0, |this| {
                        this.child(div().child(format!("{} unread", self.unread.messages)))
                    })
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(new_menu)
                    .child(
                        Button::new("chats-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                div().px_2().pb_1().flex_none().child(
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
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .px_2()
                    .pb_1p5()
                    .gap_1()
                    .children(CHAT_FILTERS.iter().map(|(key, label)| {
                        let key: &'static str = key;
                        let on = self.filter == key;
                        div()
                            .id(SharedString::from(format!("chat-filter-{key}")))
                            .px_1p5()
                            .py_0p5()
                            .rounded(theme.radius)
                            .text_xs()
                            .cursor_pointer()
                            .when(on, |d| d.bg(theme.list_active).text_color(theme.foreground))
                            .when(!on, |d| {
                                d.text_color(theme.muted_foreground)
                                    .hover(|d| d.bg(theme.list_hover))
                            })
                            .child(*label)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(key, cx)))
                    })),
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
                    .id("chats-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(more, |this| {
                        let shown = self.chats.len();
                        this.child(
                            div().p_2().child(
                                Button::new("chats-more")
                                    .label(format!("Show more ({} of {})", shown, self.total))
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.load(shown + PAGE, cx)
                                    })),
                            ),
                        )
                    })
                    .when(self.chats.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(empty),
                        )
                    }),
            )
    }
}
