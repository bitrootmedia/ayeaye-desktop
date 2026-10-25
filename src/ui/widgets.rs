//! Small pieces shared by the module views.

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Theme, h_flex};
use gpui_kit::*;

use crate::api::models::priority_rank;

/// The priority's colour: a dot, never the words — the web app's rule for
/// keeping text readable.
pub fn priority_color(priority: &str, theme: &Theme) -> Hsla {
    match priority_rank(priority) {
        0 => theme.red,
        1 => theme.yellow,
        2 => theme.blue,
        3 => theme.muted_foreground.opacity(0.6),
        _ => theme.muted_foreground.opacity(0.3),
    }
}

pub fn dot(color: Hsla) -> Div {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

/// A section label in a sidebar: small caps, muted.
pub fn section_label(label: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_3()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label.into())
}

/// A body from the server: sanitised HTML, or — for old rows with no `<`
/// in them — plain text, shown with its line breaks.
pub fn rich_text(id: impl Into<ElementId>, body: &str) -> AnyElement {
    if body.contains('<') {
        TextView::html(id, body.to_owned())
            .selectable(true)
            .into_any_element()
    } else {
        div()
            .whitespace_normal()
            .children(
                body.lines()
                    .map(|l| div().min_h(px(18.)).child(l.to_owned())),
            )
            .into_any_element()
    }
}

/// HTML from the server as markdown to edit. The server takes markdown back
/// (`*_format: markdown`), so this is the whole round trip.
pub fn html_to_markdown(html: &str) -> String {
    if !html.contains('<') {
        return html.to_owned();
    }
    htmd::convert(html).unwrap_or_else(|_| html.to_owned())
}

/// A ghost button that opens a menu of `(key, label)` choices.
pub fn picker(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    options: Vec<(String, String)>,
    selected: Option<String>,
    on_pick: impl Fn(String, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_pick = std::rc::Rc::new(on_pick);
    Button::new(id)
        .label(label)
        .ghost()
        .xsmall()
        .dropdown_menu(move |mut menu, _, _| {
            for (key, label) in &options {
                let on_pick = on_pick.clone();
                let key = key.clone();
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(selected.as_deref() == Some(key.as_str()))
                        .on_click(move |_, window, cx| on_pick(key.clone(), window, cx)),
                );
            }
            menu.max_h(px(360.)).scrollable(true)
        })
}

/// A label and a value on one line, for a task's fields.
pub fn field_row(label: &'static str, value: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .min_h(px(28.))
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(value)
}

// ------------------------------------------------------------------- dates

/// The quick choices a reminder offers, as `(label, YYYY-MM-DD)`.
pub fn quick_dates() -> Vec<(&'static str, String)> {
    let today = Local::now().date_naive();
    let monday = next_weekday(today, Weekday::Mon);
    vec![
        ("Today", fmt_day(today)),
        ("Tomorrow", fmt_day(today + Duration::days(1))),
        ("Next Monday", fmt_day(monday)),
        ("In a week", fmt_day(today + Duration::days(7))),
    ]
}

fn next_weekday(from: NaiveDate, weekday: Weekday) -> NaiveDate {
    let mut d = from + Duration::days(1);
    while d.weekday() != weekday {
        d += Duration::days(1);
    }
    d
}

pub fn fmt_day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// `YYYY-MM-DD` → "Today", "Tomorrow", "Mon 3 Nov", "3 Nov 2027".
pub fn human_day(day: &str) -> String {
    let Ok(d) = NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
        return day.to_owned();
    };
    let today = Local::now().date_naive();
    match (d - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        2..=6 => d.format("%a %-d %b").to_string(),
        _ if d.year() == today.year() => d.format("%-d %b").to_string(),
        _ => d.format("%-d %b %Y").to_string(),
    }
}

/// Accepts what a person types for a date: `2026-11-03`, `3.11.2026`,
/// `3/11`, `+3` (days from today).
pub fn parse_day(input: &str) -> Option<String> {
    let s = input.trim();
    let today = Local::now().date_naive();
    if let Some(n) = s.strip_prefix('+') {
        return n
            .trim()
            .parse::<i64>()
            .ok()
            .map(|n| fmt_day(today + Duration::days(n)));
    }
    for fmt in ["%Y-%m-%d", "%d.%m.%Y", "%d/%m/%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(fmt_day(d));
        }
    }
    for sep in ['.', '/'] {
        let parts: Vec<&str> = s.split(sep).filter(|p| !p.is_empty()).collect();
        if let [d, m] = parts.as_slice() {
            let (d, m) = (d.parse().ok()?, m.parse().ok()?);
            let mut date = NaiveDate::from_ymd_opt(today.year(), m, d)?;
            if date < today {
                date = NaiveDate::from_ymd_opt(today.year() + 1, m, d)?;
            }
            return Some(fmt_day(date));
        }
    }
    None
}

/// Seconds as `1:05:09` or `5:09`.
pub fn clock(seconds: i64) -> String {
    let s = seconds.max(0);
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn dates_parse_the_ways_people_type_them() {
        assert_eq!(parse_day("2026-11-03").as_deref(), Some("2026-11-03"));
        assert_eq!(parse_day("3.11.2026").as_deref(), Some("2026-11-03"));
        let today = Local::now().date_naive();
        assert_eq!(parse_day("+1"), Some(fmt_day(today + Duration::days(1))));
        assert!(parse_day("31/2").is_none());
        assert!(parse_day("soon").is_none());
    }

    #[::core::prelude::v1::test]
    fn clocks() {
        assert_eq!(clock(5 * 60 + 9), "5:09");
        assert_eq!(clock(3600 + 5 * 60 + 9), "1:05:09");
    }

    #[::core::prelude::v1::test]
    fn markdown_round_trip_starts_from_html() {
        assert_eq!(html_to_markdown("plain\ntext"), "plain\ntext");
        assert!(html_to_markdown("<p><strong>bold</strong></p>").contains("**bold**"));
    }
}
