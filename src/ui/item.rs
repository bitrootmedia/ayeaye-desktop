//! Anything that can live in a tab.
//!
//! The pane knows nothing about notes. A note editor is one kind of item; a
//! task's detail view will be another, and opens in the same tabs. A module
//! makes its views items by implementing [`Item`]; the pane holds them as
//! [`ItemHandle`]s.

use gpui_kit::component::Icon;
use gpui_kit::*;

/// What an item tells its pane. Everything else the pane learns by
/// observing the item, which re-renders the tab on any change.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemEvent {
    /// The item was edited: a preview tab becomes a kept tab, as in Zed.
    Edited,
    /// The item asks to be closed, e.g. its note was deleted.
    CloseRequested,
}

pub trait Item: Render + Focusable + EventEmitter<ItemEvent> {
    /// Stable identity used to find an already-open tab, e.g. `note:<id>`.
    /// Survives restarts: it is what the workspace saves to reopen tabs.
    fn item_key(&self, cx: &App) -> SharedString;
    fn tab_title(&self, cx: &App) -> SharedString;
    fn tab_icon(&self) -> Icon;
    /// Unsaved changes, shown as a dot where the close button sits.
    fn is_dirty(&self, cx: &App) -> bool;
    /// One line for the status bar, e.g. "Saved 2 min ago".
    fn status_text(&self, _cx: &App) -> Option<SharedString> {
        None
    }
    /// Cursor position for the status bar, 1-based.
    fn cursor_position(&self, _cx: &App) -> Option<(u32, u32)> {
        None
    }
    /// Start saving anything pending, now. Called when the tab closes.
    fn flush(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
    /// For quitting: whatever is unsaved, as a blocking call the app runs on
    /// a background thread and waits for before it exits.
    fn take_blocking_save(&mut self, _cx: &mut Context<Self>) -> Option<Box<dyn FnOnce() + Send>> {
        None
    }
}

pub trait ItemHandle: 'static {
    fn item_id(&self) -> EntityId;
    fn item_key(&self, cx: &App) -> SharedString;
    fn tab_title(&self, cx: &App) -> SharedString;
    fn tab_icon(&self, cx: &App) -> Icon;
    fn is_dirty(&self, cx: &App) -> bool;
    fn status_text(&self, cx: &App) -> Option<SharedString>;
    fn cursor_position(&self, cx: &App) -> Option<(u32, u32)>;
    fn focus(&self, window: &mut Window, cx: &mut App);
    fn flush(&self, window: &mut Window, cx: &mut App);
    fn take_blocking_save(&self, cx: &mut App) -> Option<Box<dyn FnOnce() + Send>>;
    fn to_any_view(&self) -> AnyView;
    fn boxed_clone(&self) -> Box<dyn ItemHandle>;
    fn subscribe_events(
        &self,
        window: &mut Window,
        cx: &mut Context<super::pane::Pane>,
    ) -> Vec<Subscription>;
}

impl<T: Item> ItemHandle for Entity<T> {
    fn item_id(&self) -> EntityId {
        self.entity_id()
    }

    fn item_key(&self, cx: &App) -> SharedString {
        self.read(cx).item_key(cx)
    }

    fn tab_title(&self, cx: &App) -> SharedString {
        self.read(cx).tab_title(cx)
    }

    fn tab_icon(&self, cx: &App) -> Icon {
        self.read(cx).tab_icon()
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty(cx)
    }

    fn status_text(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).status_text(cx)
    }

    fn cursor_position(&self, cx: &App) -> Option<(u32, u32)> {
        self.read(cx).cursor_position(cx)
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.read(cx).focus_handle(cx), cx);
    }

    fn flush(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |item, cx| item.flush(window, cx));
    }

    fn take_blocking_save(&self, cx: &mut App) -> Option<Box<dyn FnOnce() + Send>> {
        self.update(cx, |item, cx| item.take_blocking_save(cx))
    }

    fn to_any_view(&self) -> AnyView {
        self.clone().into()
    }

    fn boxed_clone(&self) -> Box<dyn ItemHandle> {
        Box::new(self.clone())
    }

    fn subscribe_events(
        &self,
        window: &mut Window,
        cx: &mut Context<super::pane::Pane>,
    ) -> Vec<Subscription> {
        let id = self.entity_id();
        vec![
            cx.observe(self, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                self,
                window,
                move |pane, _, event: &ItemEvent, window, cx| {
                    pane.handle_item_event(id, event, window, cx)
                },
            ),
        ]
    }
}
