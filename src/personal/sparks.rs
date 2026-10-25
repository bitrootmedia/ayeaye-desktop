//! Sparks: the thought that isn't work yet. No organisation, nobody else ever
//! sees one — the web app's ⌘J, and ⌘J here too, from anywhere.
//!
//! The panel captures and lists; a spark opens in a tab to be edited, turned
//! into a task in the current organisation, or thrown away.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::models::Spark;
use crate::api::{ApiClient, ApiError};
use crate::assets::Lucide;
use crate::ui::format::{ago_long, ago_short};
use crate::ui::item::{Item, ItemEvent};
use crate::ui::open::Open;

const SAVE_DEBOUNCE: Duration = Duration::from_millis(700);

// ------------------------------------------------------------------- panel

pub enum SparksEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<Open> for SparksPanel {}
impl EventEmitter<SparksEvent> for SparksPanel {}

pub struct SparksPanel {
    client: Arc<ApiClient>,
    sparks: Vec<Spark>,
    loading: bool,
    error: Option<String>,
    active: Option<String>,
    capture: Entity<InputState>,
    _subscription: Subscription,
    _tasks: Vec<Task<()>>,
}

impl SparksPanel {
    pub fn new(client: Arc<ApiClient>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let capture = cx.new(|cx| InputState::new(window, cx).placeholder("Capture a thought… ⏎"));
        let subscription = cx.subscribe_in(
            &capture,
            window,
            |this: &mut Self, input, e: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    let body = input.read(cx).value().trim().to_owned();
                    if !body.is_empty() {
                        input.update(cx, |i, cx| i.set_value("", window, cx));
                        this.create(body, cx);
                    }
                }
            },
        );
        let mut panel = Self {
            client,
            sparks: Vec::new(),
            loading: false,
            error: None,
            active: None,
            capture,
            _subscription: subscription,
            _tasks: Vec::new(),
        };
        panel.refresh(cx);
        panel
    }

    pub fn focus_capture(&self, window: &mut Window, cx: &mut App) {
        self.capture.update(cx, |c, cx| c.focus(window, cx));
    }

    pub fn set_active(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active != id {
            self.active = id;
            cx.notify();
        }
    }

    pub fn get(&self, id: &str) -> Option<&Spark> {
        self.sparks.iter().find(|s| s.id == id)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.loading = true;
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { client.sparks() }).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(mut sparks) => {
                        sparks.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
                        this.sparks = sparks;
                        this.error = None;
                    }
                    Err(e) if e.is_auth() => cx.emit(SparksEvent::AuthFailed(e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Quick capture: the text is kept on screen until the server has it.
    pub fn create(&mut self, body: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let restore = body.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.create_spark(&body) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(spark) => {
                        this.sparks.insert(0, spark);
                        this.error = None;
                    }
                    Err(e) => this.error = Some(format!("Not captured: {e} — “{restore}”")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn upsert(&mut self, spark: Spark, cx: &mut Context<Self>) {
        self.sparks.retain(|s| s.id != spark.id);
        self.sparks.insert(0, spark);
        cx.notify();
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.sparks.retain(|s| s.id != id);
        cx.notify();
    }
}

impl Render for SparksPanel {
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("SPARKS"))
                    .child(div().child(format!("{}", self.sparks.len())))
                    .child(div().flex_1())
                    .when(self.loading, |this| this.child(Spinner::new().xsmall().color(theme.muted_foreground)))
                    .child(
                        Button::new("sparks-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(div().px_2().pb_2().child(Input::new(&self.capture).small()))
            .when_some(self.error.clone(), |this, e| {
                this.child(div().mx_2().mb_2().px_2().py_1p5().rounded(theme.radius).bg(theme.danger.opacity(0.12)).text_xs().child(e))
            })
            .child(
                v_flex()
                    .id("sparks-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(self.sparks.iter().map(|spark| {
                        let id = spark.id.clone();
                        let active = self.active.as_deref() == Some(id.as_str());
                        let first = spark.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned();
                        h_flex()
                            .id(SharedString::from(format!("spark-{id}")))
                            .min_h(px(26.))
                            .px_3()
                            .gap_2()
                            .text_sm()
                            .cursor_pointer()
                            .when(active, |this| this.bg(theme.list_active))
                            .hover(|this| this.bg(theme.list_hover))
                            .child(Icon::new(Lucide::Lightbulb).xsmall().text_color(theme.muted_foreground))
                            .child(div().flex_1().min_w_0().truncate().child(first))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(ago_short(spark.updated_at)))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(Open::Spark { id: id.clone() })))
                    }))
                    .when(self.sparks.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .px_4()
                                .pt_6()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No sparks. Anything you type above lands here — ⌘J does it from anywhere."),
                        )
                    }),
            )
    }
}

