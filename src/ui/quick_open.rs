//! ⌘P: find a note by typing, or start one with that title.
//!
//! Zed's file finder, for notes. With nothing typed it lists the most recent
//! notes; typing narrows by every word, title matches first. The last row is
//! always "New note", so the same keystrokes that look for a note create it
//! when it isn't there.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{Cancel, SelectNext, SelectPrev};
use crate::api::models::Note;
use crate::notes::store::NotesStore;
use crate::ui::format::ago_short;

const MAX_RESULTS: usize = 12;

pub enum QuickOpenEvent {
    Open(String),
    Create(String),
    Dismissed,
}

impl EventEmitter<QuickOpenEvent> for QuickOpen {}

pub struct QuickOpen {
    store: Entity<NotesStore>,
    query: Entity<InputState>,
    results: Vec<Note>,
    selected: usize,
    _subscriptions: Vec<Subscription>,
}

impl QuickOpen {
    pub fn new(store: Entity<NotesStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Find a note, or type a title for a new one…")
        });
        query.update(cx, |q, cx| q.focus(window, cx));
        let subscriptions = vec![cx.subscribe_in(&query, window, Self::on_query_event)];
        let mut this = Self {
            store,
            query,
            results: Vec::new(),
            selected: 0,
            _subscriptions: subscriptions,
        };
        this.update_results(cx);
        this
    }

    fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).value().trim().to_owned()
    }

    fn update_results(&mut self, cx: &mut Context<Self>) {
        let query = self.query_text(cx);
        self.results = rank(self.store.read(cx).notes(), &query);
        self.selected = 0;
        cx.notify();
    }

    /// Rows are the results, then "New note" when something is typed.
    fn row_count(&self, cx: &App) -> usize {
        self.results.len() + usize::from(!self.query_text(cx).is_empty())
    }

    fn on_query_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.update_results(cx),
            InputEvent::PressEnter { .. } => self.confirm(self.selected, cx),
            _ => {}
        }
    }

    fn confirm(&mut self, row: usize, cx: &mut Context<Self>) {
        match self.results.get(row) {
            Some(note) => cx.emit(QuickOpenEvent::Open(note.id.clone())),
            None => {
                let title = self.query_text(cx);
                if !title.is_empty() {
                    cx.emit(QuickOpenEvent::Create(title));
                }
            }
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.row_count(cx);
        if count > 0 {
            self.selected = (self.selected + 1) % count;
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.row_count(cx);
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
            cx.notify();
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(QuickOpenEvent::Dismissed);
    }
}

/// Every word must appear in the title or body. Titles that start with the
/// query beat titles that contain it, which beat body-only matches; within a
/// rank, the most recently edited note wins (the store's order).
pub fn rank(notes: &[Note], query: &str) -> Vec<Note> {
    let query = query.to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut scored: Vec<(u8, usize, &Note)> = notes
        .iter()
        .enumerate()
        .filter(|(_, n)| crate::notes::panel::matches(n, &terms))
        .map(|(ix, n)| {
            let title = n.title.to_lowercase();
            let score = if terms.is_empty() {
                0
            } else if title.starts_with(&query) {
                3
            } else if title.contains(&query) {
                2
            } else if terms.iter().all(|t| title.contains(t)) {
                1
            } else {
                0
            };
            (score, ix, n)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, _, n)| n.clone())
        .collect()
}

/// The first body line containing a query word, for context under a title.
fn snippet(note: &Note, query: &str) -> Option<String> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let line = if terms.is_empty() {
        note.body.lines().find(|l| !l.trim().is_empty())
    } else {
        note.body
            .lines()
            .find(|l| terms.iter().any(|t| l.to_lowercase().contains(t)))
    }?;
    Some(line.trim().chars().take(120).collect())
}

impl Render for QuickOpen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let query = self.query_text(cx);
        let selected = self.selected;

        let rows = self.results.iter().enumerate().map(|(row, note)| {
            let snippet = snippet(note, &query);
            h_flex()
                .id(("qo-row", row))
                .px_3()
                .py_1p5()
                .gap_2()
                .rounded(theme.radius)
                .cursor_pointer()
                .when(row == selected, |this| this.bg(theme.list_active))
                .hover(|this| this.bg(theme.list_hover))
                .child(
                    Icon::new(IconName::FileText)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(if note.title.is_empty() {
                            "Untitled note".to_owned()
                        } else {
                            note.title.clone()
                        }))
                        .when_some(snippet, |this, s| {
                            this.child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(s),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(ago_short(note.updated_at)),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.confirm(row, cx)))
        });
        let create_row = (!query.is_empty()).then(|| {
            let row = self.results.len();
            h_flex()
                .id("qo-create")
                .px_3()
                .py_1p5()
                .gap_2()
                .rounded(theme.radius)
                .cursor_pointer()
                .when(row == selected, |this| this.bg(theme.list_active))
                .hover(|this| this.bg(theme.list_hover))
                .child(
                    Icon::new(IconName::Plus)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(div().text_sm().child(format!("New note “{query}”")))
                .on_click(cx.listener(move |this, _, _, cx| this.confirm(row, cx)))
        });

        v_flex()
            .key_context("QuickOpen")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(QuickOpenEvent::Dismissed)))
            .w(px(560.))
            .max_h(px(480.))
            .p_1()
            .gap_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.input)
            .bg(theme.popover)
            .shadow_lg()
            .child(Input::new(&self.query).appearance(false))
            .child(div().h(px(1.)).bg(theme.border))
            .child(
                v_flex()
                    .id("qo-results")
                    .overflow_y_scroll()
                    .children(rows)
                    .children(create_row)
                    .when(self.results.is_empty() && query.is_empty(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("No notes yet — type a title to start one."),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn note(id: &str, title: &str, body: &str, age_min: i64) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            created_at: Utc::now(),
            updated_at: Utc::now() - Duration::minutes(age_min),
        }
    }

    #[::core::prelude::v1::test] // not gpui's #[test], which `super::*` brings in
    fn title_prefix_beats_contains_beats_body() {
        // Store order: most recent first.
        let notes = vec![
            note("body", "Groceries", "deploy checklist inside", 1),
            note("contains", "Friday deploy", "", 2),
            note("prefix", "Deploy runbook", "", 3),
        ];
        let ids: Vec<_> = rank(&notes, "deploy").into_iter().map(|n| n.id).collect();
        assert_eq!(ids, ["prefix", "contains", "body"]);
    }

    #[::core::prelude::v1::test] // not gpui's #[test], which `super::*` brings in
    fn empty_query_keeps_recency_order() {
        let notes = vec![note("a", "A", "", 1), note("b", "B", "", 2)];
        let ids: Vec<_> = rank(&notes, "").into_iter().map(|n| n.id).collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
