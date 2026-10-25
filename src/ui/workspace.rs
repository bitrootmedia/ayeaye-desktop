//! The signed-in window: title bar, module rail, sidebar, tabs, status bar.
//!
//! One workspace is one profile. Organisations switch inside it; profiles
//! switch by replacing it. Each module contributes a sidebar panel; every
//! module's items share one strip of tabs. Panels and views ask for things
//! to be opened with [`Open`], and this is the only place that turns an id
//! into a tab.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::badge::Badge;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, ResizableState, Selectable as _, Sizable as _, TitleBar,
    WindowExt as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use super::item::ItemHandle;
use super::modules::Module;
use super::open::{Open, key_for, open_for_key};
use super::pane::{Pane, PaneEvent};
use super::prompt::{PromptEvent, PromptView};
use super::quick_open::{QuickOpen, QuickOpenEvent};
use super::search::{SearchEvent, SearchView};
use crate::actions::*;
use crate::api::models::Organisation;
use crate::api::{ApiClient, ApiError, clamp_title};
use crate::app_state::AppState;
use crate::assets::Lucide;
use crate::kb::article::{ArticleEvent, ArticleView};
use crate::kb::panel::{KbPanel, KbPanelEvent};
use crate::notes::editor::NoteEditor;
use crate::notes::panel::{NotesPanel, PanelEvent};
use crate::notes::store::{LoadState, NotesEvent, NotesStore};
use crate::personal::inbox::{InboxEvent, InboxPanel};
use crate::personal::reminders::{RemindersEvent, RemindersPanel};
use crate::personal::sparks::{SparkView, SparkViewEvent, SparksEvent, SparksPanel};
use crate::realtime::{LiveState, Nudges, Realtime};
use crate::session::{Directory, OrgCtx, Session};
use crate::store::profiles::Profile;
use crate::store::workspace::OpenTabs;
use crate::store::{orgs_cache_path, read_json, write_json};
use crate::tasks::panel::{TasksPanel, TasksPanelEvent};
use crate::tasks::planner::{PlannerEvent, PlannerPanel};
use crate::tasks::view::{TaskView, TaskViewEvent};
use crate::timer::TimerState;
use crate::ui::widgets::clock;

/// A window coming back to the front refetches, but not more often than this.
const REFRESH_ON_ACTIVATE_AFTER: Duration = Duration::from_secs(30);
/// The inbox and the timer have no events of their own; anything happening
/// nearby refetches them, but not more often than this.
const PERSONAL_REFRESH_EVERY: Duration = Duration::from_secs(10);

pub enum WorkspaceEvent {
    /// The session needs the person: sign in again, or answer a gate.
    AuthRequired(ApiError),
    SwitchProfile(String),
    AddProfile,
    SignOut,
    RemoveProfile,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

/// Everything that belongs to the selected organisation; rebuilt on switch.
struct OrgModules {
    ctx: OrgCtx,
    notes_store: Entity<NotesStore>,
    notes: Entity<NotesPanel>,
    tasks: Entity<TasksPanel>,
    planner: Entity<PlannerPanel>,
    kb: Entity<KbPanel>,
    _subscriptions: Vec<Subscription>,
}

/// The person's own things, which outlive an organisation switch.
struct Personal {
    sparks: Entity<SparksPanel>,
    reminders: Entity<RemindersPanel>,
    inbox: Entity<InboxPanel>,
    timer: Entity<TimerState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
enum PromptKind {
    NewTask,
    Spark,
    NewBook,
    NewArticle { book_id: String },
}

enum Overlay {
    QuickOpen(Entity<QuickOpen>),
    Search(Entity<SearchView>),
    Prompt(Entity<PromptView>),
}

pub struct Workspace {
    profile: Profile,
    session: Session,
    orgs: Vec<Organisation>,
    org: Option<Organisation>,
    orgs_error: Option<String>,
    orgs_loaded: bool,
    module: Module,
    modules: Option<OrgModules>,
    personal: Personal,
    pane: Entity<Pane>,
    split: Entity<ResizableState>,
    sidebar_open: bool,
    overlay: Option<Overlay>,
    /// Tabs to reopen once what they show has arrived (first run, or a
    /// cache that didn't have them yet).
    pending_restore: Option<OpenTabs>,
    /// An `Open` waiting for an organisation switch to finish.
    pending_open: Option<Open>,
    last_refresh: Instant,
    realtime: Entity<Realtime>,
    last_personal_refresh: Instant,
    _personal_refresh: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Workspace {
    pub fn new(profile: Profile, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let client = AppState::update(cx, |state, _| state.client(&profile));
        let session = Session {
            base_url: profile.base_url.clone(),
            client: client.clone(),
            me: profile.user_id.clone(),
        };
        let pane = cx.new(Pane::new);
        let split = cx.new(|_| ResizableState::default());
        let saved = &AppState::get(cx).workspace;
        let sidebar_open = saved.sidebar_open;
        let module = saved
            .module
            .as_deref()
            .and_then(Module::from_key)
            .unwrap_or(Module::Notes);

        let personal = Self::build_personal(&session, window, cx);
        let realtime =
            cx.new(|cx| Realtime::new(profile.id.clone(), &profile.base_url, client.clone(), cx));
        let mut subscriptions = vec![
            cx.subscribe_in(&pane, window, Self::on_pane_event),
            cx.subscribe_in(&realtime, window, Self::on_nudges),
            cx.observe(&realtime, |_, _, cx| cx.notify()),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active()
                    && this.last_refresh.elapsed() > REFRESH_ON_ACTIVATE_AFTER
                {
                    this.refresh(window, cx);
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                crate::theme::follow_system(window, cx)
            }),
        ];
        subscriptions.push(cx.on_app_quit(Self::on_quit));
        // Keyboard shortcuts live on the workspace: when the focused element
        // goes away (a panel swapped out, a tab closed), take focus back so
        // ⌘-anything still works.
        let focus_handle = cx.focus_handle();
        subscriptions
            .push(cx.on_focus_lost(window, |this, window, cx| this.focus_active(window, cx)));

        let orgs: Vec<Organisation> = read_json(&orgs_cache_path(&profile.id)).unwrap_or_default();
        let mut workspace = Self {
            profile,
            session,
            orgs,
            org: None,
            orgs_error: None,
            orgs_loaded: false,
            module,
            modules: None,
            personal,
            pane,
            split,
            sidebar_open,
            overlay: None,
            pending_restore: None,
            pending_open: None,
            last_refresh: Instant::now(),
            realtime,
            last_personal_refresh: Instant::now(),
            _personal_refresh: None,
            focus_handle,
            _subscriptions: subscriptions,
            _tasks: Vec::new(),
        };
        if let Some(org) = workspace.preferred_org() {
            workspace.set_org(org, window, cx);
        }
        workspace.load_session(window, cx);
        window.focus(&workspace.focus_handle, cx);
        workspace
    }

