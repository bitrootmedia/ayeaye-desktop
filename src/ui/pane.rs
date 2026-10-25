//! Zed-style tabs: one strip, one visible item, preview tabs.
//!
//! **Preview tabs.** Single-clicking through the notes list opens each note in
//! the same italic "preview" tab instead of leaving a trail of tabs behind;
//! typing in it, or double-clicking the row, keeps it. It is what makes
//! browsing cheap while working on several notes stays explicit.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::item::{ItemEvent, ItemHandle};

pub enum PaneEvent {
    /// Tabs opened, closed, reordered or activated — the workspace saves them.
    Changed,
    /// The empty state's "New note" was pressed.
    NewItemRequested,
}

impl EventEmitter<PaneEvent> for Pane {}

struct Tab {
    item: Box<dyn ItemHandle>,
    _subscriptions: Vec<Subscription>,
}

pub struct Pane {
    tabs: Vec<Tab>,
    active: usize,
    preview: Option<EntityId>,
    /// Keys of recently closed tabs, for ⌘⇧T.
    closed: Vec<SharedString>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
}

/// Dragging a tab carries its index and title.
#[derive(Clone)]
pub struct DraggedTab {
    from: usize,
    title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().tab_active)
            .border_1()
            .border_color(cx.theme().input)
            .text_sm()
            .child(self.title.clone())
    }
}

impl Pane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            preview: None,
            closed: Vec::new(),
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        }
    }

    pub fn active_item(&self) -> Option<&dyn ItemHandle> {
        self.tabs.get(self.active).map(|t| t.item.as_ref())
    }

    pub fn items(&self) -> impl Iterator<Item = &dyn ItemHandle> {
        self.tabs.iter().map(|t| t.item.as_ref())
    }

    pub fn find(&self, key: &str, cx: &App) -> Option<usize> {
        self.tabs.iter().position(|t| t.item.item_key(cx) == key)
    }

    pub fn pop_closed(&mut self) -> Option<SharedString> {
        self.closed.pop()
    }

    /// Opens `item`, or focuses the tab already showing the same key.
    ///
    /// With `preview`, the item replaces the current preview tab (unless that
    /// one has unsaved edits) instead of adding a new one.
    pub fn add_item(
        &mut self,
        item: Box<dyn ItemHandle>,
        preview: bool,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = item.item_key(cx);
        if let Some(ix) = self.find(&key, cx) {
            if !preview && self.preview == Some(self.tabs[ix].item.item_id()) {
                self.preview = None;
            }
            self.activate(ix, focus, window, cx);
            return;
        }

        let mut insert_at = if self.tabs.is_empty() {
            0
        } else {
            self.active + 1
        };
        if preview
            && let Some(ix) = self.preview_index()
            && !self.tabs[ix].item.is_dirty(cx)
        {
            self.remove_tab(ix, false, window, cx);
            insert_at = ix;
        }
        let insert_at = insert_at.min(self.tabs.len());
        let subscriptions = item.subscribe_events(window, cx);
        if preview {
            self.preview = Some(item.item_id());
        }
        self.tabs.insert(
            insert_at,
            Tab {
                item,
                _subscriptions: subscriptions,
            },
        );
        self.activate(insert_at, focus, window, cx);
    }

    fn preview_index(&self) -> Option<usize> {
        let id = self.preview?;
        self.tabs.iter().position(|t| t.item.item_id() == id)
    }

    pub fn keep_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview.take().is_some() {
            cx.notify();
        }
    }

    pub fn activate(
        &mut self,
        ix: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        self.active = ix.min(self.tabs.len() - 1);
        self.scroll.scroll_to_item(self.active);
        if focus {
            self.tabs[self.active].item.focus(window, cx);
        }
        cx.emit(PaneEvent::Changed);
        cx.notify();
    }

    pub fn activate_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + 1) % self.tabs.len(), true, window, cx);
        }
    }

    pub fn activate_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            let len = self.tabs.len();
            self.activate((self.active + len - 1) % len, true, window, cx);
        }
    }

    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.close(self.active, window, cx);
        }
    }

    pub fn close_item(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.item.item_id() == id) {
            self.close(ix, window, cx);
        }
    }

    pub fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.focus_handle.contains_focused(window, cx);
        self.remove_tab(ix, true, window, cx);
        if self.tabs.is_empty() {
            if had_focus {
                window.focus(&self.focus_handle, cx);
            }
        } else {
            // Zed moves to the tab on the left of the one that closed.
            let next = if ix <= self.active && self.active > 0 {
                self.active - 1
            } else {
                self.active
            };
            self.activate(next, had_focus, window, cx);
        }
        cx.emit(PaneEvent::Changed);
        cx.notify();
    }

    /// Closes every tab, saving what each still holds. Used when switching
    /// organisation or profile.
    pub fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for tab in self.tabs.drain(..) {
            tab.item.flush(window, cx);
        }
        self.active = 0;
        self.preview = None;
        self.closed.clear();
        cx.notify();
    }

    fn remove_tab(
        &mut self,
        ix: usize,
        remember: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tabs.remove(ix);
        tab.item.flush(window, cx);
        if remember {
            self.closed.push(tab.item.item_key(cx));
        }
        if self.preview == Some(tab.item.item_id()) {
            self.preview = None;
        }
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        } else if ix < self.active {
            self.active -= 1;
        }
    }

    pub fn flush_all(&self, window: &mut Window, cx: &mut App) {
        for tab in &self.tabs {
            tab.item.flush(window, cx);
        }
    }

    pub fn handle_item_event(
        &mut self,
        id: EntityId,
        event: &ItemEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ItemEvent::Edited => {
                if self.preview == Some(id) {
                    self.preview = None;
                    cx.emit(PaneEvent::Changed);
                    cx.notify();
                }
            }
            ItemEvent::CloseRequested => {
                if let Some(ix) = self.tabs.iter().position(|t| t.item.item_id() == id) {
                    // Nothing to remember: the item is gone, not just closed.
                    self.remove_tab(ix, false, window, cx);
                    if !self.tabs.is_empty() {
                        self.activate(self.active, false, window, cx);
                    }
                    cx.emit(PaneEvent::Changed);
                    cx.notify();
                }
            }
        }
    }

    fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.tabs.len() {
            return;
        }
        let active_id = self.tabs[self.active].item.item_id();
        let tab = self.tabs.remove(from);
        let to = to.min(self.tabs.len());
        self.tabs.insert(to, tab);
        self.active = self
            .tabs
            .iter()
            .position(|t| t.item.item_id() == active_id)
            .unwrap_or(0);
        cx.emit(PaneEvent::Changed);
        cx.notify();
    }

    fn render_tab(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let tab = &self.tabs[ix];
        let item = &tab.item;
        let active = ix == self.active;
        let preview = self.preview == Some(item.item_id());
        let dirty = item.is_dirty(cx);
        let title = item.tab_title(cx);
        let id = item.item_id();
        let group: SharedString = format!("tab-{id}").into();

        let (bg, fg) = if active {
            (theme.tab_active, theme.tab_active_foreground)
        } else {
            (theme.tab, theme.tab_foreground)
        };
        let muted = theme.muted_foreground;
        let border = theme.border;
        let hover_bg = theme.accent;
        let drop_bg = theme.drop_target;

        h_flex()
            .id(("tab", id.as_u64()))
            .group(group.clone())
            .flex_none()
            .h_full()
            .gap_1p5()
            .pl_3()
            .pr_1()
            .max_w(px(220.))
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .border_r_1()
            .border_color(border)
            .when(!active, |this| this.border_b_1())
            .cursor_pointer()
            .child(
                item.tab_icon(cx)
                    .small()
                    .text_color(if active { fg } else { muted }),
            )
            .child(
                div()
                    .truncate()
                    .when(preview, |this| this.italic())
                    .child(title.clone()),
            )
            .child(
                // The close button and the unsaved dot share one slot, as in
                // Zed: the dot until you hover, then the ×.
                div()
                    .id(("close", id.as_u64()))
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .hover(|this| this.bg(hover_bg))
                    .child(
                        div()
                            .when(dirty, |this| {
                                this.group_hover(group.clone(), |s| s.hidden())
                            })
                            .when(!dirty, |this| this.hidden())
                            .size(px(7.))
                            .rounded_full()
                            .bg(fg),
                    )
                    .child(
                        div()
                            .when(dirty || !active, |this| {
                                this.invisible().group_hover(group.clone(), |s| s.visible())
                            })
                            .when(dirty, |this| this.absolute())
                            .child(Icon::new(IconName::Close).xsmall().text_color(muted)),
                    )
                    .on_click(cx.listener(move |pane, _, window, cx| {
                        cx.stop_propagation();
                        pane.close_item(id, window, cx);
                    })),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |pane, _, window, cx| pane.close_item(id, window, cx)),
            )
            .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 && pane.preview == Some(id) {
                    pane.keep_preview(cx);
                }
                if let Some(ix) = pane.tabs.iter().position(|t| t.item.item_id() == id) {
                    pane.activate(ix, true, window, cx);
                }
            }))
            .on_drag(
                DraggedTab {
                    from: ix,
                    title: title.clone(),
                },
                |dragged, _, _, cx| cx.new(|_| dragged.clone()),
            )
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop_bg))
            .on_drop(cx.listener(move |pane, dragged: &DraggedTab, _, cx| {
                pane.move_tab(dragged.from, ix, cx);
            }))
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let row = |label: &'static str, keys: &'static str| {
            h_flex()
                .w(px(260.))
                .justify_between()
                .child(div().text_color(muted).child(label))
                .child(
                    gpui_kit::component::kbd::Kbd::new(Keystroke::parse(keys).unwrap())
                        .appearance(false),
                )
        };
        v_flex()
            .id("empty-pane")
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .bg(theme.background)
            .child(row("New note", "secondary-n"))
            .child(row("Find a note", "secondary-p"))
            .child(row("Toggle sidebar", "secondary-b"))
            .child(row("Reopen closed tab", "secondary-shift-t"))
    }
}

impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.tabs.is_empty() {
            return v_flex()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(self.render_empty(cx))
                .into_any_element();
        }

        let tabs: Vec<_> = (0..self.tabs.len())
            .map(|ix| self.render_tab(ix, cx))
            .collect();
        let end = self.tabs.len();
        let active_view = self.tabs[self.active].item.to_any_view();

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_none()
                    .bg(theme.tab_bar)
                    .child(
                        h_flex()
                            .id("tabs")
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .track_scroll(&self.scroll)
                            .children(tabs)
                            .child(
                                // The empty strip after the last tab: a drop
                                // target for "move to the end", and a border.
                                div()
                                    .id("tab-bar-end")
                                    .flex_1()
                                    .min_w(px(24.))
                                    .h_full()
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .drag_over::<DraggedTab>({
                                        let drop_target = theme.drop_target;
                                        move |style, _, _, _| style.bg(drop_target)
                                    })
                                    .on_drop(cx.listener(
                                        move |pane, dragged: &DraggedTab, _, cx| {
                                            pane.move_tab(dragged.from, end, cx);
                                        },
                                    )),
                            ),
                    )
                    .child(
                        h_flex()
                            .h_full()
                            .px_1()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                gpui_kit::component::button::Button::new("pane-new")
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("New note")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(PaneEvent::NewItemRequested)
                                    })),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(active_view))
            .into_any_element()
    }
}
