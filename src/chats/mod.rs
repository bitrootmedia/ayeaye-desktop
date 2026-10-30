//! The Chats module: direct messages, groups, and every task and project
//! thread you have a stake in, in one list — the sidebar — with a chat in a
//! tab. A DM and a task's comments are the same conversation on the server,
//! so they open, send and update the same way here.

pub mod panel;
pub mod view;

use chrono::{DateTime, Local, Utc};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::models::{Chat, LastMessage};
use crate::assets::Lucide;
use crate::ui::widgets::{avatar, plain_line, status_color};

/// The list's second line: who said it, then what — "You: on my way",
/// "Sam: on my way", or just the words in a DM with the other person.
pub fn preview(chat: &Chat, last: &LastMessage) -> String {
    if last.deleted {
        return "Message deleted".into();
    }
    let body = plain_line(&last.body);
    let body = if body.is_empty() && last.attachments > 0 {
        "File".to_owned()
    } else {
        body
    };
    if last.mine {
        return format!("You: {body}");
    }
    match (&last.author, chat.kind.as_str()) {
        (Some(author), "group" | "task" | "project") => {
            let name = author.name();
            let first = name.split_whitespace().next().unwrap_or(&name).to_owned();
            format!("{first}: {body}")
        }
        _ => body,
    }
}

/// "online", "last seen 5 min ago", "last seen 3 Nov" — under a DM's name.
pub fn seen_line(online: bool, last_seen: Option<DateTime<Utc>>) -> String {
    if online {
        return "online".into();
    }
    match last_seen {
        Some(at) => format!("last seen {}", crate::ui::format::ago_long(at)),
        None => "offline".into(),
    }
}

/// "Today", "Yesterday", "Wednesday", "3 November", "3 November 2025" — the
/// separators between days in a chat.
pub fn day_label(at: DateTime<Utc>) -> String {
    let day = at.with_timezone(&Local).date_naive();
    let today = Local::now().date_naive();
    match (today - day).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.format("%Y").to_string() == today.format("%Y").to_string() => {
            day.format("%-d %B").to_string()
        }
        _ => day.format("%-d %B %Y").to_string(),
    }
}

/// What a chat looks like at a glance: the other person for a DM (with a
/// green dot while they're online), a mark of its kind otherwise.
pub fn chat_avatar(chat: &Chat, me: Option<&str>, size: Pixels, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let badge = |icon: Lucide, color: Hsla| {
        div()
            .flex_none()
            .size(size)
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(color.opacity(0.15))
            .child(Icon::new(icon).small().text_color(color))
            .into_any_element()
    };
    match chat.kind.as_str() {
        "direct" => match chat.other(me) {
            Some(other) => div()
                .relative()
                .flex_none()
                .child(avatar(&other.name(), size, theme))
                .when(other.online, |this| {
                    this.child(
                        div()
                            .absolute()
                            .bottom(px(-1.))
                            .right(px(-1.))
                            .size(px(10.))
                            .rounded_full()
                            .border_2()
                            .border_color(theme.sidebar)
                            .bg(theme.green),
                    )
                })
                .into_any_element(),
            None => badge(Lucide::Bookmark, theme.blue),
        },
        "group" => badge(Lucide::MessagesSquare, theme.magenta),
        "task" => {
            let color = chat
                .task
                .as_ref()
                .map_or(theme.muted_foreground, |t| status_color(&t.status, theme));
            badge(Lucide::SquareCheck, color)
        }
        _ => badge(Lucide::FolderKanban, theme.cyan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Person;

    #[::core::prelude::v1::test]
    fn previews_say_who() {
        let sam = Person {
            id: "u".into(),
            email: None,
            display_name: Some("Sam Vimes".into()),
        };
        let group = Chat {
            kind: "group".into(),
            ..Default::default()
        };
        let direct = Chat {
            kind: "direct".into(),
            ..Default::default()
        };
        let said = LastMessage {
            author: Some(sam),
            body: "<p>on my way</p>".into(),
            ..Default::default()
        };
        assert_eq!(preview(&group, &said), "Sam: on my way");
        assert_eq!(preview(&direct, &said), "on my way");
        let mine = LastMessage {
            mine: true,
            ..said.clone()
        };
        assert_eq!(preview(&direct, &mine), "You: on my way");
        let gone = LastMessage {
            deleted: true,
            ..said
        };
        assert_eq!(preview(&group, &gone), "Message deleted");
    }

    #[::core::prelude::v1::test]
    fn days_read_like_a_chat() {
        assert_eq!(day_label(Utc::now()), "Today");
        assert_eq!(
            day_label(Utc::now() - chrono::Duration::days(1)),
            "Yesterday"
        );
        assert_eq!(seen_line(true, None), "online");
        assert_eq!(seen_line(false, None), "offline");
    }
}
