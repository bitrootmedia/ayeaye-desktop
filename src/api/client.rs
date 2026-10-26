//! One instance's HTTP client, with the SuperTokens header-mode lifecycle
//! hand-rolled — the same design as the mobile app's `ApiClient`.
//!
//! Each profile owns its own client and its own token set; there is no shared
//! "current session" anywhere. Calls are blocking (`ureq`) and are made from
//! GPUI's background executor, so a slow server never stalls a frame.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ureq::RequestBuilder;

use super::session::{SessionTokens, has_claim_error, tokens_from_headers};

#[derive(Clone, Debug, PartialEq)]
pub enum ApiError {
    /// Transport failure: DNS, refused connection, timeout, TLS.
    Network(String),
    /// The refresh token was refused; the profile must sign in again.
    SessionExpired,
    /// `st-mfa-ok` claim: a TOTP code (or backup code) unlocks the session.
    MfaRequired,
    /// `st-ev` claim: the person must open the link in their email.
    EmailVerificationRequired,
    /// No access reads as 404 on this API, so "gone" and "revoked" look alike.
    NotFound(String),
    Forbidden(String),
    Server {
        status: u16,
        message: String,
    },
    Decode(String),
}

impl ApiError {
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            ApiError::SessionExpired | ApiError::MfaRequired | ApiError::EmailVerificationRequired
        )
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(m) => f.write_str(m),
            ApiError::SessionExpired => f.write_str("Your session has expired — sign in again"),
            ApiError::MfaRequired => f.write_str("Two-factor authentication is required"),
            ApiError::EmailVerificationRequired => f.write_str("Verify your email address first"),
            ApiError::NotFound(m) | ApiError::Forbidden(m) => f.write_str(m),
            ApiError::Server { message, .. } => f.write_str(message),
            ApiError::Decode(m) => write!(f, "Unexpected response: {m}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

/// What `/api/auth/signin` said. SuperTokens answers 200 either way and puts
/// the verdict in `status`.
#[derive(Debug, PartialEq)]
pub enum SignInOutcome {
    SignedIn,
    WrongCredentials,
    NotAllowed(String),
    FieldErrors(Vec<String>),
}

type TokensChanged = Box<dyn Fn(&SessionTokens) + Send + Sync>;

pub struct ApiClient {
    base_url: String,
    agent: ureq::Agent,
    tokens: Mutex<SessionTokens>,
    /// Held across a refresh so concurrent requests collapse into one: five
    /// requests that all find a stale token must not mint five sessions.
    refresh_lock: Mutex<()>,
    on_tokens_changed: TokensChanged,
}

impl ApiClient {
    pub fn new(
        base_url: impl Into<String>,
        tokens: SessionTokens,
        on_tokens_changed: impl Fn(&SessionTokens) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // We interpret every status ourselves: the headers on a 4xx, and
            // on a bodyless 204, still have to be read.
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("ayeaye-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Arc::new(Self {
            base_url: normalise_base_url(&base_url.into()),
            agent,
            tokens: Mutex::new(tokens),
            refresh_lock: Mutex::new(()),
            on_tokens_changed: Box::new(on_tokens_changed),
        })
    }

    pub fn tokens(&self) -> SessionTokens {
        self.tokens.lock().unwrap().clone()
    }

    pub fn has_session(&self) -> bool {
        self.tokens.lock().unwrap().has_session()
    }

    // ------------------------------------------------------------- requests

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        decode(self.send("GET", path, None)?)
    }

    /// A page of a list, with the `X-Total-Count` the server sends when asked
    /// for a `limit` — what the page is a page *of*.
    pub fn get_counted<T: DeserializeOwned>(&self, path: &str) -> ApiResult<(T, Option<usize>)> {
        let response = self.exchange("GET", path, None)?;
        let total = response.total_count;
        Ok((decode(check_status(response)?)?, total))
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> ApiResult<T> {
        decode(self.send("POST", path, Some(to_value(body)?))?)
    }

    pub fn post_empty(&self, path: &str, body: &impl Serialize) -> ApiResult<()> {
        self.send("POST", path, Some(to_value(body)?)).map(|_| ())
    }

    pub fn patch<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> ApiResult<T> {
        decode(self.send("PATCH", path, Some(to_value(body)?))?)
    }

    pub fn put_empty(&self, path: &str, body: &impl Serialize) -> ApiResult<()> {
        self.send("PUT", path, Some(to_value(body)?)).map(|_| ())
    }

    pub fn delete(&self, path: &str) -> ApiResult<()> {
        self.send("DELETE", path, None).map(|_| ())
    }

    // ----------------------------------------------------------------- auth

    pub fn sign_in(&self, email: &str, password: &str) -> ApiResult<SignInOutcome> {
        let body = json!({"formFields": [
            {"id": "email", "value": email},
            {"id": "password", "value": password},
        ]});
        let response = self.raw(
            "POST",
            "/api/auth/signin",
            Some(&body),
            None,
            // Only the recipe routes want `rid`; /api/* does not.
            &[("rid", "emailpassword")],
        )?;
        let status = response.body.get("status").and_then(Value::as_str);
        Ok(match status {
            Some("OK") if self.has_session() => SignInOutcome::SignedIn,
            Some("WRONG_CREDENTIALS_ERROR") => SignInOutcome::WrongCredentials,
            Some("SIGN_IN_NOT_ALLOWED") => SignInOutcome::NotAllowed(
                response
                    .body
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("This account can't sign in right now")
                    .to_owned(),
            ),
            Some("FIELD_ERROR") => SignInOutcome::FieldErrors(
                response
                    .body
                    .get("formFields")
                    .and_then(Value::as_array)
                    .map(|fields| {
                        fields
                            .iter()
                            .filter_map(|f| f.get("error").and_then(Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            _ => {
                return Err(ApiError::Server {
                    status: response.status,
                    message: message_from_body(&response.body)
                        .unwrap_or_else(|| format!("Sign-in failed ({})", response.status)),
                });
            }
        })
    }

    /// Answers the `st-mfa-ok` gate. The 204 carries a *new* access token and
    /// the old one keeps returning 403 forever — `raw` absorbs it.
    pub fn totp_challenge(&self, code: &str) -> ApiResult<()> {
        self.post_empty("/api/me/mfa/totp/challenge", &json!({"code": code}))
    }

    pub fn redeem_backup_code(&self, code: &str) -> ApiResult<()> {
        self.post_empty("/api/me/mfa/backup-codes/redeem", &json!({"code": code}))
    }

    pub fn sign_out(&self) {
        // Best effort: the profile is going either way.
        let _ = self.send("POST", "/api/auth/signout", Some(json!({})));
        self.set_tokens(SessionTokens::default());
    }

    /// Rotates both tokens. Also the step that picks up a freshly verified
    /// email: the access token carries the stale claim until refreshed.
    pub fn refresh_session(&self) -> ApiResult<()> {
        let before = self.tokens();
        let _guard = self.refresh_lock.lock().unwrap();
        // Somebody else refreshed while we waited for the lock.
        if self.tokens().access_token != before.access_token && self.has_session() {
            return Ok(());
        }
        let Some(refresh) = before.refresh_token.filter(|t| !t.is_empty()) else {
            self.fail_session();
            return Err(ApiError::SessionExpired);
        };
        let response = self.raw(
            "POST",
            "/api/auth/session/refresh",
            None,
            Some(&refresh),
            &[],
        )?;
        match response.status {
            401 | 403 => {
                self.fail_session();
                Err(ApiError::SessionExpired)
            }
            s if s >= 400 => Err(ApiError::Server {
                status: s,
                message: message_from_body(&response.body)
                    .unwrap_or_else(|| "Could not refresh the session".into()),
            }),
            _ => Ok(()),
        }
    }

    fn fail_session(&self) {
        self.set_tokens(SessionTokens::default());
    }

    fn set_tokens(&self, tokens: SessionTokens) {
        *self.tokens.lock().unwrap() = tokens.clone();
        (self.on_tokens_changed)(&tokens);
    }

    // ------------------------------------------------------------- plumbing

    fn send(&self, method: &str, path: &str, body: Option<Value>) -> ApiResult<Value> {
        check_status(self.exchange(method, path, body)?)
    }

    /// One request with the session kept alive around it: refreshed first if
    /// stale, retried once after a 401.
    fn exchange(&self, method: &str, path: &str, body: Option<Value>) -> ApiResult<RawResponse> {
        let tokens = self.tokens();
        // Pre-emptive refresh: cheaper than a round trip we know will 401.
        if tokens.has_session() && tokens.is_stale() && tokens.has_refresh() {
            // Any other failure: let the request produce the real error.
            if let Err(ApiError::SessionExpired) = self.refresh_session() {
                return Err(ApiError::SessionExpired);
            }
        }

        let mut response = self.raw(method, path, body.as_ref(), None, &[])?;
        if response.status == 401 && self.tokens().has_refresh() {
            self.refresh_session()?;
            response = self.raw(method, path, body.as_ref(), None, &[])?;
        }
        Ok(response)
    }

    /// One HTTP exchange. Reads the `St-*` headers off **every** response,
    /// 204s included — the expensive gotcha in this whole flow.
    fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        bearer_override: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> ApiResult<RawResponse> {
        let url = format!("{}{}", self.base_url, path);
        let bearer = match bearer_override {
            Some(token) => Some(token.to_owned()),
            None => self.tokens().access_token.filter(|t| !t.is_empty()),
        };
        fn headers<B>(
            mut request: RequestBuilder<B>,
            bearer: &Option<String>,
            extra: &[(&str, &str)],
        ) -> RequestBuilder<B> {
            request = request
                .header("st-auth-mode", "header")
                .header("Accept", "application/json");
            if let Some(token) = bearer {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            for (k, v) in extra {
                request = request.header(*k, *v);
            }
            request
        }

        let result = match (method, body) {
            ("GET", _) => headers(self.agent.get(&url), &bearer, extra_headers).call(),
            ("DELETE", _) => headers(self.agent.delete(&url), &bearer, extra_headers).call(),
            ("POST", Some(b)) => {
                headers(self.agent.post(&url), &bearer, extra_headers).send_json(b)
            }
            ("POST", None) => headers(self.agent.post(&url), &bearer, extra_headers).send_empty(),
            ("PATCH", Some(b)) => {
                headers(self.agent.patch(&url), &bearer, extra_headers).send_json(b)
            }
            ("PUT", Some(b)) => headers(self.agent.put(&url), &bearer, extra_headers).send_json(b),
            ("PUT", None) => headers(self.agent.put(&url), &bearer, extra_headers).send_empty(),
            ("PATCH", None) => headers(self.agent.patch(&url), &bearer, extra_headers).send_empty(),
            _ => unreachable!("unsupported method {method}"),
        };
        let mut response = result.map_err(|e| self.network_error(e))?;

        if let Some(incoming) = tokens_from_headers(response.headers()) {
            let merged = self.tokens().merge(incoming);
            self.set_tokens(merged);
        }

        let status = response.status().as_u16();
        let total_count = response
            .headers()
            .get("x-total-count")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        let text = response.body_mut().read_to_string().unwrap_or_default();
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok(RawResponse {
            status,
            body,
            total_count,
        })
    }

    fn network_error(&self, error: ureq::Error) -> ApiError {
        let host = self.base_url.as_str();
        ApiError::Network(match error {
            ureq::Error::Timeout(_) => format!("{host} took too long to answer"),
            ureq::Error::HostNotFound => format!("Could not find {host}"),
            ureq::Error::Tls(_) | ureq::Error::Rustls(_) => {
                format!("{host}'s certificate could not be verified")
            }
            other => {
                log::debug!("network error: {other}");
                format!("Could not reach {host}")
            }
        })
    }
}

struct RawResponse {
    status: u16,
    body: Value,
    total_count: Option<usize>,
}

fn check_status(response: RawResponse) -> ApiResult<Value> {
    let RawResponse { status, body, .. } = response;
    match status {
        200..=299 => Ok(body),
        403 if has_claim_error(&body, "st-mfa-ok") => Err(ApiError::MfaRequired),
        403 if has_claim_error(&body, "st-ev") => Err(ApiError::EmailVerificationRequired),
        403 => Err(ApiError::Forbidden(
            message_from_body(&body).unwrap_or_else(|| "Not allowed".into()),
        )),
        401 => Err(ApiError::SessionExpired),
        404 => Err(ApiError::NotFound(
            message_from_body(&body).unwrap_or_else(|| "Not found".into()),
        )),
        _ => Err(ApiError::Server {
            status,
            message: message_from_body(&body)
                .unwrap_or_else(|| format!("Something went wrong ({status})")),
        }),
    }
}

/// FastAPI says `detail` (sometimes a list of validation errors), SuperTokens
/// says `message`. A parser that reads one shows blank errors for the other.
pub fn message_from_body(body: &Value) -> Option<String> {
    if let Some(detail) = body.get("detail") {
        if let Some(s) = detail.as_str() {
            return Some(s.to_owned());
        }
        if let Some(first) = detail.as_array().and_then(|a| a.first())
            && let Some(msg) = first.get("msg").and_then(Value::as_str)
        {
            return Some(msg.to_owned());
        }
    }
    body.get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| body.as_str().map(|s| s.chars().take(200).collect()))
}

fn to_value(body: &impl Serialize) -> ApiResult<Value> {
    serde_json::to_value(body).map_err(|e| ApiError::Decode(e.to_string()))
}

fn decode<T: DeserializeOwned>(value: Value) -> ApiResult<T> {
    serde_json::from_value(value).map_err(|e| ApiError::Decode(e.to_string()))
}

/// What a person types into the server field. `http://` is allowed — self
/// hosters run plain localhost — and warned about elsewhere, never blocked.
pub fn normalise_base_url(input: &str) -> String {
    let mut value = input.trim().to_owned();
    if value.is_empty() {
        return value;
    }
    if !value.starts_with("http://") && !value.starts_with("https://") {
        value = format!("https://{value}");
    }
    while value.ends_with('/') {
        value.pop();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_is_normalised() {
        assert_eq!(
            normalise_base_url(" tasks.example.com/ "),
            "https://tasks.example.com"
        );
        assert_eq!(normalise_base_url("http://localhost//"), "http://localhost");
    }

    #[test]
    fn both_error_shapes_are_read() {
        assert_eq!(
            message_from_body(&json!({"detail": "a note needs a title"})).as_deref(),
            Some("a note needs a title")
        );
        assert_eq!(
            message_from_body(&json!({"message": "unauthorised"})).as_deref(),
            Some("unauthorised")
        );
        assert_eq!(
            message_from_body(&json!({"detail": [{"msg": "field required"}]})).as_deref(),
            Some("field required")
        );
    }

    #[test]
    fn statuses_map_to_errors() {
        let gate = RawResponse {
            status: 403,
            body: json!({"claimValidationErrors": [{"id": "st-ev"}]}),
            total_count: None,
        };
        assert_eq!(check_status(gate), Err(ApiError::EmailVerificationRequired));
        let plain = RawResponse {
            status: 403,
            body: json!({"detail": "no"}),
            total_count: None,
        };
        assert_eq!(check_status(plain), Err(ApiError::Forbidden("no".into())));
        let missing = RawResponse {
            status: 404,
            body: json!({"detail": "note not found"}),
            total_count: None,
        };
        assert_eq!(
            check_status(missing),
            Err(ApiError::NotFound("note not found".into()))
        );
    }
}
