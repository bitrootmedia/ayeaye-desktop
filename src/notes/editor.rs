//! A note open in a tab: a title line and a plain-text body, autosaved.
//!
//! **No Save button, and nothing is ever dropped.** The same trade the web
//! notepad made: edits are debounced into a PATCH of only the fields that
//! changed, a failed save keeps its fields pending and retries, and closing
//! the tab or quitting flushes whatever is still queued.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use super::store::{NotesEvent, NotesStore};
use crate::api::models::Note;
use crate::api::{ApiClient, ApiError, clamp_title};
use crate::ui::format::{ago_long, word_count};
use crate::ui::item::{Item, ItemEvent};

/// Long enough not to write on every keystroke, short enough that quitting
/// mid-sentence has almost nothing left to flush.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(700);
const RETRY_AFTER: Duration = Duration::from_secs(5);

pub const UNTITLED: &str = "Untitled note";

#[derive(Clone, Debug, Default, PartialEq)]
struct Pending {
    title: Option<String>,
    body: Option<String>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.body.is_none()
    }

    /// Put back fields a failed save took, without overwriting anything typed
    /// since — what's newer always wins.
    fn restore(&mut self, failed: Pending) {
        if self.title.is_none() {
            self.title = failed.title;
        }
        if self.body.is_none() {
            self.body = failed.body;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SyncState {
    /// Creating on the server; edits queue until it has an id.
    Creating,
    Saved(DateTime<Utc>),
    Dirty,
    Saving,
    Failed(String),
    /// The note was deleted (here, on the web, or by another device).
    Gone,
}

pub struct NoteEditor {
    store: Entity<NotesStore>,
    client: Arc<ApiClient>,
    org_id: String,
    note_id: Option<String>,
    /// Identity for a note that doesn't exist on the server yet.
    local_key: SharedString,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    /// What the server last confirmed, to tell our own echo from a change
    /// made elsewhere.
    synced_at: Option<DateTime<Utc>>,
    pending: Pending,
    sync: SyncState,
    in_flight: bool,
    debounce: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ItemEvent> for NoteEditor {}

impl NoteEditor {
    /// An existing note, from the store's list.
    pub fn open(
        store: Entity<NotesStore>,
        note: &Note,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self::build(
            store,
            Some(note.id.clone()),
            &note.title,
            &note.body,
            window,
            cx,
        );
        editor.synced_at = Some(note.updated_at);
        editor.sync = SyncState::Saved(note.updated_at);
        editor
    }

    /// A new note: the tab appears immediately and the POST runs behind it,
    /// so ⌘N is instant whatever the network is doing.
    pub fn create(
        store: Entity<NotesStore>,
        body: &str,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self::build(store, None, title, body, window, cx);
        editor.sync = SyncState::Creating;
        editor.start_create(cx);
        editor
    }

    fn build(
        store: Entity<NotesStore>,
        note_id: Option<String>,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(UNTITLED)
                .default_value(title.to_owned())
        });
        let body_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Start typing…")
                .soft_wrap(true)
                .searchable(true)
                .default_value(body.to_owned())
        });
        let subscriptions = vec![
            cx.subscribe_in(&title_state, window, Self::on_title_event),
            cx.subscribe_in(&body_state, window, Self::on_body_event),
            cx.subscribe_in(&store, window, Self::on_store_event),
        ];
        let (client, org_id) = {
            let store = store.read(cx);
            (store.client(), store.org_id().to_owned())
        };
        static NEXT_LOCAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let local = NEXT_LOCAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            store,
            client,
            org_id,
            note_id,
            local_key: format!("note-new:{local}").into(),
            title: title_state,
            body: body_state,
            synced_at: None,
            pending: Pending::default(),
            sync: SyncState::Dirty,
            in_flight: false,
            debounce: None,
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_title(&self, select_all: bool, window: &mut Window, cx: &mut App) {
        self.title.update(cx, |title, cx| {
            title.focus(window, cx);
            if select_all {
                title.select_all(window, cx);
            }
        });
    }

    fn title_text(&self, cx: &App) -> String {
        self.title.read(cx).value().to_string()
    }

    fn body_text(&self, cx: &App) -> String {
        self.body.read(cx).value().to_string()
    }

    // ---------------------------------------------------------------- edits

    fn on_title_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.pending.title = Some(self.title_text(cx));
                self.edited(cx);
            }
            // Return in the title goes to the body, like a heading would.
            InputEvent::PressEnter { .. } => {
                self.body.update(cx, |body, cx| body.focus(window, cx));
            }
            _ => {}
        }
    }

    fn on_body_event(
        &mut self,
        _: &Entity<TextareaState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.pending.body = Some(self.body_text(cx));
                self.edited(cx);
            }
            InputEvent::Blur => self.save_now(cx),
            _ => {}
        }
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.sync, SyncState::Creating | SyncState::Gone) {
            self.sync = SyncState::Dirty;
        }
        cx.emit(ItemEvent::Edited);
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| this.save(cx)).ok();
        }));
        cx.notify();
    }

    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        self.debounce = None;
        self.save(cx);
    }

    // ----------------------------------------------------------------- sync

    fn start_create(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let org_id = self.org_id.clone();
        let title = self.title_text(cx);
        let title = if clamp_title(&title).is_empty() {
            UNTITLED.to_owned()
        } else {
            title
        };
        let body = self.body_text(cx);
        self.in_flight = true;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.create_note(&org_id, &title, &body) })
                .await;
            this.update(cx, |this, cx| {
                this.in_flight = false;
                match result {
                    Ok(note) => {
                        this.note_id = Some(note.id.clone());
                        this.synced_at = Some(note.updated_at);
                        this.sync = SyncState::Saved(note.updated_at);
                        this.store.update(cx, |store, cx| store.upsert(note, cx));
                        // Typed while the POST was out: send it now.
                        this.save(cx);
                    }
                    Err(e) => this.failed(e, cx),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.in_flight || self.sync == SyncState::Gone {
            return; // the in-flight request reschedules when it lands
        }
        let Some(note_id) = self.note_id.clone() else {
            // A create that failed: try again with what's there now, which
            // includes everything typed since.
            self.pending = Pending::default();
            self.sync = SyncState::Creating;
            self.start_create(cx);
            return;
        };
        if self.pending.is_empty() {
            if self.sync == SyncState::Dirty {
                self.sync = self
                    .synced_at
                    .map(SyncState::Saved)
                    .unwrap_or(SyncState::Dirty);
                cx.notify();
            }
            return;
        }

        let fields = std::mem::take(&mut self.pending);
        self.in_flight = true;
        self.sync = SyncState::Saving;
        cx.notify();
        let client = self.client.clone();
        let org_id = self.org_id.clone();
        let sent = fields.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.update_note(
                        &org_id,
                        &note_id,
                        sent.title.as_deref(),
                        sent.body.as_deref(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.in_flight = false;
                match result {
                    Ok(note) => {
                        this.synced_at = Some(note.updated_at);
                        this.sync = SyncState::Saved(note.updated_at);
                        this.store.update(cx, |store, cx| store.upsert(note, cx));
                        if !this.pending.is_empty() {
                            this.sync = SyncState::Dirty;
                            this.save(cx);
                        }
                    }
                    Err(ApiError::NotFound(_)) => {
                        this.pending.restore(fields);
                        this.sync = SyncState::Gone;
                    }
                    Err(e) => {
                        this.pending.restore(fields);
                        this.failed(e, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(task);
    }

    fn failed(&mut self, error: ApiError, cx: &mut Context<Self>) {
        if error.is_auth() {
            let e = error.clone();
            self.store
                .update(cx, |store, cx| store.report_auth_failure(e, cx));
        }
        self.sync = SyncState::Failed(error.to_string());
        // Keep trying in the background; the next keystroke also retries.
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RETRY_AFTER).await;
            this.update(cx, |this, cx| this.save(cx)).ok();
        }));
    }

    /// "Deleted elsewhere" → keep the text as a new note instead.
    fn recreate(&mut self, cx: &mut Context<Self>) {
        self.note_id = None;
        self.pending = Pending::default();
        self.sync = SyncState::Creating;
        self.start_create(cx);
        cx.notify();
    }

    /// A fresh list from the server. Adopt a change made elsewhere when
    /// nothing local is waiting to be saved; never overwrite unsaved typing.
    fn on_store_event(
        &mut self,
        store: &Entity<NotesStore>,
        event: &NotesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let NotesEvent::Refreshed = event else { return };
        let Some(id) = self.note_id.clone() else {
            return;
        };
        let clean = self.pending.is_empty() && !self.in_flight;
        let Some(note) = store.read(cx).get(&id).cloned() else {
            if clean {
                cx.emit(ItemEvent::CloseRequested);
            } else {
                self.sync = SyncState::Gone;
                cx.notify();
            }
            return;
        };
        let newer = self.synced_at.is_none_or(|at| note.updated_at > at);
        if !(clean && newer) {
            return;
        }
        self.synced_at = Some(note.updated_at);
        self.sync = SyncState::Saved(note.updated_at);
        if self.title.read(cx).value().as_ref() != note.title {
            self.title
                .update(cx, |t, cx| t.set_value(note.title.clone(), window, cx));
        }
        if self.body.read(cx).value().as_ref() != note.body {
            self.body.update(cx, |b, cx| {
                let cursor = b.cursor_position();
                b.set_value(note.body.clone(), window, cx);
                b.set_cursor_position(cursor, window, cx);
            });
        }
        cx.notify();
    }

    // --------------------------------------------------------------- render

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        match &self.sync {
            SyncState::Gone => {
                Some(
                    h_flex()
                        .px_6()
                        .py_2()
                        .gap_3()
                        .bg(theme.warning.opacity(0.15))
                        .text_sm()
                        .child(div().flex_1().child(
                            "This note was deleted somewhere else. Your text is still here.",
                        ))
                        .child(
                            Button::new("recreate")
                                .label("Keep as a new note")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.recreate(cx))),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }
}

impl Focusable for NoteEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.body.read(cx).focus_handle(cx)
    }
}

impl Item for NoteEditor {
    fn item_key(&self, _: &App) -> SharedString {
        match &self.note_id {
            Some(id) => format!("note:{id}").into(),
            None => self.local_key.clone(),
        }
    }

    fn tab_title(&self, cx: &App) -> SharedString {
        let title = clamp_title(&self.title_text(cx));
        if title.is_empty() {
            UNTITLED.into()
        } else {
            title.into()
        }
    }

    fn tab_icon(&self) -> gpui_kit::component::Icon {
        IconName::FileText.into()
    }

    fn is_dirty(&self, _: &App) -> bool {
        !self.pending.is_empty()
            || matches!(
                self.sync,
                SyncState::Saving | SyncState::Creating | SyncState::Dirty
            )
    }

    fn status_text(&self, cx: &App) -> Option<SharedString> {
        let words = word_count(&self.body_text(cx));
        let state = match &self.sync {
            SyncState::Creating => "Creating…".to_owned(),
            SyncState::Saving => "Saving…".to_owned(),
            SyncState::Dirty => "Edited".to_owned(),
            SyncState::Saved(at) => format!("Saved {}", ago_long(*at)),
            SyncState::Failed(e) => format!("Not saved — {e}. Retrying…"),
            SyncState::Gone => "Deleted elsewhere".to_owned(),
        };
        Some(
            format!(
                "{state}  ·  {words} {}",
                if words == 1 { "word" } else { "words" }
            )
            .into(),
        )
    }

    fn cursor_position(&self, cx: &App) -> Option<(u32, u32)> {
        let p = self.body.read(cx).cursor_position();
        Some((p.line + 1, p.character + 1))
    }

    fn flush(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
    }

    fn take_blocking_save(&mut self, _: &mut Context<Self>) -> Option<Box<dyn FnOnce() + Send>> {
        self.debounce = None;
        self.blocking_save()
    }
}

impl Render for NoteEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let bg = theme.background;
        let banner = self.render_banner(cx);
        v_flex()
            .size_full()
            .bg(bg)
            .children(banner)
            .child(
                div().pt_5().px_3().child(
                    Input::new(&self.title)
                        .appearance(false)
                        .h(px(42.))
                        .text_size(px(22.))
                        .line_height(px(32.))
                        .font_weight(FontWeight::SEMIBOLD),
                ),
            )
            .child(
                div().flex_1().min_h_0().px_3().pb_2().child(
                    Textarea::new(&self.body)
                        .appearance(false)
                        .h_full()
                        .font_family(mono)
                        .text_size(px(14.))
                        .line_height(relative(1.6)),
                ),
            )
    }
}

impl NoteEditor {
    fn blocking_save(&mut self) -> Option<Box<dyn FnOnce() + Send>> {
        if self.pending.is_empty() {
            return None;
        }
        let note_id = self.note_id.clone()?;
        let fields = std::mem::take(&mut self.pending);
        let client = self.client.clone();
        let org_id = self.org_id.clone();
        Some(Box::new(move || {
            if let Err(e) = client.update_note(
                &org_id,
                &note_id,
                fields.title.as_deref(),
                fields.body.as_deref(),
            ) {
                log::error!("could not save note {note_id} on quit: {e}");
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test] // not gpui's #[test], which `super::*` brings in
    fn restore_never_overwrites_newer_typing() {
        let mut pending = Pending {
            title: None,
            body: Some("newer".into()),
        };
        pending.restore(Pending {
            title: Some("old title".into()),
            body: Some("older".into()),
        });
        assert_eq!(pending.title.as_deref(), Some("old title"));
        assert_eq!(pending.body.as_deref(), Some("newer"));
    }
}
