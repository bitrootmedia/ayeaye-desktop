//! Live updates for one profile, over `WS /api/ws`.
//!
//! Events never carry content — "task X moved", "a comment on Y", "your
//! planner changed" — so all this does is collect them for 1.5 s (a burst
//! of edits should cost one refetch, not ten, as on the web) and tell the
//! workspace which things to refetch over HTTP. A refetch that 404s is how
//! revoked access shows up, through the same path as everything else.
//!
//! **Two sockets**, because the server keeps one watch per socket and the
//! window shows two kinds of thing at once:
//!
//! * the *board* socket watches `org:<id>` — every task change in the
//!   organisation, which keeps the lists and any open task tab current;
//! * the *thread* socket watches `task:<id>` (or `project:<id>`, or
//!   `chat:<id>`) for the task, project or chat in the front tab, because a
//!   comment only goes to people with a stake in it and to whoever has it
//!   open — not to the organisation.
//!
//! Both also receive what's addressed to the person (comments where they
//! have a stake, every message in their own chats, their planner), so
//! nothing needs a third.
//!
//! The socket signs in with a personal access token minted after sign-in
//! (see `store::vault::SocketToken`); a header-mode client has no cookie,
//! and its session token isn't accepted there. Realtime is a nicety: if the
//! token can't be minted or the socket won't connect, everything still works
//! by ⌘R, window focus and the minute polls.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::*;
use serde_json::Value;

use crate::api::ApiClient;
use crate::api::socket::{Signal, Socket, socket_url};
use crate::store::vault::{self, SocketToken};

const COALESCE: Duration = Duration::from_millis(1500);

/// What changed since the last batch, by id. Branches on the event's `type`
/// only to know *where* to look — never on `change`, which is for the logs
/// and gains new values without notice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nudges {
    pub tasks: HashSet<String>,
    /// Projects with a new comment.
    pub projects: HashSet<String>,
    pub orgs: HashSet<String>,
    /// Conversations that moved: a message, an edit, a chat renamed.
    pub chats: HashSet<String>,
    /// Organisations whose chat list moved. Kept apart from `orgs`, which
    /// refetches the task lists: a DM says nothing about tasks.
    pub chat_orgs: HashSet<String>,
    pub planner: bool,
    pub messages: bool,
    /// The board socket came back after a drop: anything could have been
    /// missed meanwhile, so whatever is on screen is refetched.
    pub reconnected: bool,
}