    fn build_personal(session: &Session, window: &mut Window, cx: &mut Context<Self>) -> Personal {
        let client = session.client.clone();
        let sparks = cx.new(|cx| SparksPanel::new(client.clone(), window, cx));
        let reminders = cx.new(|cx| RemindersPanel::new(client.clone(), window, cx));
        let inbox = cx.new(|cx| InboxPanel::new(client.clone(), session.base_url.clone(), cx));
        let timer = cx.new(|cx| TimerState::new(client.clone(), cx));
        let subscriptions = vec![
            cx.subscribe_in(&sparks, window, |this, _, open: &Open, window, cx| {
                this.open(open.clone(), window, cx)
            }),
            cx.subscribe_in(&sparks, window, |_, _, e: &SparksEvent, _, cx| match e {
                SparksEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
            }),
            // Sparks can only reopen once the list has arrived.
            cx.observe_in(&sparks, window, |this, _, window, cx| {
                if this.pending_restore.is_some() {
                    this.restore_tabs(window, cx);
                }
            }),
            cx.subscribe_in(&reminders, window, |this, _, open: &Open, window, cx| {
                this.open(open.clone(), window, cx)
            }),
            cx.subscribe_in(
                &reminders,
                window,
                |_, _, e: &RemindersEvent, _, cx| match e {
                    RemindersEvent::AuthFailed(e) => {
                        cx.emit(WorkspaceEvent::AuthRequired(e.clone()))
                    }
                },
            ),
            cx.observe(&reminders, |_, _, cx| cx.notify()),
            cx.subscribe_in(&inbox, window, |this, _, open: &Open, window, cx| {
                this.open(open.clone(), window, cx)
            }),
            cx.subscribe_in(&inbox, window, |_, _, e: &InboxEvent, _, cx| match e {
                InboxEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
            }),
            cx.observe(&inbox, |_, _, cx| cx.notify()),
            cx.observe(&timer, |_, _, cx| cx.notify()),
        ];
        Personal {
            sparks,
            reminders,
            inbox,
            timer,
            _subscriptions: subscriptions,
        }
    }

    fn preferred_org(&self) -> Option<Organisation> {
        self.profile
            .last_org_id
            .as_ref()
            .and_then(|id| self.orgs.iter().find(|o| &o.id == id))
            .or_else(|| self.orgs.first())
            .cloned()
    }

    /// `/api/me` first — it gates everything — then the organisations.
    fn load_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.session.client.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let me = client.me()?;
                    client.sync_timezone(&me);
                    let orgs = client.organisations()?;
                    Ok::<_, ApiError>((me, orgs))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.orgs_loaded = true;
                match result {
                    Ok((me, orgs)) => {
                        let id = this.profile.id.clone();
                        this.profile.display_name = me.display_name.clone();
                        if me.email.is_some() {
                            this.profile.email = me.email.clone();
                        }
                        this.profile.user_id = Some(me.id.clone());
                        this.session.me = Some(me.id.clone());
                        if let Some(m) = &this.modules {
                            let me = Some(me.id.clone());
                            m.tasks.update(cx, |t, cx| t.set_me(me, cx));
                        }
                        let snapshot = this.profile.clone();
                        AppState::update(cx, |state, _| {
                            state.update_profile(&id, |p| *p = snapshot)
                        });
                        this.orgs_error = None;
                        this.set_orgs(orgs, window, cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(WorkspaceEvent::AuthRequired(e)),
                    Err(e) => this.orgs_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }

    fn set_orgs(&mut self, orgs: Vec<Organisation>, window: &mut Window, cx: &mut Context<Self>) {
        let path = orgs_cache_path(&self.profile.id);
        let cached = orgs.clone();
        cx.background_spawn(async move {
            let _ = write_json(&path, &cached);
        })
        .detach();
        self.orgs = orgs;
        let still_there = self
            .org
            .as_ref()
            .is_some_and(|current| self.orgs.iter().any(|o| o.id == current.id));
        if !still_there {
            match self.preferred_org() {
                Some(org) => self.set_org(org, window, cx),
                None => self.clear_org(window, cx),
            }
        }
    }

    // ----------------------------------------------------------- organisation

    fn set_org(&mut self, org: Organisation, window: &mut Window, cx: &mut Context<Self>) {
        if self.org.as_ref().is_some_and(|o| o.id == org.id) && self.modules.is_some() {
            return;
        }
        self.save_tabs(cx);
        self.pane.update(cx, |pane, cx| pane.close_all(window, cx));
        self.overlay = None;

        let client = self.session.client.clone();
        let directory = cx.new(|_| Directory::new(&org.id, client.clone()));
        let ctx = OrgCtx {
            session: self.session.clone(),
            org: org.clone(),
            directory,
        };
        let notes_store =
            cx.new(|cx| NotesStore::new(&self.profile.id, &org.id, client.clone(), cx));
        let notes = cx.new(|cx| NotesPanel::new(notes_store.clone(), window, cx));
        let tasks = cx.new(|cx| TasksPanel::new(ctx.clone(), window, cx));
        let planner = cx.new(|cx| PlannerPanel::new(ctx.clone(), cx));
        let kb = cx.new(|cx| KbPanel::new(ctx.clone(), cx));
        let subscriptions = vec![
            cx.subscribe_in(&notes_store, window, Self::on_notes_event),
            cx.subscribe_in(&notes, window, Self::on_notes_panel_event),
            cx.subscribe_in(&tasks, window, |this, _, open: &Open, window, cx| {
                this.open(open.clone(), window, cx)
            }),
            cx.subscribe_in(
                &tasks,
                window,
                |this, _, e: &TasksPanelEvent, window, cx| match e {
                    TasksPanelEvent::NewTask => this.show_prompt(PromptKind::NewTask, window, cx),
                    TasksPanelEvent::AuthFailed(e) => {
                        cx.emit(WorkspaceEvent::AuthRequired(e.clone()))
                    }
                },
            ),
            cx.subscribe_in(&planner, window, |this, _, open: &Open, window, cx| {
                this.open(open.clone(), window, cx)
            }),
            cx.subscribe_in(&planner, window, |_, _, e: &PlannerEvent, _, cx| match e {
                PlannerEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
            }),
            cx.subscribe_in(&kb, window, |this, _, open: &Open, window, cx| {
                this.open(open.clone(), window, cx)
            }),
            cx.subscribe_in(
                &kb,
                window,
                |this, _, e: &KbPanelEvent, window, cx| match e {
                    KbPanelEvent::NewBook => this.show_prompt(PromptKind::NewBook, window, cx),
                    KbPanelEvent::NewArticle { book_id } => this.show_prompt(
                        PromptKind::NewArticle {
                            book_id: book_id.clone(),
                        },
                        window,
                        cx,
                    ),
                    KbPanelEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
                },
            ),
        ];
        self.modules = Some(OrgModules {
            ctx,
            notes_store,
            notes,
            tasks,
            planner,
            kb,
            _subscriptions: subscriptions,
        });
        self.personal
            .reminders
            .update(cx, |r, _| r.set_org(Some(org.id.clone())));

        let profile_id = self.profile.id.clone();
        let org_id = org.id.clone();
        self.profile.last_org_id = Some(org_id.clone());
        AppState::update(cx, |state, _| {
            state.update_profile(&profile_id, |p| p.last_org_id = Some(org_id.clone()))
        });
        let tabs = AppState::get(cx).workspace.tabs(&profile_id, &org.id);
        let board = format!("org:{}", org.id);
        self.realtime.update(cx, |r, _| r.watch_board(Some(board)));
        self.org = Some(org);
        self.last_refresh = Instant::now();
        self.pending_restore = Some(tabs);
        self.restore_tabs(window, cx);
        if let Some(open) = self.pending_open.take() {
            self.open(open, window, cx);
        }
        cx.notify();
    }

    fn clear_org(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_tabs(cx);
        self.pane.update(cx, |pane, cx| pane.close_all(window, cx));
        self.modules = None;
        self.org = None;
        self.personal.reminders.update(cx, |r, _| r.set_org(None));
        cx.notify();
    }

    fn org_ctx(&self) -> Option<OrgCtx> {
        self.modules.as_ref().map(|m| m.ctx.clone())
    }

    /// Reopens what was open last time, for every tab whose subject is
    /// known. Called again as lists arrive, for any that weren't yet.
    fn restore_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tabs) = self.pending_restore.take() else {
            return;
        };
        let Some(modules) = &self.modules else { return };
        let notes_fresh = modules.notes_store.read(cx).is_fresh();
        let notes_store = modules.notes_store.clone();
        let mut missing = OpenTabs::default();
        for key in &tabs.items {
            if self.pane.read(cx).find(key, cx).is_some() {
                continue;
            }
            let ready = match open_for_key(key) {
                Some(Open::Note { ref id, .. }) => notes_store.read(cx).get(id).is_some(),
                Some(Open::Spark { ref id }) => self.personal.sparks.read(cx).get(id).is_some(),
                Some(_) => true,
                None => continue,
            };
            if ready {
                if let Some(open) = open_for_key(key) {
                    self.open(open, window, cx);
                }
            } else {
                missing.items.push(key.clone());
            }
        }
        if let Some(active) = &tabs.active {
            if let Some(ix) = self.pane.read(cx).find(active, cx) {
                self.pane
                    .update(cx, |pane, cx| pane.activate(ix, true, window, cx));
            } else {
                missing.active = Some(active.clone());
            }
        }
        // Until the notes list has answered, a note we can't see may just be
        // one made on another machine since the cache was written.
        if !missing.items.is_empty()
            && !(notes_fresh && missing.items.iter().all(|k| k.starts_with("note:")))
        {
            self.pending_restore = Some(missing);
        }
    }

