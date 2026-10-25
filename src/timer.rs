//! The running timer: one per person across the whole installation, so it
//! belongs to the session, not an organisation. Polled every minute (the
//! menu bar app's cadence is five; a desktop window is looked at more), and
//! refetched right after anything here starts or stops it.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gpui_kit::*;

use crate::api::models::Timer;
use crate::api::{ApiClient, ApiError};

const POLL: Duration = Duration::from_secs(60);

pub struct TimerState {
    client: Arc<ApiClient>,
    pub timer: Timer,
    pub error: Option<String>,
    _poll: Task<()>,
    _tick: Option<Task<()>>,
    _action: Option<Task<()>>,
}

impl TimerState {
    pub fn new(client: Arc<ApiClient>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        Self {
            client,
            timer: Timer::default(),
            error: None,
            _poll: poll,
            _tick: None,
            _action: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.timer
            .entry
            .as_ref()
            .is_some_and(|e| e.ended_at.is_none())
    }

    pub fn running_task(&self) -> Option<&str> {
        self.timer
            .entry
            .as_ref()
            .filter(|e| e.ended_at.is_none())
            .map(|e| e.task_id.as_str())
    }

    pub fn elapsed(&self) -> i64 {
        match &self.timer.entry {
            Some(e) => match e.started_at {
                Some(start) if e.ended_at.is_none() => (Utc::now() - start).num_seconds(),
                _ => e.seconds,
            },
            None => 0,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.run(cx, move || client.timer());
    }

    pub fn start(&mut self, org_id: String, task_id: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.run(cx, move || {
            client.start_timer(&org_id, &task_id)?;
            client.timer()
        });
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.run(cx, move || {
            client.stop_timer()?;
            client.timer()
        });
    }

    /// "Still working on this?" — yes keeps it going, no stops it.
    pub fn answer_check_in(&mut self, still_working: bool, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.run(cx, move || client.answer_check_in(still_working));
    }

    fn run(
        &mut self,
        cx: &mut Context<Self>,
        call: impl FnOnce() -> Result<Timer, ApiError> + Send + 'static,
    ) {
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { call() }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(timer) => {
                        this.timer = timer;
                        this.error = None;
                    }
                    // A failed poll leaves the last known state: "we couldn't
                    // check" is not "nothing is running".
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.update_tick(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Redraw every second only while something is running.
    fn update_tick(&mut self, cx: &mut Context<Self>) {
        if !self.is_running() {
            self._tick = None;
            return;
        }
        if self._tick.is_some() {
            return;
        }
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }
}