impl Nudges {
    pub fn absorb(&mut self, event: &Value) {
        let text = |v: &Value| v.as_str().map(str::to_owned);
        let kind = event.get("type").and_then(Value::as_str);
        match kind {
            Some("planner") => self.planner = true,
            Some("message") => {
                self.messages = true;
                if let Some(id) = text(&event["conversation_id"]) {
                    self.chats.insert(id);
                }
                if let Some(id) = text(&event["organisation_id"]) {
                    self.chat_orgs.insert(id);
                }
                if let Some(id) = text(&event["anchor"]["id"]) {
                    match event["anchor"]["kind"].as_str() {
                        Some("task") => _ = self.tasks.insert(id),
                        Some("project") => _ = self.projects.insert(id),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if let Some(id) = text(&event["task_id"]) {
            self.tasks.insert(id);
        }
        if kind != Some("message")
            && let Some(id) = text(&event["organisation_id"])
        {
            self.orgs.insert(id);
        }
    }

    fn is_empty(&self) -> bool {
        *self == Nudges::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    /// No socket token (yet): never connected.
    Off,
    Connecting,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Board,
    Thread,
}

impl EventEmitter<Nudges> for Realtime {}

pub struct Realtime {
    profile_id: String,
    url: String,
    client: Arc<ApiClient>,
    token: Option<SocketToken>,
    board: Option<Socket>,
    thread: Option<Socket>,
    board_watch: Option<String>,
    thread_watch: Option<String>,
    state: LiveState,
    was_live: bool,
    /// A refused token is minted again once per session, not in a loop.
    reminted: bool,
    /// Bumped whenever the sockets are replaced, so a late signal from an
    /// old one is ignored.
    generation: u64,
    signals: mpsc::UnboundedSender<(Lane, u64, Signal)>,
    pending: Nudges,
    _pump: Task<()>,
    _flush: Option<Task<()>>,
    _token: Option<Task<()>>,
}

impl Realtime {
    pub fn new(
        profile_id: String,
        base_url: &str,
        client: Arc<ApiClient>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (signals, mut rx) = mpsc::unbounded::<(Lane, u64, Signal)>();
        let pump = cx.spawn(async move |this, cx| {
            while let Some((lane, generation, signal)) = rx.next().await {
                if this
                    .update(cx, |this, cx| this.on_signal(lane, generation, signal, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut realtime = Self {
            profile_id,
            url: socket_url(base_url),
            client,
            token: None,
            board: None,
            thread: None,
            board_watch: None,
            thread_watch: None,
            state: LiveState::Off,
            was_live: false,
            reminted: false,
            generation: 0,
            signals,
            pending: Nudges::default(),
            _pump: pump,
            _flush: None,
            _token: None,
        };
        realtime.ensure_started(cx);
        realtime
    }

    pub fn state(&self) -> LiveState {
        self.state
    }

    /// Connects if it isn't already — on start, and again on ⌘R in case the
    /// token couldn't be minted the first time.
    pub fn ensure_started(&mut self, cx: &mut Context<Self>) {
        if self.board.is_some() || self._token.is_some() {
            return;
        }
        let profile_id = self.profile_id.clone();
        let client = self.client.clone();
        self._token = Some(cx.spawn(async move |this, cx| {
            let token = cx
                .background_spawn(async move {
                    if let Some(token) = vault::load_socket(&profile_id) {
                        return Some(token);
                    }
                    match client.create_token("Desktop app", "read") {
                        Ok(created) => {
                            let token = SocketToken {
                                id: created.id,
                                token: created.token,
                            };
                            vault::save_socket(&profile_id, &token);
                            Some(token)
                        }
                        Err(e) => {
                            log::warn!("no socket token, so no live updates: {e}");
                            None
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this._token = None;
                if let Some(token) = token {
                    this.token = Some(token);
                    this.connect(cx);
                }
            })
            .ok();
        }));
    }

    /// The organisation on screen; `None` leaves the old watch in place
    /// (there is no unwatch, and its events are harmless).
    pub fn watch_board(&mut self, key: Option<String>) {
        if key.is_some() && key != self.board_watch {
            self.board_watch = key.clone();
            if let Some(socket) = &self.board {
                socket.watch(key);
            }
        }
    }

    /// The task in the front tab. The thread socket only opens the first
    /// time there is one.
    pub fn watch_thread(&mut self, key: Option<String>) {
        if key.is_none() || key == self.thread_watch {
            return;
        }
        self.thread_watch = key.clone();
        if self.thread.is_none() && self.token.is_some() {
            self.thread = Some(self.spawn(Lane::Thread));
        }
        if let Some(socket) = &self.thread {
            socket.watch(key);
        }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.state = LiveState::Connecting;
        let board = self.spawn(Lane::Board);
        board.watch(self.board_watch.clone());
        self.board = Some(board);
        self.thread = self.thread_watch.clone().map(|key| {
            let socket = self.spawn(Lane::Thread);
            socket.watch(Some(key));
            socket
        });
        cx.notify();
    }

    fn spawn(&self, lane: Lane) -> Socket {
        let token = self
            .token
            .as_ref()
            .map(|t| t.token.clone())
            .unwrap_or_default();
        let signals = self.signals.clone();
        let generation = self.generation;
        Socket::spawn(self.url.clone(), token, move |signal| {
            signals.unbounded_send((lane, generation, signal)).is_ok()
        })
    }

    fn on_signal(&mut self, lane: Lane, generation: u64, signal: Signal, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        match signal {
            Signal::Connected if lane == Lane::Board => {
                if self.was_live {
                    self.pending.reconnected = true;
                    self.schedule_flush(cx);
                }
                self.was_live = true;
                self.state = LiveState::Live;
                cx.notify();
            }
            Signal::Disconnected if lane == Lane::Board => {
                self.state = LiveState::Connecting;
                cx.notify();
            }
            Signal::Connected | Signal::Disconnected => {}
            Signal::Unauthorized => {
                // Revoked from the web's token list, most likely. Mint a new
                // one, once; if that's refused too, stay off until ⌘R.
                self.board = None;
                self.thread = None;
                self.token = None;
                self.generation += 1;
                self.state = LiveState::Off;
                let profile_id = self.profile_id.clone();
                cx.background_spawn(async move { vault::forget_socket(&profile_id) })
                    .detach();
                if !self.reminted {
                    self.reminted = true;
                    self.ensure_started(cx);
                }
                cx.notify();
            }
            Signal::Event(event) => {
                self.pending.absorb(&event);
                self.schedule_flush(cx);
            }
        }
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        if self._flush.is_some() {
            return;
        }
        self._flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COALESCE).await;
            this.update(cx, |this, cx| {
                this._flush = None;
                let nudges = std::mem::take(&mut this.pending);
                if !nudges.is_empty() {
                    cx.emit(nudges);
                }
            })
            .ok();
        }));
    }
}

/// Signing out: the token goes with the session, revoked while the session
/// can still authorise that.
pub fn revoke_socket_token(profile_id: &str, client: &ApiClient) {
    if let Some(token) = vault::load_socket(profile_id) {
        let _ = client.revoke_token(&token.id);
    }
    vault::forget_socket(profile_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn nudges_collect_what_to_refetch() {
        let mut n = Nudges::default();
        n.absorb(
            &json!({"type": "task", "task_id": "t1", "organisation_id": "o1", "change": "status"}),
        );
        n.absorb(&json!({"type": "message", "conversation_id": "c", "anchor": {"kind": "task", "id": "t2"}}));
        n.absorb(
            &json!({"type": "planner", "user_id": "u", "organisation_id": "o1", "task_id": "t3"}),
        );
        n.absorb(&json!({"type": "something_new", "task_id": "t4"}));
        assert_eq!(
            n.tasks,
            ["t1", "t2", "t3", "t4"]
                .map(String::from)
                .into_iter()
                .collect()
        );
        assert!(n.orgs.contains("o1") && n.planner && n.messages && !n.reconnected);

        let mut chat = Nudges::default();
        chat.absorb(&json!({"type": "message", "conversation_id": "c1", "organisation_id": "o1", "anchor": {"kind": "chat", "id": "c1"}}));
        assert!(chat.chats.contains("c1") && chat.chat_orgs.contains("o1"));
        assert!(chat.orgs.is_empty() && chat.tasks.is_empty());

        let mut project = Nudges::default();
        project.absorb(&json!({"type": "message", "anchor": {"kind": "project", "id": "p"}}));
        assert!(project.tasks.is_empty() && project.messages);
        assert!(project.projects.contains("p"));
    }
}
