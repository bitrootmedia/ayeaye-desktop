//! One chat in a tab: a DM, a group, or a task's or project's comments seen
//! as a chat. Telegram's right column — your messages on the right, theirs
//! on the left, a line between days, a quoted strip over a reply — opening
//! at the newest page and fetching older ones as you scroll up.
//!
//! Return sends, Shift-Return is a new line. Sending is not optimistic: the
//! message appears once the server has it, and if it's refused the text goes
//! back in the box. Reading is reported when the tab is in front, up to the
//! newest message on screen.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Theme, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use super::{chat_avatar, day_label, seen_line};
use crate::api::models::{Chat, ChatPage, FileItem, Message, Person, priority_label, status_label};
use crate::api::{ApiClient, ApiError, ApiResult};
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::files::{MAX_BYTES, content_type_for, file_name, format_bytes, sniff_image};
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;
use crate::ui::widgets::{avatar, html_to_markdown, plain_line, rich_text};

const PAGE: usize = 50;
/// Presence and the header refetch on this beat while the tab is open.
const META_EVERY: Duration = Duration::from_secs(60);
/// Messages from one person this close together share one name and avatar.
const RUN_GAP: i64 = 5 * 60;
/// How near the top (or bottom) of the scroll counts as there.
const EDGE: Pixels = px(48.);

/// Raised so the workspace can move the list and the rail's count.
pub enum ChatViewEvent {
    /// Read up to the newest message.
    Read(String),
    /// Renamed, pinned, muted, archived, a message sent: the list moved.
    Changed,
    /// You left the group; the tab closes itself.
    Left,
    AuthFailed(ApiError),
}

impl EventEmitter<ItemEvent> for ChatView {}
impl EventEmitter<ChatViewEvent> for ChatView {}
impl EventEmitter<Open> for ChatView {}

/// A file uploaded and waiting for the message it goes with.
#[derive(Clone)]
struct Staged {
    id: String,
    name: String,
}

pub struct ChatView {
    ctx: OrgCtx,
    chat_id: String,
    chat: Option<Chat>,
    messages: Vec<Message>,
    has_more: bool,
    load_error: Option<String>,
    error: Option<String>,
    loading_older: bool,
    sending: bool,
    composer: Entity<TextareaState>,
    title: Entity<InputState>,
    reply_to: Option<Message>,
    editing: Option<String>,
    staged: Vec<Staged>,
    uploading: Option<String>,
    /// The tab is in front, so what arrives is read.
    visible: bool,
    read_upto: Option<String>,
    /// Scroll so this message is at the top, once it's laid out — where the
    /// reader was before an older page went in above it.
    anchor: Option<String>,
    to_bottom: bool,
    scroll: ScrollHandle,
    thumbnails: HashMap<String, Arc<Image>>,
    fetching: HashSet<String>,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
    _tasks: Vec<Task<()>>,
}