    fn save_tabs(&self, cx: &mut Context<Self>) {
        let Some(org) = &self.org else { return };
        let pane = self.pane.read(cx);
        let restorable = |key: &str| open_for_key(key).is_some();
        let items: Vec<String> = pane
            .items()
            .map(|item| item.item_key(cx).to_string())
            .filter(|key| restorable(key))
            .collect();
        let active = pane
            .active_item()
            .map(|item| item.item_key(cx).to_string())
            .filter(|key| restorable(key));
        let tabs = OpenTabs { items, active };
        let profile_id = self.profile.id.clone();
        let org_id = org.id.clone();
        AppState::update(cx, |state, _| {
            if state.workspace.tabs(&profile_id, &org_id) != tabs {
                state.workspace.set_tabs(&profile_id, &org_id, tabs);
                state.workspace.save();
            }
        });
    }

    // ------------------------------------------------------------------- open

    /// Turns an `Open` into a tab — the existing one if it's already open.
    pub fn open(&mut self, open: Open, window: &mut Window, cx: &mut Context<Self>) {
        // Something in another organisation: switch there first.
        if let Open::Task {
            org_id: Some(org_id),
            ..
        } = &open
            && self.org.as_ref().map(|o| &o.id) != Some(org_id)
        {
            match self.orgs.iter().find(|o| &o.id == org_id).cloned() {
                Some(org) => {
                    self.pending_open = Some(open);
                    self.set_org(org, window, cx);
                }
                None => {
                    window.push_notification("That's in an organisation you're no longer in.", cx)
                }
            }
            return;
        }
        if let Open::Web(url) = &open {
            cx.open_url(url);
            return;
        }
        let (preview, focus) = match &open {
            Open::Note { preview, focus, .. } => (*preview, *focus),
            Open::Task { preview, .. } | Open::Article { preview, .. } => (*preview, !*preview),
            _ => (false, true),
        };
        if let Some(key) = key_for(&open)
            && let Some(ix) = self.pane.read(cx).find(&key, cx)
        {
            self.pane.update(cx, |pane, cx| {
                if !preview {
                    pane.keep_preview(cx);
                }
                pane.activate(ix, focus || !preview, window, cx)
            });
            return;
        }
        let item: Option<Box<dyn ItemHandle>> = match open {
            Open::Note { id, .. } => self.note_item(&id, window, cx),
            Open::Task { id, .. } => self.task_item(id, window, cx),
            Open::Article { id, .. } => self.article_item(id, window, cx),
            Open::Spark { id } => self.spark_item(&id, window, cx),
            Open::Web(_) => None,
        };
        if let Some(item) = item {
            self.pane.update(cx, |pane, cx| {
                pane.add_item(item, preview, focus, window, cx)
            });
        }
    }

    fn note_item(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        let store = self.modules.as_ref()?.notes_store.clone();
        let note = store.read(cx).get(id).cloned()?;
        Some(Box::new(
            cx.new(|cx| NoteEditor::open(store, &note, window, cx)),
        ))
    }

    fn task_item(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        let ctx = self.org_ctx()?;
        let timer = self.personal.timer.clone();
        let view = cx.new(|cx| TaskView::new(ctx, id, timer, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            |this, _, e: &TaskViewEvent, _, cx| match e {
                TaskViewEvent::Changed(task) => {
                    if let Some(m) = &this.modules {
                        m.tasks.update(cx, |t, cx| t.task_changed(task, cx));
                        m.planner.update(cx, |p, cx| p.refresh(cx));
                    }
                }
                TaskViewEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
            },
        ));
        Some(Box::new(view))
    }

    fn article_item(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        let ctx = self.org_ctx()?;
        let view = cx.new(|cx| ArticleView::new(ctx, id, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            |this, _, e: &ArticleEvent, _, cx| match e {
                ArticleEvent::Changed { book_id } => {
                    if let Some(m) = &this.modules {
                        m.kb.update(cx, |kb, cx| kb.reload_book(book_id, cx));
                    }
                }
                ArticleEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
            },
        ));
        Some(Box::new(view))
    }

