//! The window's content: sign-in, a session gate, or the workspace — and the
//! transitions between them.

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::{ActiveTheme as _, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;

use super::auth::{AuthEvent, Gate, GateEvent, GateView, SignInView};
use super::workspace::{Workspace, WorkspaceEvent};
use crate::api::ApiError;
use crate::app_state::AppState;

enum Screen {
    SignIn(Entity<SignInView>),
    Gate(Entity<GateView>),
    Workspace(Entity<Workspace>),
}

pub struct AppView {
    screen: Screen,
    /// The workspace a "Cancel" on the add-a-server screen goes back to.
    previous: Option<Entity<Workspace>>,
    _subscription: Option<Subscription>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            screen: Screen::SignIn(cx.new(|cx| SignInView::new(None, false, window, cx))),
            previous: None,
            _subscription: None,
        };
        view.open_active(window, cx);
        view
    }

    /// Opens whichever profile is active: straight into its workspace if it
    /// has a session (cached notes show at once; `/api/me` runs behind), or
    /// to sign-in if it doesn't.
    fn open_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = AppState::get(cx);
        let Some(profile) = state.active_profile() else {
            return self.show_sign_in(None, false, window, cx);
        };
        let has_session = AppState::update(cx, |state, _| state.client(&profile).has_session());
        if has_session {
            self.show_workspace(window, cx);
        } else {
            let can_cancel = AppState::get(cx).profiles.profiles.len() > 1;
            self.show_sign_in(Some(profile), can_cancel, window, cx);
        }
    }

    fn show_sign_in(
        &mut self,
        existing: Option<crate::store::profiles::Profile>,
        can_cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| SignInView::new(existing, can_cancel, window, cx));
        self._subscription = Some(cx.subscribe_in(&view, window, Self::on_auth_event));
        self.screen = Screen::SignIn(view);
        cx.notify();
    }

    fn show_gate(&mut self, gate: Gate, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = AppState::get(cx).active_profile() else {
            return;
        };
        let view = cx.new(|cx| GateView::new(profile, gate, window, cx));
        self._subscription = Some(cx.subscribe_in(&view, window, Self::on_gate_event));
        self.screen = Screen::Gate(view);
        cx.notify();
    }

    fn show_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = AppState::get(cx).active_profile() else {
            return;
        };
        let workspace = cx.new(|cx| Workspace::new(profile, window, cx));
        self._subscription = Some(cx.subscribe_in(&workspace, window, Self::on_workspace_event));
        self.previous = None;
        self.screen = Screen::Workspace(workspace);
        cx.notify();
    }

    fn leave_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Screen::Workspace(workspace) = &self.screen {
            workspace.update(cx, |ws, cx| ws.flush_all(window, cx));
        }
    }

    fn on_auth_event(
        &mut self,
        _: &Entity<SignInView>,
        event: &AuthEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AuthEvent::Ready => self.show_workspace(window, cx),
            AuthEvent::Gated(gate) => self.show_gate(*gate, window, cx),
            AuthEvent::Cancelled => match self.previous.take() {
                Some(workspace) => {
                    self._subscription =
                        Some(cx.subscribe_in(&workspace, window, Self::on_workspace_event));
                    self.screen = Screen::Workspace(workspace);
                    cx.notify();
                }
                None => {
                    // Cancelled re-signing into one profile: fall back to another
                    // that still has a session, if any.
                    let others: Vec<_> = AppState::get(cx).profiles.profiles.clone();
                    let current = AppState::get(cx).active_profile().map(|p| p.id);
                    if let Some(other) = others.iter().find(|p| Some(&p.id) != current.as_ref()) {
                        let id = other.id.clone();
                        AppState::update(cx, |state, _| state.set_active(&id));
                        self.open_active(window, cx);
                    }
                }
            },
        }
    }

    fn on_gate_event(
        &mut self,
        _: &Entity<GateView>,
        event: &GateEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            GateEvent::Unlocked => self.show_workspace(window, cx),
            GateEvent::SignOut => self.sign_out(window, cx),
        }
    }

    fn on_workspace_event(
        &mut self,
        _: &Entity<Workspace>,
        event: &WorkspaceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WorkspaceEvent::AuthRequired(ApiError::MfaRequired) => {
                self.show_gate(Gate::Mfa, window, cx)
            }
            WorkspaceEvent::AuthRequired(ApiError::EmailVerificationRequired) => {
                self.show_gate(Gate::EmailVerification, window, cx)
            }
            WorkspaceEvent::AuthRequired(_) => {
                // Only an expired session lands here; keep the profile and ask
                // for the password again.
                self.leave_workspace(window, cx);
                let profile = AppState::get(cx).active_profile();
                let can_cancel = AppState::get(cx).profiles.profiles.len() > 1;
                self.show_sign_in(profile, can_cancel, window, cx);
            }
            WorkspaceEvent::SwitchProfile(id) => {
                if AppState::get(cx)
                    .active_profile()
                    .is_some_and(|p| &p.id == id)
                {
                    return;
                }
                self.leave_workspace(window, cx);
                let id = id.clone();
                AppState::update(cx, |state, _| state.set_active(&id));
                self.open_active(window, cx);
            }
            WorkspaceEvent::AddProfile => {
                if let Screen::Workspace(workspace) = &self.screen {
                    self.previous = Some(workspace.clone());
                }
                let view = cx.new(|cx| SignInView::new(None, true, window, cx));
                self._subscription = Some(cx.subscribe_in(&view, window, Self::on_auth_event));
                self.screen = Screen::SignIn(view);
                cx.notify();
            }
            WorkspaceEvent::SignOut => self.sign_out(window, cx),
            WorkspaceEvent::RemoveProfile => self.confirm_remove(window, cx),
        }
    }

    fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_workspace(window, cx);
        let Some(profile) = AppState::get(cx).active_profile() else {
            return;
        };
        AppState::update(cx, |state, cx| state.sign_out(&profile.id, cx));
        let can_cancel = AppState::get(cx).profiles.profiles.len() > 1;
        self.show_sign_in(Some(profile), can_cancel, window, cx);
    }

    fn confirm_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = AppState::get(cx).active_profile() else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let id = profile.id.clone();
            alert
                .title(format!("Remove {} from the app?", profile.host()))
                .description(
                    "You'll be signed out and its cached notes cleared from this computer. Nothing on the server is deleted.",
                )
                .confirm()
                .ok_text("Remove")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let id = id.clone();
                    this.update(cx, |this, cx| {
                        this.leave_workspace(window, cx);
                        AppState::update(cx, |state, cx| state.remove_profile(&id, cx));
                        this.open_active(window, cx);
                    })
                    .ok();
                    true
                })
        });
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.screen {
            Screen::Workspace(workspace) => workspace.clone().into_any_element(),
            Screen::SignIn(view) => bare(view.clone().into_any_element(), cx),
            Screen::Gate(view) => bare(view.clone().into_any_element(), cx),
        }
    }
}

/// Sign-in screens still get a title bar, so the window can be dragged.
fn bare(content: AnyElement, cx: &mut Context<AppView>) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .child(TitleBar::new().child(h_flex().w_full()))
        .child(div().flex_1().min_h_0().child(content))
        .into_any_element()
}
