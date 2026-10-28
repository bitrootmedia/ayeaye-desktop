//! The organisation's front page and its calendar, each a tab: what
//! everyone has been told, who's away, what needs you, and what's due when.

use crate::api::ApiError;

pub mod calendar;
pub mod dashboard;

/// What both tabs tell the workspace beyond `Open`.
pub enum HomeEvent {
    AuthFailed(ApiError),
}