    fn spark_item(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        let spark = self.personal.sparks.read(cx).get(id).cloned()?;
        let client = self.session.client.clone();
        let view = cx.new(|cx| SparkView::new(client, spark, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            |this, view, e: &SparkViewEvent, window, cx| match e {
                SparkViewEvent::Saved(spark) => {
                    let spark = spark.clone();
                    this.personal.sparks.update(cx, |s, cx| s.upsert(spark, cx));
                }
                SparkViewEvent::Deleted(id) => {
                    let id = id.clone();
                    this.personal.sparks.update(cx, |s, cx| s.remove(&id, cx));
                }
                SparkViewEvent::MakeTask(spark) => {
                    let spark = spark.clone();
                    this.spark_to_task(spark, view.clone(), window, cx);
                }
            },
        ));
        Some(Box::new(view))
    }

    // ------------------------------------------------------------- creating

    fn show_prompt(&mut self, kind: PromptKind, window: &mut Window, cx: &mut Context<Self>) {
        let needs_org = !matches!(kind, PromptKind::Spark);
        if needs_org && self.modules.is_none() {
            window.push_notification("Pick an organisation first.", cx);
            return;
        }
        let org = self
            .org
            .as_ref()
            .map(|o| o.name.clone())
            .unwrap_or_default();
        let (label, placeholder, hint, icon): (String, &str, String, Icon) = match &kind {
            PromptKind::NewTask => (
                format!("New task in {org}"),
                "What needs doing?",
                "⏎ creates it and opens it — fill in the rest there.".into(),
                Icon::new(Lucide::SquareCheck),
            ),
            PromptKind::Spark => (
                "Spark".into(),
                "A thought that isn't work yet…",
                "⏎ keeps it in Sparks. Only you can see it, in no organisation.".into(),
                Icon::new(Lucide::Lightbulb),
            ),
            PromptKind::NewBook => (
                format!("New book in {org}"),
                "Book name",
                "Books hold articles; share them from the web app.".into(),
                Icon::new(Lucide::BookOpen),
            ),
            PromptKind::NewArticle { .. } => (
                "New article".into(),
                "Title",
                "It starts as a private draft — publish it when it's ready.".into(),
                Icon::new(Lucide::BookText),
            ),
        };
        let view = cx.new(|cx| PromptView::new(label, placeholder, hint, icon, window, cx));
        let k = kind.clone();
        self._subscriptions.push(cx.subscribe_in(
            &view,
            window,
            move |this, _, e: &PromptEvent, window, cx| {
                this.overlay = None;
                match e {
                    PromptEvent::Submit(text) => {
                        this.submit_prompt(k.clone(), text.clone(), window, cx)
                    }
                    PromptEvent::Dismissed => this.focus_active(window, cx),
                }
                cx.notify();
            },
        ));
        self.overlay = Some(Overlay::Prompt(view));
        cx.notify();
    }

    fn submit_prompt(
        &mut self,
        kind: PromptKind,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            PromptKind::Spark => {
                self.personal.sparks.update(cx, |s, cx| s.create(text, cx));
                window.push_notification("Spark kept.", cx);
                self.focus_active(window, cx);
            }
            PromptKind::NewTask => self.create_task(text, None, window, cx),
            PromptKind::NewBook => {
                let Some(ctx) = self.org_ctx() else { return };
                self.run(
                    window,
                    cx,
                    move |c: &ApiClient| c.create_book(ctx.org_id(), &text).map(|_| ()),
                    |this, _, _, cx| {
                        if let Some(m) = &this.modules {
                            m.kb.update(cx, |kb, cx| kb.refresh(cx));
                        }
                    },
                );
            }
            PromptKind::NewArticle { book_id } => {
                let Some(ctx) = self.org_ctx() else { return };
                let bid = book_id.clone();
                self.run(
                    window,
                    cx,
                    move |c: &ApiClient| c.create_article(ctx.org_id(), &bid, &text),
                    move |this, article, window, cx| {
                        if let Some(m) = &this.modules {
                            m.kb.update(cx, |kb, cx| kb.reload_book(&book_id, cx));
                        }
                        this.open(
                            Open::Article {
                                id: article.id,
                                preview: false,
                            },
                            window,
                            cx,
                        );
                    },
                );
            }
        }
    }

    /// A task from a title (and optional markdown), owned by me, opened.
    fn create_task(
        &mut self,
        title: String,
        description: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ctx) = self.org_ctx() else { return };
        let org_id = ctx.org_id().to_owned();
        let mut fields = json!({ "title": clamp_title(&title) });
        if let Some(me) = &self.session.me {
            fields["owner_user_id"] = me.clone().into();
        }
        if let Some(d) = description.filter(|d| !d.trim().is_empty()) {
            fields["description"] = d.into();
        }
        self.run(
            window,
            cx,
            move |c: &ApiClient| c.create_task(&org_id, fields),
            |this, task, window, cx| {
                if let Some(m) = &this.modules {
                    m.tasks.update(cx, |t, cx| t.refresh(cx));
                }
                this.open(
                    Open::Task {
                        id: task.id,
                        org_id: None,
                        preview: false,
                    },
                    window,
                    cx,
                );
            },
        );
    }

