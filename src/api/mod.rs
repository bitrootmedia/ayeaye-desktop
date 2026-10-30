mod client;
mod endpoints;
pub mod models;
pub mod session;
pub mod socket;

pub use client::{ApiClient, ApiError, ApiResult, SignInOutcome, normalise_base_url};
pub use endpoints::{TaskQuery, revision_files_base, task_files_base};
use serde_json::json;

use models::{MAX_TITLE_LENGTH, Me, Note, Organisation};

/// Every endpoint the desktop app calls, in one place.
impl ApiClient {
    /// Gates the whole UI: it creates the local user row on first sight and
    /// binds pending invitations, so nothing else is fetched before it answers.
    pub fn me(&self) -> ApiResult<Me> {
        self.get("/api/me")
    }

    /// Reminders and digests compute "today" per user zone; a setting nobody
    /// finds stays wrong, so the app sends the machine's zone itself.
    pub fn sync_timezone(&self, me: &Me) {
        let Ok(zone) = iana_time_zone::get_timezone() else {
            return;
        };
        if me.timezone.as_deref() != Some(zone.as_str()) {
            let _ = self.patch::<serde_json::Value>("/api/me", &json!({"timezone": zone}));
        }
    }

    pub fn organisations(&self) -> ApiResult<Vec<Organisation>> {
        self.get("/api/organisations")
    }

    pub fn notes(&self, org_id: &str) -> ApiResult<Vec<Note>> {
        self.get(&format!("/api/organisations/{org_id}/notes"))
    }

    pub fn create_note(&self, org_id: &str, title: &str, body: &str) -> ApiResult<Note> {
        self.post(
            &format!("/api/organisations/{org_id}/notes"),
            &json!({"title": clamp_title(title), "body": body}),
        )
    }

    /// Only the fields that changed. The server refuses an empty title, so a
    /// blank one is simply not sent and the old title stands.
    pub fn update_note(
        &self,
        org_id: &str,
        note_id: &str,
        title: Option<&str>,
        body: Option<&str>,
    ) -> ApiResult<Note> {
        let mut fields = serde_json::Map::new();
        if let Some(title) = title.map(clamp_title).filter(|t| !t.is_empty()) {
            fields.insert("title".into(), title.into());
        }
        if let Some(body) = body {
            fields.insert("body".into(), body.into());
        }
        self.patch(
            &format!("/api/organisations/{org_id}/notes/{note_id}"),
            &serde_json::Value::Object(fields),
        )
    }

    pub fn delete_note(&self, org_id: &str, note_id: &str) -> ApiResult<()> {
        self.delete(&format!("/api/organisations/{org_id}/notes/{note_id}"))
    }
}

pub fn clamp_title(title: &str) -> String {
    title.trim().chars().take(MAX_TITLE_LENGTH).collect()
}

/// Against a real instance, skipped unless pointed at one:
///
/// ```sh
/// AYEAYE_TEST_SERVER=http://localhost AYEAYE_TEST_EMAIL=… AYEAYE_TEST_PASSWORD=… \
///   cargo test live_ -- --ignored --nocapture
/// ```
///
/// Creates a note, a task, a book and article, a spark and reminders in the
/// account's first organisation; most are cleaned up, the task is closed.
#[cfg(test)]
mod live {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn env(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
    }

