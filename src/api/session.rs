//! One profile's SuperTokens session, in header mode.
//!
//! The same lifecycle the mobile app hand-rolls (see its `NOTES-API.md` §2):
//! `st-auth-mode: header` gets tokens instead of cookies, both tokens rotate on
//! every refresh, and the `Front-Token` header says when the access token dies
//! without us decoding the JWT.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionTokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// `ate` from the Front-Token, in epoch milliseconds.
    pub access_expiry_ms: Option<i64>,
    pub user_id: Option<String>,
}

impl SessionTokens {
    pub fn has_session(&self) -> bool {
        self.access_token.as_deref().is_some_and(|t| !t.is_empty())
    }

    pub fn has_refresh(&self) -> bool {
        self.refresh_token.as_deref().is_some_and(|t| !t.is_empty())
    }

    /// Refresh a minute early — a token that expires mid-flight costs a retry.
    pub fn is_stale(&self) -> bool {
        match self.access_expiry_ms {
            Some(exp) => chrono::Utc::now().timestamp_millis() > exp - 60_000,
            None => false,
        }
    }

    /// Rotation never clears a field the response didn't mention.
    pub fn merge(&self, other: SessionTokens) -> SessionTokens {
        SessionTokens {
            access_token: other.access_token.or_else(|| self.access_token.clone()),
            refresh_token: other.refresh_token.or_else(|| self.refresh_token.clone()),
            access_expiry_ms: other.access_expiry_ms.or(self.access_expiry_ms),
            user_id: other.user_id.or_else(|| self.user_id.clone()),
        }
    }
}

/// What a response's `St-*` / `Front-Token` headers said, or `None` when it
/// carried none — so "no rotation" is distinguishable from "rotated to nothing".
pub fn tokens_from_headers(headers: &ureq::http::HeaderMap) -> Option<SessionTokens> {
    let one = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let access = one("st-access-token");
    let refresh = one("st-refresh-token");
    let front = one("front-token");
    if access.is_none() && refresh.is_none() && front.is_none() {
        return None;
    }
    let (expiry, uid) = front.as_deref().map(parse_front_token).unwrap_or_default();
    Some(SessionTokens {
        access_token: access,
        refresh_token: refresh,
        access_expiry_ms: expiry,
        user_id: uid,
    })
}

/// Front-Token is base64 JSON: `{"ate": <ms>, "uid": "…", "up": {claims}}`.
/// A malformed one only costs us pre-emptive refresh, never the request.
pub fn parse_front_token(value: &str) -> (Option<i64>, Option<String>) {
    let engine = base64::engine::general_purpose::STANDARD;
    let trimmed = value.trim_end_matches('=');
    let padded = format!("{trimmed}{}", "=".repeat((4 - trimmed.len() % 4) % 4));
    let Ok(bytes) = engine.decode(padded) else {
        return (None, None);
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (None, None);
    };
    let ate = json.get("ate").and_then(|v| v.as_f64()).map(|v| v as i64);
    let uid = json.get("uid").and_then(|v| v.as_str()).map(str::to_owned);
    (ate, uid)
}

/// Both session gates are matched by *shape*: an ordinary 403 must never be
/// mistaken for `st-mfa-ok` or `st-ev`.
pub fn has_claim_error(body: &serde_json::Value, claim: &str) -> bool {
    body.get("claimValidationErrors")
        .and_then(|v| v.as_array())
        .is_some_and(|errors| {
            errors
                .iter()
                .any(|e| e.get("id").and_then(|id| id.as_str()) == Some(claim))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_token_parses_expiry_and_uid() {
        let raw = r#"{"ate":1788913606000,"uid":"065216ed","up":{}}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(
            parse_front_token(encoded.trim_end_matches('=')),
            (Some(1788913606000), Some("065216ed".into()))
        );
        assert_eq!(parse_front_token("not base64!"), (None, None));
    }

    #[test]
    fn merge_keeps_fields_the_response_did_not_mention() {
        let old = SessionTokens {
            access_token: Some("a1".into()),
            refresh_token: Some("r1".into()),
            access_expiry_ms: Some(1),
            user_id: Some("u".into()),
        };
        let merged = old.merge(SessionTokens {
            access_token: Some("a2".into()),
            ..Default::default()
        });
        assert_eq!(merged.access_token.as_deref(), Some("a2"));
        assert_eq!(merged.refresh_token.as_deref(), Some("r1"));
        assert_eq!(merged.user_id.as_deref(), Some("u"));
    }

    #[test]
    fn gates_are_matched_by_shape() {
        let mfa = serde_json::json!({
            "message": "invalid claim",
            "claimValidationErrors": [{"id": "st-mfa-ok", "reason": {}}]
        });
        assert!(has_claim_error(&mfa, "st-mfa-ok"));
        assert!(!has_claim_error(&mfa, "st-ev"));
        assert!(!has_claim_error(
            &serde_json::json!({"detail": "nope"}),
            "st-mfa-ok"
        ));
    }
}
