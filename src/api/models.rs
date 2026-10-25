//! The handful of DTOs the desktop app reads. Tolerant on purpose, like the
//! mobile app's generated models: a field the server stops sending becomes a
//! default instead of a failed screen.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Me {
    pub id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Organisation {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
}

/// A notepad entry: personal, scoped to one organisation, visible to its
/// author alone (`services/personal_notes.py` has no admin branch).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The server's own limit (`MAX_TITLE_LENGTH`); it truncates rather than
/// refusing, so we trim to match and never show a title it won't keep.
pub const MAX_TITLE_LENGTH: usize = 200;

// ------------------------------------------------------------------ people

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Person {
    pub id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

impl Person {
    pub fn name(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| "Someone".into())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Member {
    pub id: String,
    pub role: String,
    pub status: String,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

impl Member {
    pub fn name(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| self.email.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub project_group_name: Option<String>,
    pub archived: bool,
    pub open_task_count: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub id: String,
    pub name: String,
}

// ------------------------------------------------------------------- tasks

/// Workflow position. Open/closed is a separate flag: there is no "done".
pub const STATUSES: &[(&str, &str)] = &[
    ("todo", "To do"),
    ("in_progress", "In progress"),
    ("review", "Review"),
    ("on_hold", "On hold"),
    ("blocker", "Blocker"),
];

pub const PRIORITIES: &[(&str, &str)] = &[
    ("critical", "Critical"),
    ("urgent", "Urgent"),
    ("high", "High"),
    ("normal", "Normal"),
    ("low", "Low"),
    ("very_low", "Very low"),
];

pub fn status_label(key: &str) -> &str {
    STATUSES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| *l)
        .unwrap_or(key)
}

pub fn priority_label(key: &str) -> &str {
    PRIORITIES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| *l)
        .unwrap_or(key)
}

/// Rank for sorting: statuses and priorities sort by meaning, never A–Z.
pub fn priority_rank(key: &str) -> usize {
    PRIORITIES.iter().position(|(k, _)| *k == key).unwrap_or(3)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// Sanitised HTML, or plain text for old tasks (no `<` in it).
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub is_open: bool,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub owner: Option<Person>,
    pub action_required: Option<Person>,
    pub due_on: Option<String>,
    pub estimated_hours: Option<f64>,
    pub updated_at: Option<DateTime<Utc>>,
    pub is_pinned: bool,
    pub planner_bucket: Option<String>,
    pub tags: Vec<Tag>,
    /// `owner`, `write` or `read` — resolved server-side.
    pub access: String,
    pub can_close: bool,
}

impl Task {
    pub fn can_edit(&self) -> bool {
        matches!(self.access.as_str(), "owner" | "write")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChecklistItem {
    pub id: String,
    pub text: String,
    pub done: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checklist {
    pub id: String,
    pub title: String,
    pub items: Vec<ChecklistItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: String,
    pub author: Option<Person>,
    pub body: String,
    pub created_at: Option<DateTime<Utc>>,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted: bool,
    pub mine: bool,
    pub via: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thread {
    pub messages: Vec<Message>,
    pub can_post: bool,
}

// ------------------------------------------------------------------- time

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeEntry {
    pub id: String,
    pub task_id: String,
    pub task_title: Option<String>,
    pub project_name: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub seconds: i64,
}

/// The one running timer a person has, across every organisation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timer {
    pub entry: Option<TimeEntry>,
    pub organisation_id: Option<String>,
    /// Set while the server is asking "still working on this?".
    pub check_in_deadline: Option<DateTime<Utc>>,
}

// ----------------------------------------------------------------- planner

pub const BUCKETS: &[(&str, &str)] = &[
    ("today", "Today"),
    ("tomorrow", "Tomorrow"),
    ("this_week", "This week"),
    ("next_week", "Next week"),
    ("someday", "Someday"),
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlannerTask {
    pub id: String,
    pub title: String,
    pub priority: String,
    pub status: String,
    pub is_open: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlannerEntry {
    pub task: PlannerTask,
    pub bucket: String,
    pub position: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Planner {
    pub pool: Vec<PlannerTask>,
    pub buckets: std::collections::HashMap<String, Vec<PlannerEntry>>,
}

// ---------------------------------------------------------- knowledge base

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Book {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub archived: bool,
    pub access: String,
    pub article_count: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Article {
    pub id: String,
    pub book_id: String,
    pub title: String,
    pub owner: Option<Person>,
    /// Articles are born private drafts; publishing is what shares them.
    pub is_private: bool,
    pub can_make_private: bool,
    pub updated_at: Option<DateTime<Utc>>,
    pub access: String,
}

impl Article {
    pub fn can_edit(&self) -> bool {
        matches!(self.access.as_str(), "owner" | "write")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Revision {
    pub id: String,
    pub article_id: String,
    pub title: String,
    /// Sanitised HTML.
    pub body: String,
    pub edited_by: Option<Person>,
    pub updated_at: Option<DateTime<Utc>>,
    pub is_current: bool,
}

// -------------------------------------------------------------- tokens

#[derive(Clone, Debug, Deserialize)]
pub struct TokenCreated {
    pub id: String,
    pub token: String,
}

// -------------------------------------------------- sparks, reminders, inbox

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spark {
    pub id: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reminder {
    pub id: String,
    /// `YYYY-MM-DD`.
    pub remind_on: String,
    pub note: Option<String>,
    pub overdue: bool,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub title: Option<String>,
    pub organisation_id: Option<String>,
    pub organisation_name: Option<String>,
}

impl Reminder {
    pub fn label(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.task_title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Reminder".into())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notification {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    /// e.g. `/orgs/<org>/tasks/<task>` — the web app's route.
    pub link_path: Option<String>,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

/// Where a notification's link points, when it's somewhere this app can open.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Task { org_id: String, task_id: String },
    Book { org_id: String, book_id: String },
}

pub fn parse_link(path: &str) -> Option<LinkTarget> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["orgs", org, "tasks", task, ..] => Some(LinkTarget::Task {
            org_id: (*org).into(),
            task_id: (*task).into(),
        }),
        ["orgs", org, "kb", "books", book, ..] => Some(LinkTarget::Book {
            org_id: (*org).into(),
            book_id: (*book).into(),
        }),
        _ => None,
    }
}

// ------------------------------------------------------------------ search

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchHit {
    /// `task`, `project`, `article`, `book`, `comment`…
    pub kind: String,
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub context: Option<String>,
    pub inactive: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn links_to_tasks_and_books_parse() {
        assert_eq!(
            parse_link("/orgs/o1/tasks/t1"),
            Some(LinkTarget::Task {
                org_id: "o1".into(),
                task_id: "t1".into()
            })
        );
        assert_eq!(
            parse_link("/orgs/o1/kb/books/b1"),
            Some(LinkTarget::Book {
                org_id: "o1".into(),
                book_id: "b1".into()
            })
        );
        assert_eq!(parse_link("/settings"), None);
    }

    #[::core::prelude::v1::test]
    fn tolerant_task_decoding() {
        let task: Task = serde_json::from_str(r#"{"id":"t","title":"x","status":"todo","priority":"high","is_open":true,"unknown":1}"#).unwrap();
        assert_eq!(task.priority, "high");
        assert!(task.owner.is_none());
        assert_eq!(priority_rank("critical"), 0);
        assert_eq!(status_label("in_progress"), "In progress");
    }
}