    #[test]
    #[ignore]
    fn live_sign_in_and_note_round_trip() {
        let rotations = Arc::new(Mutex::new(0));
        let counter = rotations.clone();
        let client = ApiClient::new(env("AYEAYE_TEST_SERVER"), Default::default(), move |_| {
            *counter.lock().unwrap() += 1
        });

        let outcome = client
            .sign_in(&env("AYEAYE_TEST_EMAIL"), &env("AYEAYE_TEST_PASSWORD"))
            .unwrap();
        assert_eq!(outcome, SignInOutcome::SignedIn);
        assert!(client.tokens().has_session());
        assert!(
            client.tokens().access_expiry_ms.is_some(),
            "Front-Token parsed"
        );
        assert!(*rotations.lock().unwrap() > 0, "tokens handed to the store");

        let wrong = client
            .sign_in(&env("AYEAYE_TEST_EMAIL"), "definitely-not-it")
            .unwrap();
        assert_eq!(wrong, SignInOutcome::WrongCredentials);

        let me = client.me().unwrap();
        assert!(!me.id.is_empty());
        let orgs = client.organisations().unwrap();
        let org = orgs
            .first()
            .expect("the test account is in an organisation");

        let note = client
            .create_note(&org.id, "  desktop live test  ", "first")
            .unwrap();
        assert_eq!(note.title, "desktop live test");
        let updated = client
            .update_note(&org.id, &note.id, None, Some("second"))
            .unwrap();
        assert_eq!(updated.body, "second");
        assert_eq!(updated.title, "desktop live test");
        // A blank title is not sent, so the old one stands.
        let kept = client
            .update_note(&org.id, &note.id, Some("   "), None)
            .unwrap();
        assert_eq!(kept.title, "desktop live test");

        let listed = client.notes(&org.id).unwrap();
        assert_eq!(
            listed.first().map(|n| n.id.as_str()),
            Some(note.id.as_str()),
            "newest first"
        );

        // A forced refresh rotates both tokens and keeps the session working.
        let before = client.tokens();
        client.refresh_session().unwrap();
        assert_ne!(client.tokens().access_token, before.access_token);
        client.me().unwrap();

        client.delete_note(&org.id, &note.id).unwrap();
        assert!(matches!(
            client.update_note(&org.id, &note.id, None, Some("x")),
            Err(ApiError::NotFound(_))
        ));

        // Tasks, the planner, the timer, search.
        let task = client
            .create_task(
                &org.id,
                serde_json::json!({"title": "desktop live task", "description": "**bold**"}),
            )
            .unwrap();
        assert!(
            task.description
                .as_deref()
                .unwrap_or("")
                .contains("<strong>bold</strong>")
        );
        let task = client
            .update_task(
                &org.id,
                &task.id,
                serde_json::json!({"priority": "high", "status": "review"}),
            )
            .unwrap();
        assert_eq!(
            (task.priority.as_str(), task.status.as_str()),
            ("high", "review")
        );
        let list = client.add_checklist(&org.id, &task.id, "Steps").unwrap();
        let item = client
            .add_checklist_item(&org.id, &task.id, &list.id, "one")
            .unwrap();
        assert!(
            client
                .set_checklist_item_done(&org.id, &task.id, &list.id, &item.id, true)
                .unwrap()
                .done
        );
        client.post_comment(&org.id, &task.id, "hello").unwrap();
        assert_eq!(
            client.comments(&org.id, &task.id).unwrap().messages.len(),
            1
        );
        client
            .add_task_reminder(&org.id, &task.id, "2030-01-01", None)
            .unwrap();
        assert_eq!(client.task_reminders(&org.id, &task.id).unwrap().len(), 1);
        client
            .place_in_planner(&org.id, &task.id, "someday")
            .unwrap();
        let planner = client.planner(&org.id).unwrap();
        assert!(
            planner
                .buckets
                .get("someday")
                .is_some_and(|b| b.iter().any(|e| e.task.id == task.id))
        );
        client.start_timer(&org.id, &task.id).unwrap();
        assert_eq!(
            client.timer().unwrap().entry.map(|e| e.task_id),
            Some(task.id.clone())
        );
        client.stop_timer().unwrap();
        assert!(
            client
                .timer()
                .unwrap()
                .entry
                .is_none_or(|e| e.ended_at.is_some())
        );
        let mine = client
            .tasks(
                &org.id,
                &TaskQuery {
                    owner_user_id: Some(me.id.clone()),
                    limit: 500,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(mine.iter().any(|t| t.id == task.id));
        client.set_task_closed(&org.id, &task.id, true).unwrap();
        assert!(!client.task(&org.id, &task.id).unwrap().is_open);
        let _ = client.search(&org.id, "desktop live").unwrap();

        // Knowledge base: create, edit as markdown, publish, delete.
        let book = client.create_book(&org.id, "desktop live book").unwrap();
        let article = client
            .create_article(&org.id, &book.id, "desktop live article")
            .unwrap();
        assert!(article.is_private, "articles are born private drafts");
        let rev = client.start_edit_session(&org.id, &article.id).unwrap();
        let saved = client
            .save_revision(
                &org.id,
                &rev.id,
                "desktop live article",
                "## Heading\n\ntext",
            )
            .unwrap();
        assert!(saved.body.contains("<h2>"));
        assert!(
            !client
                .set_article_private(&org.id, &article.id, false)
                .unwrap()
                .is_private
        );
        client.delete_article(&org.id, &article.id).unwrap();

        // Sparks and reminders.
        let spark = client.create_spark("desktop live spark").unwrap();
        assert_eq!(
            client.update_spark(&spark.id, "edited").unwrap().body,
            "edited"
        );
        client.delete_spark(&spark.id).unwrap();
        let reminder = client
            .create_reminder(&org.id, "desktop live reminder", "2030-01-02", None)
            .unwrap();
        client
            .update_reminder(&reminder.id, serde_json::json!({"done": true}))
            .unwrap();
        let _ = client.notifications(10).unwrap();
        let _ = client.unread_count().unwrap();

        // Someone else's organisation reads as 404, never 403.
        assert!(matches!(
            client.notes("00000000-0000-0000-0000-000000000000"),
            Err(ApiError::NotFound(_))
        ));
    }

    /// Files, the web app's three steps: on a task and on an article
    /// revision — ticket, bytes to storage, confirm — then listed with a
    /// thumbnail that fetches, and removed.
    #[::core::prelude::v1::test]
    #[ignore]
    fn live_files_upload_list_and_remove() {
        // A 1×1 PNG.
        const DOT: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xDA, 0x63, 0xFC, 0xCF, 0xC0, 0x50, 0x0F, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xA9,
            0x8C, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let client = ApiClient::new(env("AYEAYE_TEST_SERVER"), Default::default(), |_| {});
        client
            .sign_in(&env("AYEAYE_TEST_EMAIL"), &env("AYEAYE_TEST_PASSWORD"))
            .unwrap();
        let org = client.organisations().unwrap().remove(0);
        let upload = |base: &str, name: &str, content_type: &str, bytes: &[u8]| {
            let ticket = client.stage_file(base, name, content_type).unwrap();
            client
                .put_to_storage(&ticket.upload_url, &ticket.content_type, bytes)
                .unwrap();
            client
                .confirm_attachment(&org.id, &ticket.attachment.id)
                .unwrap();
            ticket.attachment.id
        };

        let task = client
            .create_task(&org.id, serde_json::json!({"title": "desktop files test"}))
            .unwrap();
        let base = task_files_base(&org.id, &task.id);
        let png = upload(&base, "dot.png", "image/png", DOT);
        upload(
            &base,
            "notes.txt",
            "text/plain",
            b"hello from the desktop\n",
        );
        let files = client.files(&base).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();
        assert!(
            names.contains(&"dot.png") && names.contains(&"notes.txt"),
            "{names:?}"
        );
        let dot = files.iter().find(|f| f.id == png).unwrap();
        assert_eq!(dot.size_bytes, Some(DOT.len() as u64));
        assert!(dot.is_image() && !dot.from_comment);
        let fetched = client
            .get_from_storage(dot.thumbnail_url.as_deref().unwrap_or(&dot.url))
            .unwrap();
        assert!(fetched.starts_with(b"\x89PNG") || fetched.starts_with(&[0xFF, 0xD8]));
        assert!(matches!(
            client.stage_file(&base, "x.zip", "application/zip"),
            Err(ApiError::Server { .. })
        ));
        client.delete_file(&base, &png).unwrap();
        assert_eq!(client.files(&base).unwrap().len(), 1);
        client.set_task_closed(&org.id, &task.id, true).unwrap();

        let book = client.create_book(&org.id, "desktop files book").unwrap();
        let article = client
            .create_article(&org.id, &book.id, "desktop files article")
            .unwrap();
        let rev = client.start_edit_session(&org.id, &article.id).unwrap();
        let base = revision_files_base(&org.id, &rev.id);
        upload(&base, "dot.png", "image/png", DOT);
        assert_eq!(client.files(&base).unwrap().len(), 1);
        client.delete_article(&org.id, &article.id).unwrap();
    }

    /// Chats with one account: saved messages, a group, a task's thread
    /// seen as a chat — sending, quoting, editing, removing, reading,
    /// pinning, and a file posted into one.
    #[::core::prelude::v1::test]
    #[ignore]
    fn live_chats() {
        let client = ApiClient::new(env("AYEAYE_TEST_SERVER"), Default::default(), |_| {});
        client
            .sign_in(&env("AYEAYE_TEST_EMAIL"), &env("AYEAYE_TEST_PASSWORD"))
            .unwrap();
        let me = client.me().unwrap();
        let org = client.organisations().unwrap().remove(0);

        let saved = client.open_direct(&org.id, &me.id).unwrap();
        assert_eq!(
            (saved.kind.as_str(), saved.title.as_str()),
            ("direct", "Saved messages")
        );
        assert_eq!(client.open_direct(&org.id, &me.id).unwrap().id, saved.id);
        let first = client
            .send_chat_message(&org.id, &saved.id, "desktop chat one", &[], None)
            .unwrap();
        let reply = client
            .send_chat_message(&org.id, &saved.id, "desktop chat two", &[], Some(&first.id))
            .unwrap();
        assert_eq!(
            reply.reply_to.as_ref().map(|r| r.body.as_str()),
            Some("desktop chat one")
        );
        let page = client.chat_messages(&org.id, &saved.id, None, 1).unwrap();
        assert!(page.has_more && page.messages[0].id == reply.id);
        let older = client
            .chat_messages(&org.id, &saved.id, Some(&reply.id), 1)
            .unwrap();
        assert_eq!(older.messages[0].id, first.id);
        assert_eq!(
            client
                .edit_message(&org.id, &first.id, "desktop chat one, edited")
                .unwrap()
                .body,
            "desktop chat one, edited"
        );
        client.delete_message(&org.id, &first.id).unwrap();
        client.mark_chat_read(&org.id, &saved.id, None).unwrap();

        let ticket = client
            .stage_chat_attachment(&org.id, &saved.id, "note.txt", "text/plain")
            .unwrap();
        client
            .put_to_storage(&ticket.upload_url, &ticket.content_type, b"hello")
            .unwrap();
        client
            .confirm_attachment(&org.id, &ticket.attachment.id)
            .unwrap();
        let with_file = client
            .send_chat_message(
                &org.id,
                &saved.id,
                "",
                &[ticket.attachment.id.clone()],
                None,
            )
            .unwrap();
        // Only a file, no words: allowed exactly because something's attached.
        assert_eq!(with_file.attachments.len(), 1);
        assert!(with_file.body.is_empty());

        client
            .set_chat_state(&org.id, &saved.id, serde_json::json!({"pinned": true}))
            .unwrap();
        let (list, total) = client.chats(&org.id, "all", "", 50, 0).unwrap();
        assert!(total >= list.len() && list[0].pinned);
        let found = list.iter().find(|c| c.id == saved.id).unwrap();
        assert_eq!(found.last_message.as_ref().map(|m| m.attachments), Some(1));
        client
            .set_chat_state(&org.id, &saved.id, serde_json::json!({"pinned": false}))
            .unwrap();
        let (hits, _) = client
            .chats(&org.id, "all", "desktop chat two", 50, 0)
            .unwrap();
        assert!(hits.iter().any(|c| c.id == saved.id));
        let _ = client.chats_unread(&org.id).unwrap();

        let group = client
            .create_group(&org.id, "  desktop live group  ", &[])
            .unwrap();
        assert_eq!(group.title, "desktop live group");
        assert_eq!(
            client
                .rename_chat(&org.id, &group.id, "desktop group, renamed")
                .unwrap()
                .title,
            "desktop group, renamed"
        );
        client
            .send_chat_message(&org.id, &group.id, "hello group", &[], None)
            .unwrap();
        client
            .remove_chat_member(&org.id, &group.id, &me.id)
            .unwrap();
        assert!(matches!(
            client.chat(&org.id, &group.id),
            Err(ApiError::NotFound(_))
        ));

        let task = client
            .create_task(&org.id, serde_json::json!({"title": "desktop chat task"}))
            .unwrap();
        let thread = client.task_chat(&org.id, &task.id).unwrap();
        assert_eq!(thread.kind, "task");
        assert_eq!(
            thread.task.as_ref().map(|t| t.id.as_str()),
            Some(task.id.as_str())
        );
        client
            .send_chat_message(&org.id, &thread.id, "from chats", &[], None)
            .unwrap();
        assert_eq!(
            client.comments(&org.id, &task.id).unwrap().messages[0].body,
            "from chats"
        );
        client.set_task_closed(&org.id, &task.id, true).unwrap();
    }

    /// The socket: a minted read token gets in, a task change and a comment
    /// arrive as content-free nudges, and a revoked token is refused.
    #[::core::prelude::v1::test]
    #[ignore]
    fn live_socket_nudges() {
        use std::sync::mpsc;
        use std::time::Duration;

        use socket::{Signal, Socket, socket_url};

        let client = ApiClient::new(env("AYEAYE_TEST_SERVER"), Default::default(), |_| {});
        client
            .sign_in(&env("AYEAYE_TEST_EMAIL"), &env("AYEAYE_TEST_PASSWORD"))
            .unwrap();
        let org = client.organisations().unwrap().remove(0);
        let minted = client
            .create_token("Desktop app (live test)", "read")
            .unwrap();
        let url = socket_url(&env("AYEAYE_TEST_SERVER"));

        let (tx, rx) = mpsc::channel();
        let socket = Socket::spawn(url.clone(), minted.token.clone(), move |s| {
            tx.send(s).is_ok()
        });
        let wait_for = |want: &dyn Fn(&Signal) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if let Ok(signal) = rx.recv_timeout(Duration::from_millis(200))
                    && want(&signal)
                {
                    return signal;
                }
            }
            panic!("no matching signal within 10s");
        };
        wait_for(&|s| matches!(s, Signal::Connected));
        socket.watch(Some(format!("org:{}", org.id)));
        std::thread::sleep(Duration::from_millis(500));

        let task = client
            .create_task(&org.id, serde_json::json!({"title": "desktop socket test"}))
            .unwrap();
        let id = task.id.clone();
        wait_for(
            &|s| matches!(s, Signal::Event(e) if e["type"] == "task" && e["task_id"] == id.as_str()),
        );
        client
            .update_task(&org.id, &task.id, serde_json::json!({"status": "review"}))
            .unwrap();
        wait_for(
            &|s| matches!(s, Signal::Event(e) if e["type"] == "task" && e["task_id"] == id.as_str()),
        );

        socket.watch(Some(format!("task:{}", task.id)));
        std::thread::sleep(Duration::from_millis(500));
        client
            .post_comment(&org.id, &task.id, "socket hello")
            .unwrap();
        // Another live test may be talking in the same organisation: wait
        // for this task's message, not just any.
        let signal = wait_for(
            &|s| matches!(s, Signal::Event(e) if e["type"] == "message" && e["anchor"]["id"] == id.as_str()),
        );
        let Signal::Event(event) = signal else {
            unreachable!()
        };
        assert_eq!(event["anchor"]["id"], task.id.as_str());
        assert!(event.get("body").is_none(), "events carry no content");

        client.set_task_closed(&org.id, &task.id, true).unwrap();
        drop(socket);
        client.revoke_token(&minted.id).unwrap();
        assert!(matches!(
            socket::connect(&url, &minted.token),
            Err(socket::ConnectError::Unauthorized)
        ));
    }
}