// -------------------------------------------------------------------- view

pub enum SparkViewEvent {
    Saved(Spark),
    Deleted(String),
    /// Turn it into a task in the current organisation.
    MakeTask(Spark),
}

impl EventEmitter<ItemEvent> for SparkView {}
impl EventEmitter<SparkViewEvent> for SparkView {}

pub struct SparkView {
    client: Arc<ApiClient>,
    spark: Spark,
    body: Entity<TextareaState>,
    dirty: bool,
    saving: bool,
    error: Option<String>,
    debounce: Option<Task<()>>,
    _subscription: Subscription,
    _tasks: Vec<Task<()>>,
}

impl SparkView {
    pub fn new(
        client: Arc<ApiClient>,
        spark: Spark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .default_value(spark.body.clone())
        });
        let subscription = cx.subscribe_in(
            &body,
            window,
            |this: &mut Self, _, e: &InputEvent, _, cx| {
                if let InputEvent::Change = e {
                    this.dirty = true;
                    cx.emit(ItemEvent::Edited);
                    this.debounce = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(SAVE_DEBOUNCE).await;
                        this.update(cx, |this, cx| this.save(cx)).ok();
                    }));
                    cx.notify();
                }
            },
        );
        Self {
            client,
            spark,
            body,
            dirty: false,
            saving: false,
            error: None,
            debounce: None,
            _subscription: subscription,
            _tasks: Vec::new(),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.dirty || self.saving {
            return;
        }
        let text = self.body.read(cx).value().to_string();
        if text.trim().is_empty() {
            return; // an empty spark is refused; keep the old text on the server
        }
        self.dirty = false;
        self.saving = true;
        let client = self.client.clone();
        let id = self.spark.id.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.update_spark(&id, &text) })
                .await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(spark) => {
                        this.spark = spark.clone();
                        this.error = None;
                        cx.emit(SparkViewEvent::Saved(spark));
                        if this.dirty {
                            this.save(cx);
                        }
                    }
                    Err(e) => {
                        this.dirty = true;
                        this.error = Some(format!("Not saved — {e}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn delete_now(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let id = self.spark.id.clone();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let delete_id = id.clone();
            let result = cx
                .background_spawn(async move { client.delete_spark(&delete_id) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(()) | Err(ApiError::NotFound(_)) => {
                    this.dirty = false;
                    cx.emit(SparkViewEvent::Deleted(id));
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

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Delete this spark?")
                .description("Sparks have no bin.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    this.update(cx, |this, cx| this.delete_now(cx)).ok();
                    true
                })
        });
    }

    fn make_task(&mut self, cx: &mut Context<Self>) {
        let mut spark = self.spark.clone();
        spark.body = self.body.read(cx).value().to_string();
        cx.emit(SparkViewEvent::MakeTask(spark));
    }
}

impl Focusable for SparkView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.body.read(cx).focus_handle(cx)
    }
}

impl Item for SparkView {
    fn item_key(&self, _: &App) -> SharedString {
        format!("spark:{}", self.spark.id).into()
    }

    fn tab_title(&self, cx: &App) -> SharedString {
        let body = self.body.read(cx).value();
        let first = body
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("Spark");
        first.chars().take(40).collect::<String>().into()
    }

    fn tab_icon(&self) -> Icon {
        Icon::new(Lucide::Lightbulb)
    }

    fn is_dirty(&self, _: &App) -> bool {
        self.dirty || self.saving
    }

    fn status_text(&self, _: &App) -> Option<SharedString> {
        Some(
            if self.saving {
                "Saving…".to_owned()
            } else if self.dirty {
                "Edited".to_owned()
            } else {
                format!(
                    "Saved {} · only you can see sparks",
                    ago_long(self.spark.updated_at)
                )
            }
            .into(),
        )
    }

    fn flush(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.debounce = None;
        self.save(cx);
    }
}

impl Render for SparkView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .bg(theme.background)
            .px_6()
            .py_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(Lucide::Lightbulb)
                            .small()
                            .text_color(theme.yellow),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Spark"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("make-task")
                            .icon(Lucide::SquareCheck)
                            .label("Make a task")
                            .small()
                            .tooltip("Create a task from it here and remove the spark")
                            .on_click(cx.listener(|this, _, _, cx| this.make_task(cx))),
                    )
                    .child(
                        Button::new("delete")
                            .icon(Lucide::Trash)
                            .ghost()
                            .small()
                            .tooltip("Delete spark")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_delete(window, cx)),
                            ),
                    ),
            )
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
            .child(
                div().flex_1().min_h_0().child(
                    Textarea::new(&self.body)
                        .appearance(false)
                        .h_full()
                        .text_size(px(16.))
                        .line_height(relative(1.6)),
                ),
            )
    }
}
