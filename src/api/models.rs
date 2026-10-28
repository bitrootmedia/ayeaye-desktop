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
    /// "Heads-down on the refit today": a line colleagues see beside the name.
    pub status_message: Option<String>,
    pub daily_summary_enabled: bool,
    /// In the person's own timezone.
    pub daily_summary_hour: i64,
    /// Only sent to organisation admins, so only offered to them.
    pub weekly_report_enabled: bool,
    /// `telegram`, `apps` or `off`.
    pub timer_check_ins: String,
    pub timer_check_ins_per_day: i64,
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

/// A project as the caller sees it: private to its owner until shared, so
/// `access` is this person's level (`read`, `write` or `owner`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub project_group_id: Option<String>,
    pub project_group_name: Option<String>,
    pub owner: Option<Person>,
    pub archived: bool,
    pub access: String,
    pub open_task_count: i64,
    /// Open and critical, urgent or high.
    pub important_task_count: i64,
}

impl Project {
    /// Name, description and group.
    pub fn can_edit(&self) -> bool {
        matches!(self.access.as_str(), "write" | "owner")
    }

    /// Archiving, sharing, handing over, deleting: the owner's alone.
    pub fn is_owner(&self) -> bool {
        self.access == "owner"
    }

    pub fn access_label(&self) -> &'static str {
        match self.access.as_str() {
            "owner" => "Owner",
            "write" => "Can edit",
            _ => "Can view",
        }
    }
}

/// A label for tidiness: filing a project in one grants nobody access.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub id: String,
    pub name: String,
    /// Its tasks leave the lists unless the tag is asked for by name.
    pub off_board: bool,
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
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    /// Only its owner can see it.
    pub is_hidden: bool,
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

// --------------------------------------------------------------- files

/// A file on a task (added directly or posted in a comment) or on one
/// article revision. `url` is presigned and minted per read: open it, never
/// keep it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct FileItem {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: Option<u64>,
    pub url: String,
    /// `None` until the worker has made one, or for anything not an image.
    pub thumbnail_url: Option<String>,
    /// Removed by removing its comment, not from the Files list.
    pub from_comment: bool,
    pub uploaded_by: Option<Person>,
    pub created_at: Option<DateTime<Utc>>,
}

impl FileItem {
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }
}

/// Step 1 of an upload: where to PUT the bytes, and the content type the
/// signature covers — sent back byte for byte, not our own guess.
#[derive(Clone, Debug, Deserialize)]
pub struct UploadTicket {
    pub attachment: UploadTicketAttachment,
    pub upload_url: String,
    pub content_type: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadTicketAttachment {
    pub id: String,
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
    Person { org_id: String, user_id: String },
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
        ["orgs", org, "people", user, ..] => Some(LinkTarget::Person {
            org_id: (*org).into(),
            user_id: (*user).into(),
        }),
        _ => None,
    }
}

// ----------------------------------------------------------------- people

/// One hour of one weekday somebody plans to be around: `weekday` is
/// 0 = Monday … 6 = Sunday, as the server's Python `date.weekday()` counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkingHourCell {
    pub weekday: u8,
    pub hour: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkingHours {
    /// IANA, or none for someone who has never opened an app.
    pub timezone: Option<String>,
    pub cells: Vec<WorkingHourCell>,
}

/// A stretch of out-of-office. Dates are `YYYY-MM-DD`, both included.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Absence {
    pub id: String,
    pub starts_on: String,
    pub ends_on: String,
    pub note: Option<String>,
    /// Who, on an organisation's list; absent on someone's own.
    pub person: Option<Person>,
    /// Away today, as opposed to some time in the next fortnight.
    pub away_now: bool,
}

/// A task on a colleague's profile. `is_owner` and `is_action_required` are
/// from *their* side, not the viewer's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub project_name: Option<String>,
    pub due_on: Option<String>,
    pub is_owner: bool,
    pub is_action_required: bool,
    pub is_open: bool,
    pub updated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileTimer {
    pub task: ProfileTask,
    pub started_at: Option<DateTime<Utc>>,
}

/// A colleague, for "is now a good time, and what are they on?". Their
/// tasks and timer reach only as far as the viewer's own task access.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemberProfile {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub role: String,
    pub joined_at: Option<DateTime<Utc>>,
    pub timezone: Option<String>,
    pub status_message: Option<String>,
    pub working_hours: Vec<WorkingHourCell>,
    pub away: Vec<Absence>,
    /// Only when the timer is on a task the viewer can see.
    pub working_on: Option<ProfileTimer>,
    pub open_tasks: Vec<ProfileTask>,
    /// How many open tasks of theirs the viewer can see in all.
    pub open_total: i64,
    pub recently_closed: Vec<ProfileTask>,
}

impl MemberProfile {
    pub fn name(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.email.clone())
    }
}

pub fn role_label(role: &str) -> &str {
    match role {
        "owner" => "Owner",
        "admin" => "Admin",
        "member" => "Member",
        "guest" => "Guest",
        other => other,
    }
}

// ---------------------------------------------------------------- account

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MfaStatus {
    pub enrolled: bool,
    pub codes_remaining: i64,
}

