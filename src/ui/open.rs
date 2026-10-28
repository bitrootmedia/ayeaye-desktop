//! "Open this" — the one event every panel and view emits, so the workspace
//! is the only place that knows how to turn an id into a tab.

use gpui_kit::SharedString;

use crate::tasks::table::TableFilters;

#[derive(Clone, Debug, PartialEq)]
pub enum Open {
    Note {
        id: String,
        preview: bool,
        focus: bool,
    },
    /// `org_id` set when the task may live in another organisation (an inbox
    /// link, a reminder); the workspace switches organisation first.
    Task {
        id: String,
        org_id: Option<String>,
        preview: bool,
    },
    Article {
        id: String,
        preview: bool,
    },
    Project {
        id: String,
        preview: bool,
    },
    /// A colleague's page. `org_id` as for a task.
    Person {
        id: String,
        org_id: Option<String>,
        preview: bool,
    },
    /// Your own account; one tab, whatever the organisation.
    Account,
    /// The organisation's front page.
    Home,
    /// The month grid.
    Calendar,
    Spark {
        id: String,
    },
    /// The task table, one per organisation. With filters, it shows those
    /// (the sidebar's "Open as table"); without, whatever it showed last.
    TaskTable {
        filters: Option<TableFilters>,
    },
    /// Something only the web app does.
    Web(String),
}

/// The tab key an `Open` lands on, for finding an already-open tab.
pub fn key_for(open: &Open) -> Option<SharedString> {
    Some(
        match open {
            Open::Note { id, .. } => format!("note:{id}"),
            Open::Task { id, .. } => format!("task:{id}"),
            Open::Article { id, .. } => format!("article:{id}"),
            Open::Project { id, .. } => format!("project:{id}"),
            Open::Person { id, .. } => format!("person:{id}"),
            Open::Account => "account:me".to_owned(),
            Open::Home => "home:dashboard".to_owned(),
            Open::Calendar => "calendar:month".to_owned(),
            Open::Spark { id } => format!("spark:{id}"),
            Open::TaskTable { .. } => "tasks:table".to_owned(),
            Open::Web(_) => return None,
        }
        .into(),
    )
}

/// The reverse, for reopening saved tabs.
pub fn open_for_key(key: &str) -> Option<Open> {
    let (kind, id) = key.split_once(':')?;
    let id = id.to_owned();
    Some(match kind {
        "note" => Open::Note {
            id,
            preview: false,
            focus: false,
        },
        "task" => Open::Task {
            id,
            org_id: None,
            preview: false,
        },
        "article" => Open::Article { id, preview: false },
        "project" => Open::Project { id, preview: false },
        "person" => Open::Person {
            id,
            org_id: None,
            preview: false,
        },
        "account" if id == "me" => Open::Account,
        "home" if id == "dashboard" => Open::Home,
        "calendar" if id == "month" => Open::Calendar,
        "spark" => Open::Spark { id },
        "tasks" if id == "table" => Open::TaskTable { filters: None },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn keys_round_trip() {
        for key in [
            "note:1",
            "task:2",
            "article:3",
            "project:5",
            "person:6",
            "account:me",
            "home:dashboard",
            "calendar:month",
            "spark:4",
            "tasks:table",
        ] {
            let open = open_for_key(key).unwrap();
            assert_eq!(key_for(&open).unwrap().as_ref(), key);
        }
        assert!(open_for_key("note-new:1").is_none());
    }
}
