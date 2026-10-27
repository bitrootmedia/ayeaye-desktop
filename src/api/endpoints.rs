//! The endpoints behind tasks, the knowledge base, sparks, reminders, the
//! inbox, the timer, the planner and search. One method per call, a path and
//! a decode: filtering, sorting and access are all the server's job.

use serde_json::{Value, json};

use super::models::*;
use super::{ApiClient, ApiResult};

/// Which slice of the task list a view asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskQuery {
    pub project_id: Option<String>,
    /// Only tasks in no project.
    pub loose: bool,
    pub tag_id: Option<String>,
    pub owner_user_id: Option<String>,
    pub action_required_user_id: Option<String>,
    /// Triage: open tasks nobody is being asked to act on.
    pub action_required_unset: bool,
    pub include_closed: bool,
    pub status: Option<String>,
    pub priority: Option<String>,
    /// One of the server's sort keys; unknown ones are ignored there.
    pub sort: Option<String>,
    pub descending: bool,
    pub limit: usize,
}

fn org(org_id: &str) -> String {
    format!("/api/organisations/{org_id}")
}

/// Where a task's files live.
pub fn task_files_base(org_id: &str, task_id: &str) -> String {
    format!("{}/tasks/{task_id}", org(org_id))
}

/// Where one article revision's files live.
pub fn revision_files_base(org_id: &str, revision_id: &str) -> String {
    format!("{}/kb/revisions/{revision_id}", org(org_id))
}

