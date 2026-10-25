mod client;
mod endpoints;
pub mod models;
pub mod session;
pub mod socket;

pub use client::{ApiClient, ApiError, ApiResult, SignInOutcome, normalise_base_url};
pub use endpoints::TaskQuery;
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
        let signal = wait_for(&|s| matches!(s, Signal::Event(e) if e["type"] == "message"));
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
