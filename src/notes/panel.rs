//! The notes list in the sidebar — Zed's project panel, for notes.
//!
//! One line per note, most recently edited first, with a filter field that
//! matches every word you type against title and body. Single click previews,
//! double click (or Return) keeps the tab.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::store::{LoadState, NotesStore};
use crate::actions::{SelectNext, SelectPrev};
use crate::api::models::Note;
use crate::ui::format::ago_short;

const ROW_HEIGHT: f32 = 26.;

pub enum PanelEvent {
    /// `focus`: move the caret into the note. Arrowing through the list
    /// previews without taking focus from the filter.
    Open {
        id: String,
        preview: bool,
        focus: bool,
    },
    OpenNew {
        title: String,
    },
    Delete {
        id: String,
    },
    /// Escape in the filter: hand focus back to the editor.
    Dismiss,
}

impl EventEmitter<PanelEvent> for NotesPanel {}

pub struct NotesPanel {
    store: Entity<NotesStore>,
    filter: Entity<InputState>,
    /// Indices into the store's list, in display order.
    matches: Vec<usize>,
    selected: usize,
    active_note: Option<String>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl NotesPanel {
    pub fn new(store: Entity<NotesStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter notes…")
                .clean_on_escape()
        });
        let subscriptions = vec![
            cx.observe(&store, |this, _, cx| this.update_matches(cx)),
            cx.subscribe_in(&filter, window, Self::on_filter_event),
        ];
        let mut panel = Self {
            store,
            filter,
            matches: Vec::new(),
            selected: 0,
            active_note: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        panel.update_matches(cx);
        panel
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter.update(cx, |filter, cx| {
            filter.focus(window, cx);
            filter.select_all(window, cx);
        });
    }

    pub fn set_active_note(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.active_note != id {
            self.active_note = id;
            cx.notify();
        }
    }

    fn query(&self, cx: &App) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let terms: Vec<&str> = query.split_whitespace().collect();
        let notes = self.store.read(cx).notes();
        self.matches = notes
            .iter()
            .enumerate()
            .filter(|(_, note)| matches(note, &terms))
            .map(|(ix, _)| ix)
            .collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        cx.notify();
    }

    fn on_filter_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.selected = 0;
                self.update_matches(cx);
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            InputEvent::PressEnter { .. } => match self.selected_note(cx) {
                Some(note) => cx.emit(PanelEvent::Open {
                    id: note.id.clone(),
                    preview: false,
                    focus: true,
                }),
                // Nothing matches: the query becomes a new note's title —
                // "find it or start it" from one field.
                None => {
                    let title = self.filter.read(cx).value().trim().to_owned();
                    if !title.is_empty() {
                        cx.emit(PanelEvent::OpenNew { title });
                    }
                }
            },
            _ => {}
        }
    }

    fn selected_note<'a>(&self, cx: &'a App) -> Option<&'a Note> {
        let ix = *self.matches.get(self.selected)?;
        self.store.read(cx).notes().get(ix)
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1).min(self.matches.len() - 1);
            self.scroll
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
            self.preview_selected(cx);
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if !self.matches.is_empty() {
            self.selected = self.selected.saturating_sub(1);
            self.scroll
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
            self.preview_selected(cx);
        }
    }

    fn preview_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(note) = self.selected_note(cx) {
            let id = note.id.clone();
            cx.emit(PanelEvent::Open {
                id,
                preview: true,
                focus: false,
            });
        }
        cx.notify();
    }

    fn escape(&mut self, _: &crate::actions::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Dismiss);
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let notes = self.store.read(cx).notes().to_vec();
        range
            .filter_map(|row| {
                let note = notes.get(*self.matches.get(row)?)?.clone();
                let id = note.id.clone();
                let active = self.active_note.as_deref() == Some(id.as_str());
                let selected = row == self.selected && !self.filter.read(cx).value().is_empty();
                let title: SharedString = if note.title.trim().is_empty() {
                    super::editor::UNTITLED.into()
                } else {
                    note.title.clone().into()
                };
                let open_id = id.clone();
                let menu_open_id = id.clone();
                let menu_delete_id = id.clone();
                let this = cx.entity().downgrade();
                let this_menu = this.clone();
                Some(
                    h_flex()
                        .id(SharedString::from(format!("note-row-{id}")))
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px_3()
                        .gap_2()
                        .text_sm()
                        .cursor_pointer()
                        .when(active, |this| this.bg(theme.list_active))
                        .when(selected && !active, |this| this.bg(theme.list_hover))
                        .hover(|this| this.bg(theme.list_hover))
                        .child(
                            Icon::new(IconName::FileText)
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(ago_short(note.updated_at)),
                        )
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            this.selected = row;
                            cx.emit(PanelEvent::Open {
                                id: open_id.clone(),
                                preview: event.click_count() < 2,
                                focus: true,
                            });
                        }))
                        .context_menu(move |menu, _, _| {
                            let open_id = menu_open_id.clone();
                            let delete_id = menu_delete_id.clone();
                            let this_open = this_menu.clone();
                            let this_delete = this_menu.clone();
                            menu.item(PopupMenuItem::new("Open in new tab").on_click(
                                move |_, _, cx| {
                                    this_open
                                        .update(cx, |_, cx| {
                                            cx.emit(PanelEvent::Open {
                                                id: open_id.clone(),
                                                preview: false,
                                                focus: true,
                                            })
                                        })
                                        .ok();
                                },
                            ))
                            .separator()
                            .item(
                                PopupMenuItem::new("Delete note…").on_click(move |_, _, cx| {
                                    this_delete
                                        .update(cx, |_, cx| {
                                            cx.emit(PanelEvent::Delete {
                                                id: delete_id.clone(),
                                            })
                                        })
                                        .ok();
                                }),
                            )
                        })
                        .into_any_element(),
                )
            })
            .collect()
    }
}