impl ChatView {
    pub fn new(ctx: OrgCtx, chat_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write a message…")
                .auto_grow(1, 10)
                .submit_on_enter(true)
        });
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Group name"));
        let subscriptions = vec![
            cx.subscribe_in(&composer, window, Self::on_composer_event),
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.observe(&ctx.directory, |_, _, cx| cx.notify()),
        ];
        ctx.directory.update(cx, |d, cx| d.ensure_loaded(cx));
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(META_EVERY).await;
                if this.update(cx, |this, cx| this.reload_meta(cx)).is_err() {
                    break;
                }
            }
        });
        let mut view = Self {
            ctx,
            chat_id,
            chat: None,
            messages: Vec::new(),
            has_more: false,
            load_error: None,
            error: None,
            loading_older: false,
            sending: false,
            composer,
            title,
            reply_to: None,
            editing: None,
            staged: Vec::new(),
            uploading: None,
            visible: false,
            read_upto: None,
            anchor: None,
            to_bottom: true,
            scroll: ScrollHandle::new(),
            thumbnails: HashMap::new(),
            fetching: HashSet::new(),
            _subscriptions: subscriptions,
            _poll: poll,
            _tasks: Vec::new(),
        };
        view.reload(window, cx);
        view
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    /// What the thread socket watches while this is in front: the task or
    /// project for a thread (that's where its events go for someone without
    /// a stake), the chat itself otherwise.
    pub fn watch_key(&self) -> String {
        match &self.chat {
            Some(chat) if chat.kind == "task" => match &chat.task {
                Some(task) => format!("task:{}", task.id),
                None => format!("chat:{}", self.chat_id),
            },
            Some(chat) if chat.kind == "project" => match &chat.project_id {
                Some(id) => format!("project:{id}"),
                None => format!("chat:{}", self.chat_id),
            },
            _ => format!("chat:{}", self.chat_id),
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            if visible {
                self.mark_read(cx);
            }
        }
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    fn org_id(&self) -> String {
        self.ctx.org_id().to_owned()
    }

    /// The chat and its newest page. Older pages already loaded stay: the
    /// newest page replaces only the stretch it covers.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.org_id();
        let chat_id = self.chat_id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let chat = client.chat(&org_id, &chat_id)?;
                    let page = client.chat_messages(&org_id, &chat_id, None, PAGE)?;
                    Ok::<_, ApiError>((chat, page))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((chat, page)) => {
                        this.load_error = None;
                        this.set_chat(chat, window, cx);
                        this.take_newest(page);
                        this.fetch_thumbnails(cx);
                        this.mark_read(cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(ChatViewEvent::AuthFailed(e)),
                    Err(e) => this.load_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    /// Only the header — who's online — on the minute.
    fn reload_meta(&mut self, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.org_id();
        let chat_id = self.chat_id.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.chat(&org_id, &chat_id) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(chat) = result {
                    this.chat = Some(chat);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn set_chat(&mut self, chat: Chat, window: &mut Window, cx: &mut Context<Self>) {
        // A name being typed isn't overwritten by the server's.
        if chat.kind == "group" && !self.title.read(cx).focus_handle(cx).is_focused(window) {
            let title = chat.title.clone();
            self.title
                .update(cx, |t, cx| t.set_value(title, window, cx));
        }
        self.chat = Some(chat);
    }

    fn take_newest(&mut self, page: ChatPage) {
        let at_bottom = self.at_bottom();
        let newest_before = self.messages.last().map(|m| m.id.clone());
        let first = page.messages.first().map(|m| m.id.clone());
        let mut merged: Vec<Message> = match (&first, page.has_more) {
            // UUIDv7 ids sort by time, as strings too.
            (Some(first), true) => self
                .messages
                .iter()
                .filter(|m| m.id < *first)
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        if merged.is_empty() {
            self.has_more = page.has_more;
        }
        merged.extend(page.messages);
        let grew = merged.last().map(|m| m.id.clone()) != newest_before;
        self.messages = merged;
        if grew && (at_bottom || self.messages.last().is_some_and(|m| m.mine)) {
            self.to_bottom = true;
        }
    }

    /// Pictures in messages are drawn from their thumbnail, fetched once per
    /// file — the Files panel's way, and the original stands in until the
    /// worker has made one.
    fn fetch_thumbnails(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, String)> = self
            .messages
            .iter()
            .filter(|m| !m.deleted)
            .flat_map(|m| m.attachments.iter())
            .filter(|f| f.is_image())
            .filter(|f| !self.thumbnails.contains_key(&f.id) && !self.fetching.contains(&f.id))
            .map(|f| {
                (
                    f.id.clone(),
                    f.thumbnail_url.clone().unwrap_or(f.url.clone()),
                )
            })
            .collect();
        for (id, url) in wanted {
            self.fetching.insert(id.clone());
            let client = self.ctx.client();
            self._tasks.push(cx.spawn(async move |this, cx| {
                let image = cx
                    .background_spawn(async move {
                        let bytes = client
                            .get_from_storage(&url)
                            .inspect_err(|e| log::debug!("chat thumbnail: {e}"))
                            .ok()?;
                        let format = sniff_image(&bytes)?;
                        Some(Arc::new(Image::from_bytes(format, bytes)))
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(image) = image {
                        this.thumbnails.insert(id, image);
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
    }

    fn at_bottom(&self) -> bool {
        let offset = self.scroll.offset().y;
        let max = self.scroll.max_offset().y;
        max <= px(0.) || -offset >= max - EDGE
    }

    fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.loading_older || !self.has_more {
            return;
        }
        let Some(first) = self.messages.first().map(|m| m.id.clone()) else {
            return;
        };
        self.loading_older = true;
        cx.notify();
        let client = self.ctx.client();
        let org_id = self.org_id();
        let chat_id = self.chat_id.clone();
        let before = first.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.chat_messages(&org_id, &chat_id, Some(&before), PAGE)
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading_older = false;
                match result {
                    Ok(page) => {
                        this.has_more = page.has_more;
                        let mut older = page.messages;
                        older.retain(|m| m.id < first);
                        older.append(&mut this.messages);
                        this.messages = older;
                        this.anchor = Some(first);
                        this.fetch_thumbnails(cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(ChatViewEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Tells the server — once per newest message — that it's been seen,
    /// while the tab is in front.
    fn mark_read(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let Some(newest) = self.messages.last().map(|m| m.id.clone()) else {
            return;
        };
        if self.read_upto.as_deref() == Some(newest.as_str()) {
            return;
        }
        self.read_upto = Some(newest.clone());
        if let Some(chat) = &mut self.chat {
            chat.unread = 0;
        }
        cx.emit(ChatViewEvent::Read(self.chat_id.clone()));
        let client = self.ctx.client();
        let org_id = self.org_id();
        let chat_id = self.chat_id.clone();
        self._tasks.push(cx.background_spawn(async move {
            let _ = client.mark_chat_read(&org_id, &chat_id, Some(&newest));
        }));
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
        let chat_id = self.chat_id.clone();
        self.error = None;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { call(&client, &org_id, &chat_id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(e) if e.is_auth() => cx.emit(ChatViewEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    // -------------------------------------------------------------- sending

    fn on_composer_event(
        &mut self,
        input: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { shift: false, .. } = event {
            let body = input.read(cx).value().trim().to_owned();
            self.send(body, window, cx);
        }
    }

    fn send(&mut self, body: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending || self.uploading.is_some() {
            return;
        }
        // Files with no words are a message too — the server takes an empty
        // body exactly when something is attached. An edit still needs words.
        if body.is_empty() && (self.staged.is_empty() || self.editing.is_some()) {
            return;
        }
        self.composer
            .update(cx, |c, cx| c.set_value("", window, cx));
        self.sending = true;
        let restore = body.clone();
        let editing = self.editing.take();
        let files: Vec<String> = self.staged.drain(..).map(|s| s.id).collect();
        let reply = self.reply_to.take().map(|m| m.id);
        let call = move |c: &ApiClient, org: &str, chat: &str| match &editing {
            Some(id) => c.edit_message(org, id, &body),
            None => c.send_chat_message(org, chat, &body, &files, reply.as_deref()),
        };
        self.request(window, cx, call, |this, message, _, cx| {
            this.sending = false;
            match this.messages.iter_mut().find(|m| m.id == message.id) {
                // An edit keeps what the edit response leaves out.
                Some(old) => {
                    let attachments = std::mem::take(&mut old.attachments);
                    *old = Message {
                        attachments: if message.attachments.is_empty() {
                            attachments
                        } else {
                            message.attachments.clone()
                        },
                        ..message
                    };
                }
                None => {
                    this.messages.push(message);
                    this.to_bottom = true;
                }
            }
            this.fetch_thumbnails(cx);
            this.mark_read(cx);
            cx.emit(ChatViewEvent::Changed);
        });
        // If it fails, the text is still the only copy: put it back.
        let composer = self.composer.clone();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let state = this
                    .update(cx, |this, _| (this.sending, this.error.clone()))
                    .ok();
                match state {
                    Some((true, None)) => continue,
                    Some((_, Some(_))) => {
                        this.update(cx, |this, _| this.sending = false).ok();
                        composer
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

    fn start_reply(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.reply_to = Some(message);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn start_edit(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        self.reply_to = None;
        self.editing = Some(message.id.clone());
        let text = html_to_markdown(&message.body);
        self.composer.update(cx, |c, cx| {
            c.set_value(text, window, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    fn cancel_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.composer
                .update(cx, |c, cx| c.set_value("", window, cx));
        }
        self.reply_to = None;
        cx.notify();
    }

    fn confirm_delete(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let id = message.id.clone();
            alert
                .title("Delete this message?")
                .description("Everyone in the chat will see that a message was deleted.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let id = id.clone();
                    this.update(cx, |this, cx| {
                        let target = id.clone();
                        this.request(
                            window,
                            cx,
                            move |c, org, _| c.delete_message(org, &target),
                            move |this, _, _, cx| {
                                if let Some(m) = this.messages.iter_mut().find(|m| m.id == id) {
                                    m.deleted = true;
                                    m.body.clear();
                                    m.attachments.clear();
                                }
                                cx.emit(ChatViewEvent::Changed);
                            },
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| this.upload(paths, cx)).ok();
            }
        }));
    }

    /// The web app's three steps, one file after another: a ticket, the
    /// bytes straight to storage, confirm. Each lands in `staged` until the
    /// message goes.
    fn upload(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        let client = self.ctx.client();
        let org_id = self.org_id();
        let chat_id = self.chat_id.clone();
        self.uploading = paths.first().map(|p| file_name(p));
        cx.notify();
        self._tasks.push(cx.spawn(async move |this, cx| {
            for path in paths.into_iter().filter(|p| p.is_file()) {
                let name = file_name(&path);
                this.update(cx, |this, cx| {
                    this.uploading = Some(name.clone());
                    cx.notify();
                })
                .ok();
                let (client, org_id, chat_id) = (client.clone(), org_id.clone(), chat_id.clone());
                let staged_name = name.clone();
                let result = cx
                    .background_spawn(async move {
                        let content_type = content_type_for(&path).ok_or_else(|| {
                            "this kind of file can't be attached — images, PDF, text, CSV, \
                             video and audio can"
                                .to_owned()
                        })?;
                        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                        if size > MAX_BYTES {
                            return Err(format!("it's over the {} MB limit", MAX_BYTES >> 20));
                        }
                        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                        let ticket = client
                            .stage_chat_attachment(&org_id, &chat_id, &staged_name, content_type)
                            .map_err(|e| e.to_string())?;
                        // The signed type, not our guess.
                        client
                            .put_to_storage(&ticket.upload_url, &ticket.content_type, &bytes)
                            .map_err(|e| e.to_string())?;
                        client
                            .confirm_attachment(&org_id, &ticket.attachment.id)
                            .map_err(|e| e.to_string())?;
                        Ok(ticket.attachment.id)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(id) => this.staged.push(Staged { id, name }),
                        Err(e) => this.error = Some(format!("{name} didn't upload: {e}")),
                    }
                    cx.notify();
                })
                .ok();
            }
            this.update(cx, |this, cx| {
                this.uploading = None;
                cx.notify();
            })
            .ok();
        }));
    }

    // ------------------------------------------------------- the chat itself

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
        let title = input.read(cx).value().trim().to_owned();
        let Some(chat) = &self.chat else { return };
        if title.is_empty() || title == chat.title {
            return;
        }
        self.request(
            window,
            cx,
            move |c, org, chat| c.rename_chat(org, chat, &title),
            |this, chat, window, cx| {
                this.set_chat(chat, window, cx);
                cx.emit(ChatViewEvent::Changed);
            },
        );
    }

    fn set_state(
        &mut self,
        fields: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request(
            window,
            cx,
            move |c, org, chat| {
                c.set_chat_state(org, chat, fields)?;
                c.chat(org, chat)
            },
            |this, chat, window, cx| {
                this.set_chat(chat, window, cx);
                cx.emit(ChatViewEvent::Changed);
            },
        );
    }

    fn add_member(&mut self, user_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.request(
            window,
            cx,
            move |c, org, chat| c.add_chat_members(org, chat, &[user_id]),
            |this, chat, window, cx| {
                this.set_chat(chat, window, cx);
                cx.emit(ChatViewEvent::Changed);
            },
        );
    }

    fn confirm_leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(me) = self.ctx.me().map(str::to_owned) else {
            return;
        };
        let name = self
            .chat
            .as_ref()
            .map(|c| c.title.clone())
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let me = me.clone();
            alert
                .title(format!("Leave “{name}”?"))
                .description(
                    "It leaves your list. What you said stays; someone in it can add you back.",
                )
                .confirm()
                .ok_text("Leave")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let me = me.clone();
                    this.update(cx, |this, cx| {
                        this.request(
                            window,
                            cx,
                            move |c, org, chat| c.remove_chat_member(org, chat, &me),
                            |_, _, _, cx| {
                                cx.emit(ChatViewEvent::Left);
                                cx.emit(ItemEvent::CloseRequested);
                            },
                        )
                    })
                    .ok();
                    true
                })
        });
    }

    // --------------------------------------------------------------- render

    fn render_header(&self, chat: &Chat, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let me = self.ctx.me().map(str::to_owned);
        let subtitle = match chat.kind.as_str() {
            "direct" => match chat.other(me.as_deref()) {
                Some(other) => seen_line(other.online, other.last_seen_at),
                None => "notes to yourself".to_owned(),
            },
            "group" => {
                let online = chat.members.iter().filter(|m| m.online).count();
                let mut line = format!("{} members", chat.members.len());
                if online > 0 {
                    line.push_str(&format!(", {online} online"));
                }
                line
            }
            "task" => match &chat.task {
                Some(task) => format!(
                    "Task · {} · {}{}",
                    status_label(&task.status),
                    priority_label(&task.priority),
                    if task.closed { " · closed" } else { "" }
                ),
                None => "Task".to_owned(),
            },
            _ => "Project".to_owned(),
        };
        let online = chat.other(me.as_deref()).is_some_and(|other| other.online);

        let open: Option<(&'static str, Lucide, Open)> = match chat.kind.as_str() {
            "task" => chat.task.as_ref().map(|t| {
                (
                    "Open task",
                    Lucide::SquareCheck,
                    Open::Task {
                        id: t.id.clone(),
                        org_id: None,
                        preview: false,
                    },
                )
            }),
            "project" => chat.project_id.as_ref().map(|id| {
                (
                    "Open project",
                    Lucide::FolderKanban,
                    Open::Project {
                        id: id.clone(),
                        preview: false,
                    },
                )
            }),
            "direct" => chat.other(me.as_deref()).map(|other| {
                (
                    "Profile",
                    Lucide::Users,
                    Open::Person {
                        id: other.id.clone(),
                        org_id: None,
                        preview: false,
                    },
                )
            }),
            _ => None,
        };

        let outsiders: Vec<(String, String)> = if chat.kind == "group" {
            self.ctx
                .directory
                .read(cx)
                .members
                .iter()
                .filter_map(|m| {
                    let id = m.user_id.clone()?;
                    (!chat.members.iter().any(|c| c.id == id)).then(|| (id, m.name()))
                })
                .collect()
        } else {
            Vec::new()
        };

        let this = cx.entity().downgrade();
        let (pinned, muted, archived) = (chat.pinned, chat.muted, chat.archived);
        let is_group = chat.kind == "group";
        let more = Button::new("chat-more")
            .icon(IconName::Ellipsis)
            .ghost()
            .small()
            .dropdown_menu(move |menu, _, _| {
                let item = |label: &'static str, fields: serde_json::Value| {
                    let this = this.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let fields = fields.clone();
                        this.update(cx, |this, cx| this.set_state(fields, window, cx))
                            .ok();
                    })
                };
                let mut menu = menu
                    .item(item(
                        if pinned { "Unpin" } else { "Pin to the top" },
                        json!({ "pinned": !pinned }),
                    ))
                    .item(item(
                        if muted { "Unmute" } else { "Mute" },
                        json!({ "muted": !muted }),
                    ))
                    .item(item(
                        if archived { "Unarchive" } else { "Archive" },
                        json!({ "archived": !archived }),
                    ));
                if is_group {
                    let this = this.clone();
                    menu = menu
                        .separator()
                        .item(
                            PopupMenuItem::new("Leave group…").on_click(move |_, window, cx| {
                                this.update(cx, |this, cx| this.confirm_leave(window, cx))
                                    .ok();
                            }),
                        );
                }
                menu
            });

        let add = (!outsiders.is_empty()).then(|| {
            let this = cx.entity().downgrade();
            Button::new("chat-add")
                .icon(Lucide::UserPlus)
                .label("Add people")
                .ghost()
                .small()
                .dropdown_menu(move |mut menu, _, _| {
                    for (id, name) in &outsiders {
                        let this = this.clone();
                        let id = id.clone();
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                            move |_, window, cx| {
                                this.update(cx, |this, cx| this.add_member(id.clone(), window, cx))
                                    .ok();
                            },
                        ));
                    }
                    menu.max_h(px(360.)).scrollable(true)
                })
        });

        h_flex()
            .flex_none()
            .gap_3()
            .px_4()
            .h(px(56.))
            .border_b_1()
            .border_color(theme.border)
            .child(chat_avatar(chat, me.as_deref(), px(36.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(if is_group {
                        Input::new(&self.title)
                            .appearance(false)
                            .px_0()
                            .h(px(22.))
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .into_any_element()
                    } else {
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chat.title.clone())
                            .into_any_element()
                    })
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(if online {
                                theme.green
                            } else {
                                theme.muted_foreground
                            })
                            .child(subtitle),
                    ),
            )
            .when(self.sending || self.loading_older, |this| {
                this.child(Spinner::new().small())
            })
            .when_some(add, |this, add| this.child(add))
            .when_some(open, |this, (label, icon, open)| {
                this.child(
                    Button::new("chat-open")
                        .icon(icon)
                        .label(label)
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(open.clone()))),
                )
            })
            .child(more)
    }

    fn render_messages(&mut self, chat: &Chat, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let names = chat.kind != "direct";
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.has_more {
            rows.push(
                h_flex()
                    .justify_center()
                    .py_2()
                    .child(
                        Button::new("chat-older")
                            .label(if self.loading_older {
                                "Loading…"
                            } else {
                                "Earlier messages"
                            })
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.load_older(cx))),
                    )
                    .into_any_element(),
            );
        }
        let mut anchor_ix = None;
        let mut last_day = None;
        let mut prev: Option<&Message> = None;
        for m in &self.messages {
            let at = m.created_at.unwrap_or_else(Utc::now);
            let day = at.with_timezone(&Local).date_naive();
            if last_day != Some(day) {
                last_day = Some(day);
                prev = None;
                rows.push(day_separator(at, &theme));
            }
            let starts_run = prev.is_none_or(|p| {
                p.mine != m.mine
                    || p.author.as_ref().map(|a| &a.id) != m.author.as_ref().map(|a| &a.id)
                    || gap(p.created_at, m.created_at) > RUN_GAP
            });
            if self.anchor.as_deref() == Some(m.id.as_str()) {
                anchor_ix = Some(rows.len());
            }
            rows.push(self.render_message(m, starts_run && names && !m.mine, starts_run, cx));
            prev = Some(m);
        }
        if let Some(ix) = anchor_ix {
            self.scroll.scroll_to_top_of_item(ix);
            self.anchor = None;
        } else if self.to_bottom {
            self.scroll.scroll_to_bottom();
            self.to_bottom = false;
        }
        let empty = self.messages.is_empty();
        div()
            .id("chat-messages")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                if this.has_more && this.scroll.offset().y > -EDGE {
                    this.load_older(cx);
                }
            }))
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .children(rows)
            .when(empty, |this| {
                this.child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(
                            Icon::new(Lucide::MessageCircle)
                                .large()
                                .text_color(theme.muted_foreground.opacity(0.5)),
                        )
                        .child("No messages yet — say hello."),
                )
            })
            .into_any_element()
    }

    /// One bubble. Yours sit on the right in blue; theirs on the left, with
    /// a name and face at the start of a run in a group or thread.
    fn render_message(
        &self,
        m: &Message,
        show_name: bool,
        starts_run: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let name = m
            .author
            .as_ref()
            .map(Person::name)
            .unwrap_or_else(|| "ayeaye".into());
        let (bubble, edge) = if m.mine {
            (theme.blue.opacity(0.14), theme.blue.opacity(0.30))
        } else {
            (theme.secondary, theme.border)
        };
        let time = m
            .created_at
            .map(|at| at.with_timezone(&Local).format("%H:%M").to_string())
            .unwrap_or_default();
        let group = SharedString::from(format!("bubble-{}", m.id));
        let this = cx.entity().downgrade();
        let menu_message = m.clone();
        let reply_message = m.clone();
        let deleted = m.deleted;

        let body = if deleted {
            div()
                .italic()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Message deleted")
                .into_any_element()
        } else if m.body.is_empty() {
            // Only files: the chips below are the whole message.
            div().into_any_element()
        } else {
            div()
                .text_size(px(14.))
                .line_height(px(21.))
                .child(rich_text(
                    SharedString::from(format!("chat-msg-{}", m.id)),
                    &m.body,
                ))
                .into_any_element()
        };

        let bubble_el = v_flex()
            .max_w(px(560.))
            .min_w(px(80.))
            .px_3()
            .py_1p5()
            .gap_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(edge)
            .bg(bubble)
            .when(show_name, |this| {
                this.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.blue)
                        .child(name.clone()),
                )
            })
            .when_some(m.reply_to.clone().filter(|_| !deleted), |this, quoted| {
                this.child(
                    v_flex()
                        .pl_2()
                        .border_l_2()
                        .border_color(theme.blue)
                        .text_xs()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.blue)
                                .child(
                                    quoted.author.as_ref().map(Person::name).unwrap_or_default(),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .when(quoted.deleted, |d| d.italic())
                                .child(if quoted.deleted {
                                    "Message deleted".to_owned()
                                } else if quoted.body.is_empty() {
                                    "File".to_owned()
                                } else {
                                    plain_line(&quoted.body)
                                }),
                        ),
                )
            })
            .child(body)
            .when(!deleted && !m.attachments.is_empty(), |this| {
                this.child(v_flex().gap_1().children(m.attachments.iter().map(|f| {
                    match self.thumbnails.get(&f.id) {
                        Some(image) => picture(f, image.clone(), &theme),
                        None => attachment_chip(f, &theme),
                    }
                })))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when(m.edited_at.is_some() && !deleted, |this| {
                        this.child("edited")
                    })
                    .when_some(m.via.clone(), |this, via| this.child(format!("via {via}")))
                    .child(time),
            );

        let reply_button = (!deleted).then(|| {
            div()
                .invisible()
                .group_hover(group.clone(), |s| s.visible())
                .child(
                    Button::new(SharedString::from(format!("reply-{}", m.id)))
                        .icon(Lucide::Reply)
                        .ghost()
                        .xsmall()
                        .tooltip("Reply")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_reply(reply_message.clone(), window, cx)
                        })),
                )
        });

        let mine = m.mine;
        h_flex()
            .id(SharedString::from(format!("chat-row-{}", m.id)))
            .group(group)
            .w_full()
            .items_end()
            .gap_2()
            .when(starts_run, |this| this.pt_2())
            .when(!starts_run, |this| this.pt_0p5())
            .when(mine, |this| this.justify_end())
            .when(!mine, |this| {
                this.child(if show_name {
                    avatar(&name, px(28.), &theme).into_any_element()
                } else {
                    div().w(px(28.)).flex_none().into_any_element()
                })
            })
            .map(|this| {
                if mine {
                    this.children(reply_button).child(bubble_el)
                } else {
                    this.child(bubble_el).children(reply_button)
                }
            })
            .context_menu(move |menu, _, _| {
                if menu_message.deleted {
                    return menu;
                }
                let item = |label: &'static str,
                            act: fn(
                    &mut ChatView,
                    Message,
                    &mut Window,
                    &mut Context<ChatView>,
                )| {
                    let this = this.clone();
                    let message = menu_message.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let message = message.clone();
                        this.update(cx, |this, cx| act(this, message, window, cx))
                            .ok();
                    })
                };
                let text = plain_line(&menu_message.body);
                let mut menu = menu
                    .item(item("Reply", |this, m, window, cx| {
                        this.start_reply(m, window, cx)
                    }))
                    .item(PopupMenuItem::new("Copy text").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                    }));
                if menu_message.mine {
                    menu = menu
                        .separator()
                        .item(item("Edit", |this, m, window, cx| {
                            this.start_edit(m, window, cx)
                        }))
                        .item(item("Delete…", |this, m, window, cx| {
                            this.confirm_delete(m, window, cx)
                        }));
                }
                menu
            })
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let strip = match (&self.editing, &self.reply_to) {
            (Some(_), _) => Some((Lucide::Pencil, "Editing".to_owned(), String::new())),
            (None, Some(m)) => Some((
                Lucide::Reply,
                format!(
                    "Reply to {}",
                    m.author.as_ref().map(Person::name).unwrap_or_default()
                ),
                plain_line(&m.body),
            )),
            _ => None,
        };
        v_flex()
            .flex_none()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .when_some(strip, |this, (icon, label, quote)| {
                this.child(
                    h_flex()
                        .gap_2()
                        .pl_2()
                        .border_l_2()
                        .border_color(theme.blue)
                        .text_xs()
                        .child(Icon::new(icon).xsmall().text_color(theme.blue))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.blue)
                                        .child(label),
                                )
                                .when(!quote.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .truncate()
                                            .text_color(theme.muted_foreground)
                                            .child(quote),
                                    )
                                }),
                        )
                        .child(
                            Button::new("compose-cancel")
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_compose(window, cx)
                                })),
                        ),
                )
            })
            .when(
                !self.staged.is_empty() || self.uploading.is_some(),
                |this| {
                    this.child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .children(self.staged.iter().cloned().map(|s| {
                                let id = s.id.clone();
                                h_flex()
                                    .gap_1()
                                    .pl_2()
                                    .rounded(theme.radius)
                                    .bg(theme.muted)
                                    .text_xs()
                                    .child(Icon::new(Lucide::Paperclip).xsmall())
                                    .child(s.name)
                                    .child(
                                        Button::new(SharedString::from(format!("unstage-{id}")))
                                            .icon(IconName::Close)
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.staged.retain(|s| s.id != id);
                                                cx.notify();
                                            })),
                                    )
                            }))
                            .when_some(self.uploading.clone(), |this, name| {
                                this.child(
                                    h_flex()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(Spinner::new().xsmall())
                                        .child(format!("Uploading {name}…")),
                                )
                            }),
                    )
                },
            )
            .child(
                h_flex()
                    .items_end()
                    .gap_1()
                    .child(
                        Button::new("chat-attach")
                            .icon(Lucide::Paperclip)
                            .ghost()
                            .small()
                            .tooltip("Attach files")
                            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&self.composer)),
                    )
                    .child(
                        Button::new("chat-send")
                            .icon(if self.editing.is_some() {
                                IconName::Check
                            } else {
                                IconName::ArrowUp
                            })
                            .primary()
                            .small()
                            .tooltip(if self.editing.is_some() {
                                "Save (Return)"
                            } else {
                                "Send (Return; Shift-Return for a new line)"
                            })
                            .disabled(self.uploading.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let body = this.composer.read(cx).value().trim().to_owned();
                                this.send(body, window, cx);
                            })),
                    ),
            )
    }
}

