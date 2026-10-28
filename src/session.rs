//! What every module needs to know about where it is: which profile (and its
//! client), who "me" is, and — for organisation-scoped modules — which
//! organisation, with its projects and members loaded once and shared.

use std::sync::Arc;

use gpui_kit::*;

use crate::api::ApiClient;
use crate::api::models::{Member, Organisation, Project};

/// One signed-in profile. Cheap to clone.
#[derive(Clone)]
pub struct Session {
    pub base_url: String,
    pub client: Arc<ApiClient>,
    /// The local user id `/api/me` returned, once it has.
    pub me: Option<String>,
}

impl Session {
    /// The web app's page for a path, for anything this app doesn't do.
    pub fn web_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

/// One organisation inside a session.
#[derive(Clone)]
pub struct OrgCtx {
    pub session: Session,
    pub org: Organisation,
    pub directory: Entity<Directory>,
}

impl OrgCtx {
    pub fn org_id(&self) -> &str {
        &self.org.id
    }

    pub fn client(&self) -> Arc<ApiClient> {
        self.session.client.clone()
    }

    pub fn me(&self) -> Option<&str> {
        self.session.me.as_deref()
    }
}

/// Projects and members, for the pickers on a task. Loaded on first use and
/// again on refresh; a failure leaves the last good list in place.
pub struct Directory {
    org_id: String,
    client: Arc<ApiClient>,
    pub projects: Vec<Project>,
    pub members: Vec<Member>,
    loaded: bool,
    _task: Option<Task<()>>,
}

impl Directory {
    pub fn new(org_id: &str, client: Arc<ApiClient>) -> Self {
        Self {
            org_id: org_id.to_owned(),
            client,
            projects: Vec::new(),
            members: Vec::new(),
            loaded: false,
            _task: None,
        }
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && self._task.is_none() {
            self.refresh(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let org_id = self.org_id.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let (projects, members) = cx
                .background_spawn(async move {
                    (client.projects(&org_id, false), client.members(&org_id))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(mut projects) = projects {
                    projects.retain(|p| !p.archived);
                    projects.sort_by_key(|p| p.name.to_lowercase());
                    this.projects = projects;
                }
                if let Ok(mut members) = members {
                    members.retain(|m| m.user_id.is_some() && m.status == "active");
                    members.sort_by_key(|m| m.name().to_lowercase());
                    this.members = members;
                }
                this.loaded = true;
                this._task = None;
                cx.notify();
            })
            .ok();
        }));
    }
}
