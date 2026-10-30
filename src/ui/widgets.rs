//! Small pieces shared by the module views.

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Theme, h_flex, v_flex};
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

/// A body from the server: sanitised HTML, or — for rows written before the
/// server converted markdown on the way in — the text as it was sent. That
/// text is often markdown (an agent's `## Steps`), so it renders as markdown
/// when it looks like it, and as plain lines, line breaks kept, when not.
///
/// HTML is shown through the markdown renderer too, converted first: GPUI
/// Kit's HTML view loses any paragraph, list item or code block with inline
/// `<code>` in it to a grey placeholder bar, and the server's tag set is
/// exactly what markdown can say.
pub fn rich_text(id: impl Into<ElementId>, body: &str) -> AnyElement {
    if is_html(body) {
        markdown_text(id, &html_body_as_markdown(body))
    } else if looks_like_markdown(body) {
        markdown_text(id, body)
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

/// Converted once per distinct body, not once per frame.
fn html_body_as_markdown(html: &str) -> std::rc::Rc<str> {
    use std::collections::HashMap;
    use std::hash::{DefaultHasher, Hash, Hasher};
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<u64, std::rc::Rc<str>>> = Default::default();
    }
    let mut hasher = DefaultHasher::new();
    html.hash(&mut hasher);
    let key = hasher.finish();
    CACHE.with_borrow_mut(|cache| {
        if let Some(hit) = cache.get(&key) {
            return hit.clone();
        }
        if cache.len() > 256 {
            cache.clear();
        }
        let markdown: std::rc::Rc<str> = split_markdown_tables(html)
            .into_iter()
            .map(|segment| match segment {
                Segment::Html(html) => html_to_markdown(&html),
                Segment::Table(table) => table,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
            .into();
        cache.insert(key, markdown.clone());
        markdown
    })
}

/// Markdown rendered: a body that never went through the server's
/// conversion, or an editor's draft in preview.
///
/// Sized as the web app's `.prose-task` (index.css), so a body reads the
/// same in both: 14px text at 1.625, 12px between blocks, `h2` at 16px and
/// `h3` at 14px, code blocks at 12px.
pub fn markdown_text(id: impl Into<ElementId>, markdown: &str) -> AnyElement {
    let style = TextViewStyle::default()
        .paragraph_gap(rems(0.75))
        .heading_font_size(|level, _| if level <= 2 { px(16.) } else { px(14.) })
        .code_block(StyleRefinement::default().text_size(px(12.)));
    TextView::markdown(id, markdown.to_owned())
        .style(style)
        .selectable(true)
        .text_size(px(14.))
        .line_height(relative(1.625))
        .into_any_element()
}

/// A stretch of an HTML body, for rendering.
#[derive(Debug, PartialEq)]
enum Segment {
    Html(String),
    /// A markdown table the server couldn't keep: its allow-list has no
    /// table tags and its markdown conversion no table extension, so
    /// `| a | b |` lines arrive as one `<p>`, pipes and all — and converting
    /// that paragraph would fold its lines into one.
    Table(String),
}

fn split_markdown_tables(html: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut pending = String::new();
    let mut rest = html;
    while let Some(start) = rest.find("<p>") {
        let Some(len) = rest[start..].find("</p>") else {
            break;
        };
        let end = start + len + 4;
        match markdown_table(&rest[start + 3..start + len]) {
            Some(table) => {
                pending.push_str(&rest[..start]);
                if !pending.trim().is_empty() {
                    segments.push(Segment::Html(std::mem::take(&mut pending)));
                }
                pending.clear();
                segments.push(Segment::Table(table));
            }
            None => pending.push_str(&rest[..end]),
        }
        rest = &rest[end..];
    }
    pending.push_str(rest);
    if !pending.trim().is_empty() {
        segments.push(Segment::Html(pending));
    }
    segments
}

/// `| h | h |` / `|---|---|` / rows inside a paragraph, back to markdown;
/// each cell's inline markup (`<code>`, `<strong>`) converted with it.
fn markdown_table(inner: &str) -> Option<String> {
    let lines: Vec<&str> = inner
        .split('\n')
        .flat_map(|l| l.split("<br>"))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 2 || !lines.iter().all(|l| l.starts_with('|')) {
        return None;
    }
    let separator =
        lines[1].chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && lines[1].contains('-');
    if !separator {
        return None;
    }
    let row = |line: &str| {
        let line = line.trim_start_matches('|');
        let line = line.strip_suffix('|').unwrap_or(line);
        let cells: Vec<String> = line
            .split('|')
            .map(|c| html_to_markdown(c.trim()).replace('\n', " "))
            .collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![row(lines[0]), lines[1].to_owned()];
    out.extend(lines[2..].iter().map(|l| row(l)));
    Some(out.join("\n"))
}

/// Markup, not just a `<`: `a < b` or `x<y` in markdown is not a tag.
fn is_html(body: &str) -> bool {
    let bytes = body.as_bytes();
    bytes
        .windows(2)
        .any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || w[1] == b'/' || w[1] == b'!'))
        && body.contains('>')
}

/// A body as one line of plain text, for a preview: tags dropped, the
/// commonest entities decoded, whitespace folded.
pub fn plain_line(body: &str) -> String {
    let text = if is_html(body) {
        let mut out = String::with_capacity(body.len());
        let mut in_tag = false;
        for c in body.chars() {
            match c {
                '<' => {
                    in_tag = true;
                    out.push(' ');
                }
                '>' if in_tag => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&nbsp;", " ")
    } else {
        body.to_owned()
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Markdown syntax a plain sentence wouldn't have by accident. Plain text
/// shouldn't go through markdown: its single line breaks would fold into
/// one paragraph.
fn looks_like_markdown(body: &str) -> bool {
    let block = body.lines().any(|line| {
        let l = line.trim_start();
        let heading = l.trim_start_matches('#');
        (heading.len() < l.len() && l.len() - heading.len() <= 6 && heading.starts_with(' '))
            || ["- ", "* ", "+ ", "> ", "```", "|"]
                .iter()
                .any(|p| l.starts_with(p))
            || l.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    block || body.contains("**") || body.contains('`') || body.contains("](")
}

/// HTML from the server as markdown to edit. The server takes markdown back
/// (`*_format: markdown`), so this is the whole round trip.
pub fn html_to_markdown(html: &str) -> String {
    if !is_html(html) {
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

/// A small label over its value, for one cell of a task's field grid.
pub fn field_cell(label: &'static str, value: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap_0p5()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(h_flex().gap_1p5().min_h(px(24.)).child(value))
}

/// The workflow status's colour, for its pill.
pub fn status_color(status: &str, theme: &Theme) -> Hsla {
    match status {
        "in_progress" => theme.blue,
        "review" => theme.magenta,
        "on_hold" => theme.yellow,
        "blocker" => theme.red,
        _ => theme.muted_foreground,
    }
}

/// A person's initials on a colour of their own, the same every time.
pub fn avatar(name: &str, size: Pixels, theme: &Theme) -> Div {
    let palette = [
        theme.blue,
        theme.green,
        theme.magenta,
        theme.cyan,
        theme.yellow,
        theme.red,
    ];
    let hash = name
        .bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    let color = palette[hash % palette.len()];
    let initials: String = name
        .split(|c: char| c.is_whitespace() || c == '.' || c == '@')
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    div()
        .flex_none()
        .size(size)
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(color.opacity(0.18))
        .text_color(color)
        .text_size(size * 0.42)
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials)
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
    fn previews_are_one_plain_line() {
        assert_eq!(
            plain_line("<p>Keel &amp; <strong>hull</strong></p>\n<p>ok</p>"),
            "Keel & hull ok"
        );
        assert_eq!(plain_line("a < b\n  and c"), "a < b and c");
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
        assert_eq!(html_to_markdown("if a < b"), "if a < b");
    }

    #[::core::prelude::v1::test]
    fn markdown_tables_in_paragraphs_become_tables() {
        let html = "<h2>Tags</h2>\n<p>| Tag | Use for |\n|---|---|\n| <code>app:NAME</code> | the app |</p>\n<p>after | this</p>";
        assert_eq!(
            split_markdown_tables(html),
            vec![
                Segment::Html("<h2>Tags</h2>\n".into()),
                Segment::Table("| Tag | Use for |\n|---|---|\n| `app:NAME` | the app |".into()),
                Segment::Html("\n<p>after | this</p>".into()),
            ]
        );
        let plain = "<p>| not a table</p><p>a</p>";
        assert_eq!(
            split_markdown_tables(plain),
            vec![Segment::Html(plain.into())]
        );
    }

    #[::core::prelude::v1::test]
    fn html_bodies_read_as_markdown() {
        let html = "<h2>Step 1</h2>\n<p>members of <code>group:admins</code></p>\n<ol>\n<li><strong>Get the key</strong></li>\n</ol>\n<p>| a | b |\n|---|---|\n| <code>x</code> | y |</p>";
        let md = html_body_as_markdown(html);
        for part in [
            "## Step 1",
            "members of `group:admins`",
            "1.  **Get the key**",
            "| a | b |\n|---|---|\n| `x` | y |",
        ] {
            assert!(md.contains(part), "{part:?} not in {md:?}");
        }
    }

    #[::core::prelude::v1::test]
    fn bodies_are_told_apart() {
        assert!(is_html("<p>hi</p>"));
        assert!(!is_html("a < b and c > d"));
        assert!(!is_html("x<5"));
        for md in [
            "## Steps\nrun it",
            "intro\n- one\n- two",
            "1. first",
            "use `make`",
            "**bold**",
            "[a](b)",
        ] {
            assert!(looks_like_markdown(md), "{md}");
        }
        for plain in ["just a note\nsecond line", "#hashtag", "Version 2.0 is out"] {
            assert!(!looks_like_markdown(plain), "{plain}");
        }
    }
}