/// Every term must appear in the title or the body, case-insensitively.
pub fn matches(note: &Note, terms: &[&str]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let title = note.title.to_lowercase();
    let body = note.body.to_lowercase();
    terms.iter().all(|t| title.contains(t) || body.contains(t))
}

impl Focusable for NotesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NotesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let store = self.store.read(cx);
        let state = store.state().clone();
        let total = store.notes().len();
        let has_notes = total > 0;
        let count = self.matches.len();
        let querying = !self.query(cx).is_empty();

        v_flex()
            .key_context("NotesPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::escape))
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("NOTES"))
                    .when(has_notes, |this| {
                        this.child(div().child(format!("{total}")))
                    })
                    .child(div().flex_1())
                    .when(state == LoadState::Loading, |this| {
                        this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                    })
                    .child(
                        Button::new("notes-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.store.update(cx, |store, cx| store.refresh(cx))
                            })),
                    ),
            )
            .child(
                div().px_2().pb_2().flex_none().child(
                    Input::new(&self.filter)
                        .small()
                        .prefix(
                            Icon::new(IconName::Search)
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        )
                        .cleanable(true),
                ),
            )
            .when_some(
                match &state {
                    LoadState::Failed(e) => Some(e.clone()),
                    _ => None,
                },
                |this, error| {
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
                },
            )
            .child(if count > 0 {
                uniform_list("notes-list", count, cx.processor(Self::render_rows))
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .into_any_element()
            } else {
                v_flex()
                    .flex_1()
                    .px_4()
                    .pt_6()
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if querying {
                        "No notes match. Return creates one with that title."
                    } else if state == LoadState::Loading {
                        "Loading…"
                    } else {
                        if cfg!(target_os = "macos") {
                            "No notes yet. ⌘N starts one."
                        } else {
                            "No notes yet. Ctrl+N starts one."
                        }
                    })
                    .into_any_element()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn note(title: &str, body: &str) -> Note {
        Note {
            id: "1".into(),
            title: title.into(),
            body: body.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[::core::prelude::v1::test] // not gpui's #[test], which `super::*` brings in
    fn every_term_must_match_title_or_body() {
        let n = note("Release checklist", "bump version, tag, notarise");
        assert!(matches(&n, &[]));
        assert!(matches(&n, &["release", "tag"]));
        assert!(matches(&n, &["checklist"]));
        assert!(!matches(&n, &["release", "deploy"]));
    }
}