/// A fresh authenticator secret and its QR; nothing is kept until a code
/// from it is verified.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TotpDevice {
    pub secret: String,
    /// `data:image/png;base64,…`
    pub qr_data_uri: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupCodes {
    pub codes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessToken {
    pub id: String,
    pub name: String,
    /// `read` or `write`.
    pub scope: String,
    pub prefix: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

/// An app that connected itself over OAuth, rather than a pasted token.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuthGrant {
    pub id: String,
    pub client_name: String,
    pub scope: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationChannel {
    pub id: String,
    /// `email`, `telegram` or `webhook`.
    pub kind: String,
    pub label: String,
    pub enabled_kinds: Vec<String>,
    /// None for a Telegram link nobody has tapped `/start` on yet.
    pub verified_at: Option<DateTime<Utc>>,
    /// The webhook's own URL, never its secret.
    pub url: Option<String>,
    /// Where a linked Telegram chat's `/task` files tasks; set from Telegram.
    pub default_organisation_id: Option<String>,
}

/// A webhook, with the signing secret shown this once.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookCreated {
    pub id: String,
    pub secret: String,
}

/// The notification kinds a channel can carry, labelled as the web app
/// labels them in its routing table.
pub const NOTIFICATION_KINDS: &[(&str, &str)] = &[
    ("task_action_required", "Asked to act on a task"),
    ("task_action_required_cleared", "Handed back to you"),
    ("task_owner_changed", "Given ownership of a task"),
    ("task_closed", "A task you own was closed"),
    ("task_shared", "A task was shared with you"),
    ("project_shared", "A project was shared with you"),
    ("reminder_soon", "Reminder, the day before"),
    ("reminder_due", "Reminder, on the day"),
    ("task_deadline_tomorrow", "A task is due tomorrow"),
    ("daily_summary", "Daily summary"),
    ("export_ready", "A data export finished"),
    ("book_shared", "A book was shared with you"),
    ("comment_mention", "Mentioned in a comment"),
    ("timer_paused", "Your timer was paused while you were away"),
    (
        "weekly_report",
        "Weekly report (organisations you administer)",
    ),
];

/// Where one organisation's mail goes for this person. `pending` is asked
/// for and not yet confirmed — *not* in use.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrganisationEmail {
    pub organisation_id: String,
    pub organisation_name: String,
    pub email: Option<String>,
    pub pending: Option<String>,
    pub effective: Option<String>,
}

// --------------------------------------------------------------- dashboard

/// Something posted to everyone in the organisation. Admins post and take
/// them down; `sticky` ones stay at the top.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Announcement {
    pub id: String,
    pub body: String,
    pub sticky: bool,
    /// `YYYY-MM-DD`, the last day it shows.
    pub expires_on: Option<String>,
    pub author: Option<Person>,
    pub created_at: Option<DateTime<Utc>>,
}

/// A task on one of the dashboard's cards, from the viewer's side: their
/// action, or whoever they're waiting on. Overdue and due-today are worked
/// out by the server against the viewer's own timezone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DashboardTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub project_name: Option<String>,
    pub due_on: Option<String>,
    pub is_owner: bool,
    pub is_action_required: bool,
    pub waiting_on: Option<Person>,
    pub is_overdue: bool,
    pub is_due_today: bool,
}

/// One of the organisation's most recently touched tasks; a comment counts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub project_name: Option<String>,
    pub owner: Option<Person>,
    pub is_open: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

/// The organisation's front page, in one request. The task cards are the
/// viewer's own (owned or asked to act); `recent` is everybody's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dashboard {
    pub announcements: Vec<Announcement>,
    /// Away now or in the next fortnight.
    pub away: Vec<Absence>,
    pub can_announce: bool,
    pub critical: Vec<DashboardTask>,
    pub urgent: Vec<DashboardTask>,
    pub high: Vec<DashboardTask>,
    pub due_soon: Vec<DashboardTask>,
    /// Your own bookmarks; nobody else's dashboard shows them.
    pub pinned: Vec<DashboardTask>,
    pub recent: Vec<RecentTask>,
}

// ---------------------------------------------------------------- calendar

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarTask {
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD`.
    pub due_on: String,
    pub status: String,
    pub priority: String,
    pub project_name: Option<String>,
}

/// One of your own reminders; a standalone one has a `title` and no task.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarReminder {
    pub id: String,
    pub remind_on: String,
    pub note: Option<String>,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub title: Option<String>,
}

impl CalendarReminder {
    /// The note if there is one, else what it's about.
    pub fn label(&self) -> String {
        self.note
            .clone()
            .or_else(|| self.task_title.clone())
            .or_else(|| self.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Reminder".into())
    }
}

/// A window of the calendar: every visible task due in it and who's away
/// are the team's; the reminders are the viewer's alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarWindow {
    pub tasks: Vec<CalendarTask>,
    pub reminders: Vec<CalendarReminder>,
    pub away: Vec<Absence>,
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
        assert_eq!(
            parse_link("/orgs/o1/people/u1"),
            Some(LinkTarget::Person {
                org_id: "o1".into(),
                user_id: "u1".into()
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