fn gap(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> i64 {
    match (a, b) {
        (Some(a), Some(b)) => (b - a).num_seconds().abs(),
        _ => 0,
    }
}

fn day_separator(at: DateTime<Utc>, theme: &Theme) -> AnyElement {
    h_flex()
        .justify_center()
        .py_2()
        .child(
            div()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(theme.muted)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(day_label(at)),
        )
        .into_any_element()
}

/// A picture in a message, once its thumbnail is in: shown, at most a
/// thumbnail's size, and opened full-size in the browser on click.
fn picture(file: &FileItem, image: Arc<Image>, theme: &Theme) -> AnyElement {
    let url = file.url.clone();
    div()
        .id(SharedString::from(format!("chat-picture-{}", file.id)))
        .w(px(240.))
        .h(px(180.))
        .overflow_hidden()
        .rounded(theme.radius)
        .bg(theme.muted)
        .cursor_pointer()
        .child(img(image).size_full().object_fit(ObjectFit::Cover))
        .on_click(move |_, _, cx| cx.open_url(&url))
        .into_any_element()
}

/// A file in a message: its name and size; clicking opens it — the URL is
/// presigned and fresh from this read.
fn attachment_chip(file: &FileItem, theme: &Theme) -> AnyElement {
    let url = file.url.clone();
    h_flex()
        .id(SharedString::from(format!("chat-file-{}", file.id)))
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .bg(theme.background.opacity(0.6))
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .cursor_pointer()
        .hover(|s| s.bg(theme.list_hover))
        .child(Icon::new(Lucide::Paperclip).xsmall())
        .child(div().min_w_0().truncate().child(file.filename.clone()))
        .when_some(file.size_bytes, |this, size| {
            this.child(
                div()
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .child(format_bytes(size)),
            )
        })
        .on_click(move |_, _, cx| cx.open_url(&url))
        .into_any_element()
}

/// Opening a chat is for writing in it: the tab's focus is the box.
impl Focusable for ChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

impl Item for ChatView {
    fn item_key(&self, _: &App) -> SharedString {
        format!("chat:{}", self.chat_id).into()
    }

    fn tab_title(&self, _: &App) -> SharedString {
        self.chat
            .as_ref()
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "Chat".into())
            .into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::MessageCircle)
    }

    fn is_dirty(&self, _: &App) -> bool {
        false
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        let chat = self.chat.as_ref()?;
        let mut line = match chat.kind.as_str() {
            "direct" => "Direct message".to_owned(),
            "group" => "Group".to_owned(),
            "task" => "Task comments".to_owned(),
            _ => "Project comments".to_owned(),
        };
        if chat.muted {
            line.push_str(" · muted");
        }
        if chat.pinned {
            line.push_str(" · pinned");
        }
        Some(line.into())
    }
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content = match (self.chat.clone(), &self.load_error) {
            (Some(chat), _) => v_flex()
                .size_full()
                .child(self.render_header(&chat, cx))
                .when_some(self.error.clone(), |this, e| {
                    this.child(
                        div()
                            .mx_4()
                            .mt_2()
                            .px_3()
                            .py_2()
                            .rounded(theme.radius)
                            .bg(theme.danger.opacity(0.12))
                            .text_sm()
                            .child(e),
                    )
                })
                .child(self.render_messages(&chat, cx))
                .when(chat.can_post, |this| this.child(self.render_composer(cx)))
                .into_any_element(),
            (None, Some(e)) => v_flex()
                .p_6()
                .gap_2()
                .child(div().text_lg().child("Couldn't open this chat"))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(e.clone()),
                )
                .into_any_element(),
            (None, None) => h_flex()
                .p_6()
                .gap_2()
                .child(Spinner::new())
                .child("Loading…")
                .into_any_element(),
        };
        v_flex()
            .id("chat-view")
            .size_full()
            .bg(theme.background)
            .child(content)
    }
}