    /// "Make a task": the first line is the title, the rest the description;
    /// the spark goes once the task exists.
    fn spark_to_task(
        &mut self,
        spark: crate::api::models::Spark,
        view: Entity<SparkView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ctx) = self.org_ctx() else {
            window.push_notification("Pick an organisation first — a task lives in one.", cx);
            return;
        };
        let mut lines = spark.body.trim().lines();
        let title = clamp_title(lines.next().unwrap_or("Spark"));
        let description: String = lines.collect::<Vec<_>>().join("\n");
        let org_id = ctx.org_id().to_owned();
        let mut fields = json!({ "title": title });
        if let Some(me) = &self.session.me {
            fields["owner_user_id"] = me.clone().into();
        }
        if !description.trim().is_empty() {
            fields["description"] = description.trim().to_owned().into();
        }
        self.run(
            window,
            cx,
            move |c: &ApiClient| c.create_task(&org_id, fields),
            move |this, task, window, cx| {
                view.update(cx, |v, cx| v.delete_now(cx));
                if let Some(m) = &this.modules {
                    m.tasks.update(cx, |t, cx| t.refresh(cx));
                }
                this.open(
                    Open::Task {
                        id: task.id,
                        org_id: None,
                        preview: false,
                    },
                    window,
                    cx,
                );
            },
        );
    }

    /// One background call with its result handled on the main thread; a
    /// failure becomes a notification, never silence.
    fn run<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        call: impl FnOnce(&ApiClient) -> Result<T, ApiError> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let client: Arc<ApiClient> = self.session.client.clone();
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { call(&client) }).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(value) => done(this, value, window, cx),
                Err(e) if e.is_auth() => cx.emit(WorkspaceEvent::AuthRequired(e)),
                Err(e) => window.push_notification(e.to_string(), cx),
            })
            .ok();
        }));
    }

    // ------------------------------------------------------------------ notes

    fn notes_store(&self) -> Option<Entity<NotesStore>> {
        self.modules.as_ref().map(|m| m.notes_store.clone())
    }

    fn create_note(&mut self, title: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.notes_store() else {
            window.push_notification("Pick an organisation first — notes live in one.", cx);
            return;
        };
        let has_title = title.is_some();
        let editor =
            cx.new(|cx| NoteEditor::create(store, "", &title.unwrap_or_default(), window, cx));
        self.pane.update(cx, |pane, cx| {
            pane.add_item(Box::new(editor.clone()), false, false, window, cx)
        });
        // A titled note (from the filter) goes straight to the body.
        if has_title {
            ItemHandle::focus(&editor, window, cx);
        } else {
            editor.update(cx, |e, cx| e.focus_title(false, window, cx));
        }
    }

    fn confirm_delete_note(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.notes_store() else {
            return;
        };
        let Some(note) = store.read(cx).get(&id).cloned() else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let id = id.clone();
            alert
                .title(format!("Delete “{}”?", note.title))
                .description("It's gone for good — the web app has no bin for notes either.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.delete_note(id.clone(), window, cx))
                        .ok();
                    true
                })
        });
    }

    fn delete_note(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.notes_store() else {
            return;
        };
        let org_id = store.read(cx).org_id().to_owned();
        let delete_id = id.clone();
        self._tasks.push(cx.spawn_in(window, {
            let client = self.session.client.clone();
            async move |this, cx| {
                let result = cx
                    .background_spawn(async move { client.delete_note(&org_id, &delete_id) })
                    .await;
                this.update_in(cx, |this, window, cx| match result {
                    // Already gone is what we wanted.
                    Ok(()) | Err(ApiError::NotFound(_)) => {
                        store.update(cx, |store, cx| store.remove(&id, cx));
                        let key: SharedString = format!("note:{id}").into();
                        let found = this.pane.read(cx).find(&key, cx);
                        if let Some(ix) = found {
                            this.pane.update(cx, |pane, cx| pane.close(ix, window, cx));
                        }
                    }
                    Err(e) => {
                        window.push_notification(format!("Couldn't delete the note: {e}"), cx)
                    }
                })
                .ok();
            }
        }));
    }

    fn active_key(&self, cx: &App) -> Option<SharedString> {
        Some(self.pane.read(cx).active_item()?.item_key(cx))
    }

    fn active_id(&self, prefix: &str, cx: &App) -> Option<String> {
        let key = self.active_key(cx)?;
        key.strip_prefix(prefix).map(str::to_owned)
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.last_refresh = Instant::now();
        if let Some(m) = &self.modules {
            m.notes_store.update(cx, |s, cx| s.refresh(cx));
            m.ctx.directory.update(cx, |d, cx| d.refresh(cx));
            match self.module {
                Module::Tasks => m.tasks.update(cx, |t, cx| t.refresh(cx)),
                Module::Planner => m.planner.update(cx, |p, cx| p.refresh(cx)),
                Module::Knowledge => m.kb.update(cx, |k, cx| k.refresh(cx)),
                _ => {}
            }
        }
        self.personal.inbox.update(cx, |i, cx| i.refresh(cx));
        self.personal.timer.update(cx, |t, cx| t.refresh(cx));
        self.last_personal_refresh = Instant::now();
        self.realtime.update(cx, |r, cx| r.ensure_started(cx));
        match self.module {
            Module::Sparks => self.personal.sparks.update(cx, |s, cx| s.refresh(cx)),
            Module::Reminders => self.personal.reminders.update(cx, |r, cx| r.refresh(cx)),
            _ => {}
        }
        if self.orgs_loaded {
            self.orgs_loaded = false;
            self.load_session(window, cx);
        }
    }

    // --------------------------------------------------------------- realtime

    /// Something changed on the server. The events say only *what*, so this
    /// refetches whatever on screen shows it: open task tabs, the list in
    /// front, and — throttled — the inbox and the timer.
    fn on_nudges(
        &mut self,
        _: &Entity<Realtime>,
        nudges: &Nudges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if nudges.reconnected {
            // Back after a drop; whatever happened meanwhile was missed.
            self.refresh(window, cx);
            self.reload_task_tabs(|_| true, window, cx);
            return;
        }
        self.reload_task_tabs(|id| nudges.tasks.contains(id), window, cx);
        let here = self
            .org
            .as_ref()
            .is_some_and(|org| nudges.orgs.contains(&org.id));
        if here && let Some(m) = &self.modules {
            match self.module {
                Module::Tasks => m.tasks.update(cx, |t, cx| t.refresh(cx)),
                Module::Planner => m.planner.update(cx, |p, cx| p.refresh(cx)),
                _ => {}
            }
        }
        // A check-in on the running task can't wait for the throttle.
        let timer_task = self
            .personal
            .timer
            .read(cx)
            .running_task()
            .map(str::to_owned);
        if timer_task.is_some_and(|id| nudges.tasks.contains(&id)) {
            self.personal.timer.update(cx, |t, cx| t.refresh(cx));
        }
        self.schedule_personal_refresh(cx);
    }

    /// Reloads open task tabs whose id passes `which`. Each keeps what's
    /// being typed: the description editor, a focused title, the comment.
    fn reload_task_tabs(
        &mut self,
        which: impl Fn(&str) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let views: Vec<Entity<TaskView>> = self
            .pane
            .read(cx)
            .items()
            .filter(|item| item.item_key(cx).strip_prefix("task:").is_some_and(&which))
            .filter_map(|item| item.to_any_view().downcast::<TaskView>().ok())
            .collect();
        for view in views {
            view.update(cx, |v, cx| v.reload(window, cx));
        }
    }

    fn schedule_personal_refresh(&mut self, cx: &mut Context<Self>) {
        if self._personal_refresh.is_some() {
            return;
        }
        let wait = PERSONAL_REFRESH_EVERY.saturating_sub(self.last_personal_refresh.elapsed());
        self._personal_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this._personal_refresh = None;
                this.last_personal_refresh = Instant::now();
                this.personal.inbox.update(cx, |i, cx| i.refresh(cx));
                this.personal.timer.update(cx, |t, cx| t.refresh(cx));
            })
            .ok();
        }));
    }

    fn show_module(&mut self, module: Module, window: &mut Window, cx: &mut Context<Self>) {
        let was = self.module;
        self.module = module;
        if !self.sidebar_open {
            self.sidebar_open = true;
        }
        AppState::update(cx, |state, _| {
            state.workspace.module = Some(module.key().to_owned());
            state.workspace.sidebar_open = true;
            state.workspace.save();
        });
        // Coming back to a list is the moment it should be current.
        if was != module {
            if let Some(m) = &self.modules {
                match module {
                    Module::Planner => m.planner.update(cx, |p, cx| p.refresh(cx)),
                    Module::Tasks => m.tasks.update(cx, |t, cx| t.refresh(cx)),
                    _ => {}
                }
            }
            match module {
                Module::Reminders => self.personal.reminders.update(cx, |r, cx| r.refresh(cx)),
                Module::Inbox => self.personal.inbox.update(cx, |i, cx| i.refresh(cx)),
                _ => {}
            }
        }
        if module == Module::Sparks {
            let sparks = self.personal.sparks.clone();
            sparks.update(cx, |s, cx| s.focus_capture(window, cx));
        } else if !self.focus_handle.contains_focused(window, cx) {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    // ----------------------------------------------------------------- events

    fn on_pane_event(
        &mut self,
        _: &Entity<Pane>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Changed => {
                let note = self.active_id("note:", cx);
                let task = self.active_id("task:", cx);
                let article = self.active_id("article:", cx);
                let spark = self.active_id("spark:", cx);
                let thread = task.as_ref().map(|id| format!("task:{id}"));
                self.realtime.update(cx, |r, _| r.watch_thread(thread));
                if let Some(m) = &self.modules {
                    m.notes.update(cx, |p, cx| p.set_active_note(note, cx));
                    m.tasks
                        .update(cx, |p, cx| p.set_active_task(task.clone(), cx));
                    m.planner.update(cx, |p, cx| p.set_active_task(task, cx));
                    m.kb.update(cx, |p, cx| p.set_active_article(article, cx));
                }
                self.personal
                    .sparks
                    .update(cx, |p, cx| p.set_active(spark, cx));
                self.save_tabs(cx);
                cx.notify();
            }
            PaneEvent::NewItemRequested => match self.module {
                Module::Tasks | Module::Planner => {
                    self.show_prompt(PromptKind::NewTask, window, cx)
                }
                Module::Sparks => self.show_prompt(PromptKind::Spark, window, cx),
                _ => self.create_note(None, window, cx),
            },
        }
    }

    fn on_notes_event(
        &mut self,
        _: &Entity<NotesStore>,
        event: &NotesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            NotesEvent::Refreshed => self.restore_tabs(window, cx),
            NotesEvent::AuthFailed(e) => cx.emit(WorkspaceEvent::AuthRequired(e.clone())),
        }
    }

    fn on_notes_panel_event(
        &mut self,
        _: &Entity<NotesPanel>,
        event: &PanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PanelEvent::Open { id, preview, focus } => self.open(
                Open::Note {
                    id: id.clone(),
                    preview: *preview,
                    focus: *focus,
                },
                window,
                cx,
            ),
            PanelEvent::OpenNew { title } => self.create_note(Some(title.clone()), window, cx),
            PanelEvent::Delete { id } => self.confirm_delete_note(id.clone(), window, cx),
            PanelEvent::Dismiss => self.focus_active(window, cx),
        }
    }

    fn on_quick_open_event(
        &mut self,
        _: &Entity<QuickOpen>,
        event: &QuickOpenEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay = None;
        match event {
            QuickOpenEvent::Open(id) => self.open(
                Open::Note {
                    id: id.clone(),
                    preview: false,
                    focus: true,
                },
                window,
                cx,
            ),
            QuickOpenEvent::Create(title) => self.create_note(Some(title.clone()), window, cx),
            QuickOpenEvent::Dismissed => self.focus_active(window, cx),
        }
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        _: &Entity<SearchView>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay = None;
        match event {
            SearchEvent::Open(open) => self.open(open.clone(), window, cx),
            SearchEvent::Dismissed => self.focus_active(window, cx),
        }
        cx.notify();
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pane.read(cx).active_item().map(|i| i.boxed_clone()) {
            Some(item) => item.focus(window, cx),
            None => window.focus(&self.focus_handle, cx),
        }
    }

    /// Quitting waits (briefly) for every unsaved note to reach the server.
    fn on_quit(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        self.save_tabs(cx);
        let items: Vec<Box<dyn ItemHandle>> = self
            .pane
            .read(cx)
            .items()
            .map(|i| i.boxed_clone())
            .collect();
        let saves: Vec<_> = items
            .iter()
            .filter_map(|item| item.take_blocking_save(cx))
            .collect();
        cx.background_spawn(async move {
            for save in saves {
                save();
            }
        })
    }

    /// Hands the window over: everything still pending is sent first.
    pub fn flush_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_tabs(cx);
        self.pane.update(cx, |pane, cx| pane.flush_all(window, cx));
    }

    // ---------------------------------------------------------------- actions

    fn action_new_note(&mut self, _: &NewNote, window: &mut Window, cx: &mut Context<Self>) {
        self.create_note(None, window, cx);
    }

    fn action_new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.show_prompt(PromptKind::NewTask, window, cx);
    }

    fn action_capture_spark(
        &mut self,
        _: &CaptureSpark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_prompt(PromptKind::Spark, window, cx);
    }

    fn action_search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ctx) = self.org_ctx() else { return };
        let view = cx.new(|cx| SearchView::new(ctx, window, cx));
        self._subscriptions
            .push(cx.subscribe_in(&view, window, Self::on_search_event));
        self.overlay = Some(Overlay::Search(view));
        cx.notify();
    }

    fn action_show_module(
        &mut self,
        action: &ShowModule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(module) = Module::all().nth(action.0) {
            self.show_module(module, window, cx);
        }
    }

    fn action_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.pane
            .update(cx, |pane, cx| pane.close_active(window, cx));
    }

    fn action_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.pane
            .update(cx, |pane, cx| pane.activate_next(window, cx));
    }

    fn action_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.pane
            .update(cx, |pane, cx| pane.activate_prev(window, cx));
    }

    fn action_activate_tab(
        &mut self,
        action: &ActivateTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = action.0;
        self.pane.update(cx, |pane, cx| {
            // ⌘9 is always the last tab, as in browsers and Zed.
            let count = pane.items().count();
            if count == 0 {
                return;
            }
            let ix = if ix == 8 { count - 1 } else { ix };
            if ix < count {
                pane.activate(ix, true, window, cx);
            }
        });
    }

    fn action_reopen(&mut self, _: &ReopenClosedTab, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.pane.update(cx, |pane, _| pane.pop_closed());
        if let Some(open) = key.as_deref().and_then(open_for_key) {
            self.open(open, window, cx);
        }
    }

    fn action_toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_open = !self.sidebar_open;
        let open = self.sidebar_open;
        AppState::update(cx, |state, _| {
            state.workspace.sidebar_open = open;
            state.workspace.save();
        });
        if !open {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// ⌘⇧E: the filter of whichever list is showing.
    fn action_focus_notes(&mut self, _: &FocusNotes, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_open {
            self.sidebar_open = true;
            cx.notify();
        }
        let Some(m) = &self.modules else { return };
        match self.module {
            Module::Tasks => {
                let tasks = m.tasks.clone();
                tasks.update(cx, |t, cx| t.focus_filter(window, cx));
            }
            Module::Sparks => {
                let sparks = self.personal.sparks.clone();
                sparks.update(cx, |s, cx| s.focus_capture(window, cx));
            }
            _ => {
                self.module = Module::Notes;
                let notes = m.notes.clone();
                notes.update(cx, |p, cx| p.focus_filter(window, cx));
            }
        }
    }

    fn action_quick_open(&mut self, _: &FindNote, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.notes_store() else {
            return;
        };
        if matches!(self.overlay, Some(Overlay::QuickOpen(_))) {
            self.overlay = None;
            self.focus_active(window, cx);
        } else {
            let view = cx.new(|cx| QuickOpen::new(store, window, cx));
            self._subscriptions
                .push(cx.subscribe_in(&view, window, Self::on_quick_open_event));
            self.overlay = Some(Overlay::QuickOpen(view));
        }
        cx.notify();
    }

    fn action_save(&mut self, _: &SaveNow, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.pane.read(cx).active_item().map(|i| i.boxed_clone()) {
            item.flush(window, cx);
        }
    }

    fn action_delete(&mut self, _: &DeleteNote, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_id("note:", cx) {
            self.confirm_delete_note(id, window, cx);
        }
    }

    fn action_refresh(&mut self, _: &Refresh, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(window, cx);
    }

    // ----------------------------------------------------------------- render

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let this = cx.entity().downgrade();
        let orgs = self.orgs.clone();
        let current_org = self.org.as_ref().map(|o| o.id.clone());
        let org_label: SharedString = self
            .org
            .as_ref()
            .map(|o| o.name.clone())
            .unwrap_or_else(|| {
                if self.orgs_loaded {
                    "No organisation".into()
                } else {
                    "Loading…".into()
                }
            })
            .into();
        let personal_module = Module::PERSONAL.contains(&self.module);

        let org_menu = {
            let this = this.clone();
            Button::new("org-switcher")
                .label(org_label)
                .icon(Icon::new(Lucide::Building2).xsmall())
                .ghost()
                .xsmall()
                .dropdown_menu(move |mut menu, _, _| {
                    for org in &orgs {
                        let this = this.clone();
                        let org_clone = org.clone();
                        menu = menu.item(
                            PopupMenuItem::new(org.name.clone())
                                .checked(current_org.as_deref() == Some(org.id.as_str()))
                                .on_click(move |_, window, cx| {
                                    let org = org_clone.clone();
                                    this.update(cx, |this, cx| this.set_org(org, window, cx))
                                        .ok();
                                }),
                        );
                    }
                    if orgs.is_empty() {
                        menu = menu.label("You're not in any organisation yet");
                    }
                    let this = this.clone();
                    menu.separator()
                        .item(
                            PopupMenuItem::new("Refresh").on_click(move |_, window, cx| {
                                this.update(cx, |this, cx| this.refresh(window, cx)).ok();
                            }),
                        )
                })
        };

        let profiles = AppState::get(cx).profiles.profiles.clone();
        let current_profile = self.profile.id.clone();
        let profile_menu = Button::new("profile-switcher")
            .label(format!(
                "{} · {}",
                self.profile.person(),
                self.profile.host()
            ))
            .ghost()
            .xsmall()
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                for profile in &profiles {
                    let this = this.clone();
                    let id = profile.id.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{} — {}", profile.person(), profile.host()))
                            .checked(profile.id == current_profile)
                            .on_click(move |_, _, cx| {
                                let id = id.clone();
                                this.update(cx, |_, cx| cx.emit(WorkspaceEvent::SwitchProfile(id)))
                                    .ok();
                            }),
                    );
                }
                let add = this.clone();
                let sign_out = this.clone();
                let remove = this.clone();
                menu.separator()
                    .item(
                        PopupMenuItem::new("Add a server…")
                            .icon(Icon::new(Lucide::UserPlus))
                            .on_click(move |_, _, cx| {
                                add.update(cx, |_, cx| cx.emit(WorkspaceEvent::AddProfile))
                                    .ok();
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Sign out")
                            .icon(Icon::new(Lucide::LogOut))
                            .on_click(move |_, _, cx| {
                                sign_out
                                    .update(cx, |_, cx| cx.emit(WorkspaceEvent::SignOut))
                                    .ok();
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Remove this server from the app…").on_click(
                            move |_, _, cx| {
                                remove
                                    .update(cx, |_, cx| cx.emit(WorkspaceEvent::RemoveProfile))
                                    .ok();
                            },
                        ),
                    )
            });

        TitleBar::new().child(
            h_flex()
                .w_full()
                .h_full()
                .pr_2()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .when(!personal_module, |this| {
                            this.child(org_menu).child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .text_sm()
                                    .child("/"),
                            )
                        })
                        .child(div().text_sm().child(self.module.label())),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("title-search")
                                .icon(IconName::Search)
                                .ghost()
                                .xsmall()
                                .tooltip("Search (⌘⇧F)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.action_search(&Search, window, cx)
                                })),
                        )
                        .when(self.profile.is_insecure(), |this| {
                            this.child(
                                div()
                                    .id("insecure")
                                    .text_xs()
                                    .text_color(theme.warning)
                                    .child("http")
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "This server is reached over plain http.",
                                        )
                                        .build(window, cx)
                                    }),
                            )
                        })
                        .child(profile_menu),
                ),
        )
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let unread = self.personal.inbox.read(cx).unread().max(0) as usize;
        let due = self.personal.reminders.read(cx).due_count();
        let button = |module: Module, ix: usize, cx: &mut Context<Self>| {
            let active = module == self.module;
            let count = match module {
                Module::Inbox => unread,
                Module::Reminders => due,
                _ => 0,
            };
            let button = Button::new(SharedString::from(format!("module-{}", module.key())))
                .icon(Icon::new(module.icon()))
                .ghost()
                .small()
                .selected(active)
                .tooltip(format!("{} (⌘⌥{})", module.label(), ix + 1))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.show_module(module, window, cx)),
                );
            if count > 0 {
                Badge::new()
                    .count(count)
                    .max(99)
                    .child(button)
                    .into_any_element()
            } else {
                button.into_any_element()
            }
        };
        let org_buttons: Vec<_> = Module::ORG
            .iter()
            .enumerate()
            .map(|(ix, m)| button(*m, ix, cx))
            .collect();
        let personal_buttons: Vec<_> = Module::PERSONAL
            .iter()
            .enumerate()
            .map(|(ix, m)| button(*m, Module::ORG.len() + ix, cx))
            .collect();
        v_flex()
            .w(px(44.))
            .h_full()
            .flex_none()
            .py_2()
            .gap_1()
            .items_center()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .children(org_buttons)
            .child(div().w(px(20.)).h(px(1.)).my_1().bg(theme.border))
            .children(personal_buttons)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let personal = match self.module {
            Module::Sparks => Some(self.personal.sparks.clone().into_any_element()),
            Module::Reminders => Some(self.personal.reminders.clone().into_any_element()),
            Module::Inbox => Some(self.personal.inbox.clone().into_any_element()),
            _ => None,
        };
        if let Some(panel) = personal {
            return panel;
        }
        match &self.modules {
            Some(m) => match self.module {
                Module::Tasks => m.tasks.clone().into_any_element(),
                Module::Planner => m.planner.clone().into_any_element(),
                Module::Knowledge => m.kb.clone().into_any_element(),
                _ => m.notes.clone().into_any_element(),
            },
            None => v_flex()
                .size_full()
                .p_4()
                .gap_2()
                .bg(theme.sidebar)
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(match (&self.orgs_error, self.orgs_loaded) {
                    (Some(e), _) => format!("Couldn't load organisations: {e}"),
                    (None, false) => "Loading organisations…".to_owned(),
                    (None, true) => "You're not in any organisation on this server yet. Notes, tasks and the knowledge base live in one — accept an invite or create one in the web app.".to_owned(),
                })
                .when(self.orgs_loaded && self.orgs_error.is_none(), |this| {
                    let url = self.profile.base_url.clone();
                    this.child(
                        Button::new("open-web")
                            .label("Open the web app")
                            .small()
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                })
                .into_any_element(),
        }
    }

    fn render_timer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let timer = self.personal.timer.read(cx);
        let entry = timer.timer.entry.clone().filter(|e| e.ended_at.is_none())?;
        let elapsed = timer.elapsed();
        let asking = timer.timer.check_in_deadline.is_some();
        let open = Open::Task {
            id: entry.task_id.clone(),
            org_id: timer.timer.organisation_id.clone(),
            preview: false,
        };
        Some(
            h_flex()
                .gap_1()
                .child(
                    h_flex()
                        .id("timer")
                        .gap_1p5()
                        .px_1p5()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.accent))
                        .child(div().size(px(7.)).rounded_full().bg(theme.green))
                        .child(div().text_color(theme.foreground).child(clock(elapsed)))
                        .child(
                            div()
                                .max_w(px(240.))
                                .truncate()
                                .child(entry.task_title.clone().unwrap_or_else(|| "Timer".into())),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open(open.clone(), window, cx)
                        })),
                )
                .when(asking, |this| {
                    this.child(
                        div()
                            .text_color(theme.warning)
                            .child("Still working on this?"),
                    )
                    .child(
                        Button::new("check-in-yes")
                            .label("Yes")
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.personal
                                    .timer
                                    .update(cx, |t, cx| t.answer_check_in(true, cx))
                            })),
                    )
                })
                .child(
                    Button::new("timer-stop")
                        .icon(IconName::Pause)
                        .label("Stop")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.personal.timer.update(cx, |t, cx| t.stop(cx))
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let pane = self.pane.read(cx);
        let status = pane.active_item().and_then(|item| item.status_text(cx));
        let cursor = pane.active_item().and_then(|item| item.cursor_position(cx));
        let offline = self.orgs_error.is_some()
            || self
                .modules
                .as_ref()
                .is_some_and(|m| matches!(m.notes_store.read(cx).state(), LoadState::Failed(_)));
        let timer = self.render_timer(cx);

        h_flex()
            .h(px(26.))
            .flex_none()
            .px_1()
            .gap_2()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                Button::new("toggle-sidebar")
                    .icon(if self.sidebar_open {
                        IconName::PanelLeftClose
                    } else {
                        IconName::PanelLeftOpen
                    })
                    .ghost()
                    .xsmall()
                    .tooltip("Toggle sidebar")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.action_toggle_sidebar(&ToggleSidebar, window, cx)
                    })),
            )
            .when(offline, |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .text_color(theme.warning)
                        .child(Icon::new(Lucide::CloudOff).xsmall())
                        .child("Can't reach the server"),
                )
            })
            .children(timer)
            .child(div().flex_1())
            .when_some(status, |this, status| this.child(status))
            .child(self.render_live(cx))
            .when_some(cursor, |this, (line, col)| {
                this.child(div().pr_2().child(format!("Ln {line}, Col {col}")))
            })
    }
}