/// `key=value&…`, percent-encoded, skipping unset values.
fn query(pairs: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", encode(v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

impl ApiClient {
    // ------------------------------------------------------------ structure

    pub fn projects(&self, org_id: &str) -> ApiResult<Vec<Project>> {
        self.get(&format!("{}/projects", org(org_id)))
    }

    pub fn members(&self, org_id: &str) -> ApiResult<Vec<Member>> {
        self.get(&format!("{}/members", org(org_id)))
    }

    // ---------------------------------------------------------------- tasks

    /// There is no default limit on this endpoint, so one is always sent.
    pub fn tasks(&self, org_id: &str, q: &TaskQuery) -> ApiResult<Vec<Task>> {
        self.tasks_counted(org_id, q).map(|(tasks, _)| tasks)
    }

    /// The tasks and how many match in all, which may be more than `limit`.
    pub fn tasks_counted(&self, org_id: &str, q: &TaskQuery) -> ApiResult<(Vec<Task>, usize)> {
        let flag = |b: bool| b.then(|| "true".to_owned());
        let qs = query(&[
            ("project_id", q.project_id.clone()),
            ("loose", flag(q.loose)),
            ("tag_id", q.tag_id.clone()),
            ("owner_user_id", q.owner_user_id.clone()),
            ("action_required_user_id", q.action_required_user_id.clone()),
            ("action_required_unset", flag(q.action_required_unset)),
            ("include_closed", flag(q.include_closed)),
            ("status", q.status.clone()),
            ("priority", q.priority.clone()),
            ("sort", q.sort.clone()),
            (
                "dir",
                q.sort
                    .as_ref()
                    .map(|_| if q.descending { "desc" } else { "asc" }.to_owned()),
            ),
            ("limit", Some(q.limit.max(1).to_string())),
        ]);
        let (tasks, total): (Vec<Task>, _) =
            self.get_counted(&format!("{}/tasks{qs}", org(org_id)))?;
        let total = total.unwrap_or(tasks.len());
        Ok((tasks, total))
    }

    pub fn tags(&self, org_id: &str) -> ApiResult<Vec<Tag>> {
        self.get(&format!("{}/tags", org(org_id)))
    }

    pub fn task(&self, org_id: &str, task_id: &str) -> ApiResult<Task> {
        self.get(&format!("{}/tasks/{task_id}", org(org_id)))
    }

    /// Descriptions go in as markdown — the door the server built for
    /// clients that don't emit its HTML allow-list.
    pub fn create_task(&self, org_id: &str, fields: Value) -> ApiResult<Task> {
        let mut fields = fields;
        if fields.get("description").is_some() {
            fields["description_format"] = "markdown".into();
        }
        self.post(&format!("{}/tasks", org(org_id)), &fields)
    }

    pub fn update_task(&self, org_id: &str, task_id: &str, fields: Value) -> ApiResult<Task> {
        let mut fields = fields;
        if fields.get("description").is_some() {
            fields["description_format"] = "markdown".into();
        }
        self.patch(&format!("{}/tasks/{task_id}", org(org_id)), &fields)
    }

    pub fn set_task_closed(&self, org_id: &str, task_id: &str, closed: bool) -> ApiResult<()> {
        self.post_empty(
            &format!("{}/tasks/{task_id}/closed", org(org_id)),
            &json!({ "closed": closed }),
        )
    }

    pub fn set_task_pinned(&self, org_id: &str, task_id: &str, pinned: bool) -> ApiResult<()> {
        let path = format!("{}/tasks/{task_id}/pin", org(org_id));
        if pinned {
            self.post_empty(&path, &json!({}))
        } else {
            self.delete(&path)
        }
    }

    pub fn checklists(&self, org_id: &str, task_id: &str) -> ApiResult<Vec<Checklist>> {
        self.get(&format!("{}/tasks/{task_id}/checklists", org(org_id)))
    }

    pub fn add_checklist(&self, org_id: &str, task_id: &str, title: &str) -> ApiResult<Checklist> {
        self.post(
            &format!("{}/tasks/{task_id}/checklists", org(org_id)),
            &json!({ "title": title }),
        )
    }

    pub fn add_checklist_item(
        &self,
        org_id: &str,
        task_id: &str,
        checklist_id: &str,
        text: &str,
    ) -> ApiResult<ChecklistItem> {
        self.post(
            &format!(
                "{}/tasks/{task_id}/checklists/{checklist_id}/items",
                org(org_id)
            ),
            &json!({ "text": text }),
        )
    }

    pub fn set_checklist_item_done(
        &self,
        org_id: &str,
        task_id: &str,
        checklist_id: &str,
        item_id: &str,
        done: bool,
    ) -> ApiResult<ChecklistItem> {
        self.patch(
            &format!(
                "{}/tasks/{task_id}/checklists/{checklist_id}/items/{item_id}",
                org(org_id)
            ),
            &json!({ "done": done }),
        )
    }

    pub fn delete_checklist_item(
        &self,
        org_id: &str,
        task_id: &str,
        checklist_id: &str,
        item_id: &str,
    ) -> ApiResult<()> {
        self.delete(&format!(
            "{}/tasks/{task_id}/checklists/{checklist_id}/items/{item_id}",
            org(org_id)
        ))
    }

    pub fn comments(&self, org_id: &str, task_id: &str) -> ApiResult<Thread> {
        self.get(&format!("{}/tasks/{task_id}/comments", org(org_id)))
    }

    pub fn post_comment(&self, org_id: &str, task_id: &str, body: &str) -> ApiResult<Message> {
        self.post(
            &format!("{}/tasks/{task_id}/comments", org(org_id)),
            &json!({ "body": body }),
        )
    }

    pub fn task_reminders(&self, org_id: &str, task_id: &str) -> ApiResult<Vec<Reminder>> {
        self.get(&format!("{}/tasks/{task_id}/reminders", org(org_id)))
    }

    pub fn add_task_reminder(
        &self,
        org_id: &str,
        task_id: &str,
        remind_on: &str,
        note: Option<&str>,
    ) -> ApiResult<Reminder> {
        self.post(
            &format!("{}/tasks/{task_id}/reminders", org(org_id)),
            &json!({ "remind_on": remind_on, "note": note }),
        )
    }

    // ---------------------------------------------------------------- time

    pub fn timer(&self) -> ApiResult<Timer> {
        self.get("/api/me/timer")
    }

    pub fn start_timer(&self, org_id: &str, task_id: &str) -> ApiResult<()> {
        self.post_empty(
            &format!("{}/tasks/{task_id}/time/start", org(org_id)),
            &json!({}),
        )
    }

    pub fn stop_timer(&self) -> ApiResult<()> {
        self.post_empty("/api/me/timer/stop", &json!({}))
    }

    /// "Still working on this?" — yes, or stop. The first answer anywhere wins.
    pub fn answer_check_in(&self, still_working: bool) -> ApiResult<Timer> {
        self.post(
            "/api/me/timer/check-in",
            &json!({ "still_working": still_working }),
        )
    }

    // -------------------------------------------------------------- planner

    pub fn planner(&self, org_id: &str) -> ApiResult<Planner> {
        self.get(&format!("{}/planner", org(org_id)))
    }

    pub fn place_in_planner(&self, org_id: &str, task_id: &str, bucket: &str) -> ApiResult<()> {
        self.put_empty(
            &format!("{}/planner/{task_id}", org(org_id)),
            &json!({ "bucket": bucket }),
        )
    }

    pub fn remove_from_planner(&self, org_id: &str, task_id: &str) -> ApiResult<()> {
        self.delete(&format!("{}/planner/{task_id}", org(org_id)))
    }

    // ------------------------------------------------------- knowledge base

    pub fn books(&self, org_id: &str) -> ApiResult<Vec<Book>> {
        self.get(&format!("{}/kb/books", org(org_id)))
    }

    pub fn create_book(&self, org_id: &str, name: &str) -> ApiResult<Book> {
        self.post(
            &format!("{}/kb/books", org(org_id)),
            &json!({ "name": name }),
        )
    }

    pub fn articles(&self, org_id: &str, book_id: &str) -> ApiResult<Vec<Article>> {
        self.get(&format!("{}/kb/books/{book_id}/articles", org(org_id)))
    }

    pub fn article(&self, org_id: &str, article_id: &str) -> ApiResult<Article> {
        self.get(&format!("{}/kb/articles/{article_id}", org(org_id)))
    }

    pub fn create_article(&self, org_id: &str, book_id: &str, title: &str) -> ApiResult<Article> {
        self.post(
            &format!("{}/kb/books/{book_id}/articles", org(org_id)),
            &json!({ "title": title }),
        )
    }

    pub fn delete_article(&self, org_id: &str, article_id: &str) -> ApiResult<()> {
        self.delete(&format!("{}/kb/articles/{article_id}", org(org_id)))
    }

    /// The article row carries no body; reading means its current revision.
    pub fn current_revision(&self, org_id: &str, article_id: &str) -> ApiResult<Option<Revision>> {
        let revisions: Vec<Revision> = self.get(&format!(
            "{}/kb/articles/{article_id}/revisions",
            org(org_id)
        ))?;
        let current = revisions.iter().find(|r| r.is_current).cloned();
        Ok(current.or_else(|| revisions.into_iter().next()))
    }

    /// The mutable revision an editor writes into: the server decides when a
    /// fresh one starts rather than reusing the last.
    pub fn start_edit_session(&self, org_id: &str, article_id: &str) -> ApiResult<Revision> {
        self.post(
            &format!("{}/kb/articles/{article_id}/edit-session", org(org_id)),
            &json!({}),
        )
    }

    pub fn save_revision(
        &self,
        org_id: &str,
        revision_id: &str,
        title: &str,
        markdown: &str,
    ) -> ApiResult<Revision> {
        self.patch(
            &format!("{}/kb/revisions/{revision_id}", org(org_id)),
            &json!({ "title": title, "body": markdown, "body_format": "markdown" }),
        )
    }

    pub fn set_article_private(
        &self,
        org_id: &str,
        article_id: &str,
        private: bool,
    ) -> ApiResult<Article> {
        self.patch(
            &format!("{}/kb/articles/{article_id}/private", org(org_id)),
            &json!({ "is_private": private }),
        )
    }

    // --------------------------------------------------------------- sparks

    pub fn sparks(&self) -> ApiResult<Vec<Spark>> {
        self.get("/api/sparks")
    }

    pub fn create_spark(&self, body: &str) -> ApiResult<Spark> {
        self.post("/api/sparks", &json!({ "body": body }))
    }

    pub fn update_spark(&self, id: &str, body: &str) -> ApiResult<Spark> {
        self.patch(&format!("/api/sparks/{id}"), &json!({ "body": body }))
    }

    pub fn delete_spark(&self, id: &str) -> ApiResult<()> {
        self.delete(&format!("/api/sparks/{id}"))
    }

    // ------------------------------------------------------------ reminders

    pub fn reminders(&self) -> ApiResult<Vec<Reminder>> {
        self.get("/api/reminders")
    }

    pub fn create_reminder(
        &self,
        org_id: &str,
        title: &str,
        remind_on: &str,
        note: Option<&str>,
    ) -> ApiResult<Reminder> {
        self.post(
            &format!("{}/reminders", org(org_id)),
            &json!({ "title": title, "remind_on": remind_on, "note": note }),
        )
    }

    pub fn update_reminder(&self, id: &str, fields: Value) -> ApiResult<Reminder> {
        self.patch(&format!("/api/reminders/{id}"), &fields)
    }

    pub fn delete_reminder(&self, id: &str) -> ApiResult<()> {
        self.delete(&format!("/api/reminders/{id}"))
    }

    // ---------------------------------------------------------------- inbox

    pub fn notifications(&self, limit: usize) -> ApiResult<Vec<Notification>> {
        self.get(&format!("/api/notifications?limit={limit}"))
    }

    pub fn unread_count(&self) -> ApiResult<i64> {
        let v: Value = self.get("/api/notifications/unread-count")?;
        Ok(v.get("unread").and_then(Value::as_i64).unwrap_or(0))
    }

    pub fn mark_read(&self, id: &str) -> ApiResult<()> {
        self.post_empty(&format!("/api/notifications/{id}/read"), &json!({}))
    }

    pub fn mark_all_read(&self) -> ApiResult<()> {
        self.post_empty("/api/notifications/read-all", &json!({}))
    }

    // --------------------------------------------------------------- search

    pub fn search(&self, org_id: &str, q: &str) -> ApiResult<SearchResults> {
        let qs = query(&[("q", Some(q.to_owned())), ("limit", Some("30".into()))]);
        self.get(&format!("{}/search{qs}", org(org_id)))
    }

    // --------------------------------------------------------------- files
    //
    // A task and an article revision expose the same `{base}/files` shape;
    // the base is the one thing that differs (`task_files_base`,
    // `revision_files_base`). Uploads are the web app's three steps: a
    // ticket, the bytes to storage, then confirm.

    pub fn files(&self, base: &str) -> ApiResult<Vec<FileItem>> {
        self.get(&format!("{base}/files"))
    }

    pub fn stage_file(
        &self,
        base: &str,
        filename: &str,
        content_type: &str,
    ) -> ApiResult<UploadTicket> {
        self.post(
            &format!("{base}/files"),
            &json!({ "filename": filename, "content_type": content_type }),
        )
    }

    /// Step 3, shared by every anchor: the server checks the real size here.
    pub fn confirm_attachment(&self, org_id: &str, attachment_id: &str) -> ApiResult<()> {
        self.post_empty(
            &format!("{}/attachments/{attachment_id}/confirm", org(org_id)),
            &json!({}),
        )
    }

    pub fn delete_file(&self, base: &str, file_id: &str) -> ApiResult<()> {
        self.delete(&format!("{base}/files/{file_id}"))
    }

    // ------------------------------------------------------ access tokens

    /// Mints a personal access token. The plaintext is in this answer and
    /// nowhere else, ever.
    pub fn create_token(&self, name: &str, scope: &str) -> ApiResult<TokenCreated> {
        self.post("/api/me/tokens", &json!({"name": name, "scope": scope}))
    }

    pub fn revoke_token(&self, token_id: &str) -> ApiResult<()> {
        self.delete(&format!("/api/me/tokens/{token_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn query_strings_encode_and_skip_unset() {
        assert_eq!(
            query(&[
                ("q", Some("a b&c".into())),
                ("x", None),
                ("limit", Some("5".into()))
            ]),
            "?q=a%20b%26c&limit=5"
        );
        assert_eq!(query(&[("x", None)]), "");
    }
}