impl Workspace {
    /// A dot in the status bar: green while changes arrive live.
    fn render_live(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (color, tip) = match self.realtime.read(cx).state() {
            LiveState::Live => (
                theme.green,
                "Live — changes made elsewhere appear here as they happen",
            ),
            LiveState::Connecting => (
                theme.warning,
                "Reconnecting for live updates — until then ⌘R refetches",
            ),
            LiveState::Off => (
                theme.muted_foreground.opacity(0.5),
                "No live updates — ⌘R refetches",
            ),
        };
        div()
            .id("live")
            .px_1p5()
            .py_1()
            .child(div().size(px(7.)).rounded_full().bg(color))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sidebar = self.render_sidebar(cx);
        let overlay = self.overlay.as_ref().map(|o| match o {
            Overlay::QuickOpen(v) => v.clone().into_any_element(),
            Overlay::Search(v) => v.clone().into_any_element(),
            Overlay::Prompt(v) => v.clone().into_any_element(),
        });
        v_flex()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::action_new_note))
            .on_action(cx.listener(Self::action_new_task))
            .on_action(cx.listener(Self::action_capture_spark))
            .on_action(cx.listener(Self::action_search))
            .on_action(cx.listener(Self::action_show_module))
            .on_action(cx.listener(Self::action_close_tab))
            .on_action(cx.listener(Self::action_next_tab))
            .on_action(cx.listener(Self::action_prev_tab))
            .on_action(cx.listener(Self::action_activate_tab))
            .on_action(cx.listener(Self::action_reopen))
            .on_action(cx.listener(Self::action_toggle_sidebar))
            .on_action(cx.listener(Self::action_focus_notes))
            .on_action(cx.listener(Self::action_quick_open))
            .on_action(cx.listener(Self::action_save))
            .on_action(cx.listener(Self::action_delete))
            .on_action(cx.listener(Self::action_refresh))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .child(
                        // The split has to be told its width is what's left
                        // after the rail, or it lays out against the window
                        // and pushes the right edge of every tab off screen.
                        div().flex_1().min_w_0().h_full().child(
                            h_resizable("workspace-split")
                                .with_state(&self.split)
                                .child(
                                    resizable_panel()
                                        .size(px(270.))
                                        .size_range(px(200.)..px(560.))
                                        .visible(self.sidebar_open)
                                        .child(sidebar),
                                )
                                .child(resizable_panel().child(self.pane.clone())),
                        ),
                    ),
            )
            .child(self.render_status_bar(cx))
            .when_some(overlay, |this, overlay| {
                this.child(
                    div()
                        .absolute()
                        .top(px(48.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(overlay),
                )
            })
    }
}
