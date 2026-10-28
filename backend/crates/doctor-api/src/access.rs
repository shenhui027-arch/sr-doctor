//! Network deployments: one operator-configured access token, exchanged once
//! for an HttpOnly session cookie. The token is never logged, echoed or
//! written anywhere; sessions live only in this process and end on restart.
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, header::COOKIE};

/// The cookie that carries a session id after a successful login.
pub(crate) const SESSION_COOKIE: &str = "doctor_session";
/// Shorter tokens are refused at startup: they are guessable.
pub const MIN_TOKEN_CHARS: usize = 16;
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
const MAX_SESSIONS: usize = 256;
/// Failed logins allowed in `FAILURE_WINDOW` before logins are throttled.
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Eq, PartialEq)]
pub struct AccessConfig {
    pub token: String,
    /// Add `Secure` to the cookie; set when browsers reach the server over HTTPS.
    pub secure_cookie: bool,
}

impl std::fmt::Debug for AccessConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccessConfig")
            .field("token", &"<redacted>")
            .field("secure_cookie", &self.secure_cookie)
            .finish()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum LoginError {
    Invalid,
    Throttled,
}

pub(crate) struct AccessGate {
    token: Box<[u8]>,
    secure_cookie: bool,
    sessions: Mutex<HashMap<String, Instant>>,
    failures: Mutex<VecDeque<Instant>>,
}

impl AccessGate {
    pub(crate) fn new(config: &AccessConfig) -> Self {
        Self {
            token: config.token.as_bytes().into(),
            secure_cookie: config.secure_cookie,
            sessions: Mutex::new(HashMap::new()),
            failures: Mutex::new(VecDeque::new()),
        }
    }

    /// Whether the request carries a live session cookie.
    pub(crate) fn authenticated(&self, headers: &HeaderMap) -> bool {
        let Some(id) = session_cookie(headers) else {
            return false;
        };
        let mut sessions = self.sessions.lock().expect("access sessions");
        let now = Instant::now();
        sessions.retain(|_, expires| *expires > now);
        sessions.contains_key(id)
    }

    /// Checks the token and returns the `Set-Cookie` value of a new session.
    pub(crate) fn login(&self, token: &str) -> Result<String, LoginError> {
        let now = Instant::now();
        {
            let mut failures = self.failures.lock().expect("access failures");
            while failures
                .front()
                .is_some_and(|at| now.duration_since(*at) > FAILURE_WINDOW)
            {
                failures.pop_front();
            }
            if failures.len() >= MAX_FAILURES {
                return Err(LoginError::Throttled);
            }
            if !constant_time_eq(token.as_bytes(), &self.token) {
                failures.push_back(now);
                return Err(LoginError::Invalid);
            }
        }
        let id = random_id().map_err(|_| LoginError::Invalid)?;
        let mut sessions = self.sessions.lock().expect("access sessions");
        sessions.retain(|_, expires| *expires > now);
        if sessions.len() >= MAX_SESSIONS {
            // Drop the session closest to expiry rather than refusing the login.
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, expires)| **expires)
                .map(|(id, _)| id.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(id.clone(), now + SESSION_TTL);
        Ok(self.cookie(&id, SESSION_TTL.as_secs()))
    }

    /// Ends the request's session and returns a `Set-Cookie` that clears it.
    pub(crate) fn logout(&self, headers: &HeaderMap) -> String {
        if let Some(id) = session_cookie(headers) {
            self.sessions.lock().expect("access sessions").remove(id);
        }
        self.cookie("", 0)
    }

    fn cookie(&self, value: &str, max_age: u64) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
        )
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| *name == SESSION_COOKIE && !value.is_empty())
        .map(|(_, value)| value)
}

/// Compares every byte whatever the first difference, so timing reveals
/// nothing beyond the length.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn random_id() -> Result<String, getrandom::Error> {
    let mut entropy = [0_u8; 32];
    getrandom::fill(&mut entropy)?;
    Ok(entropy.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn gate() -> AccessGate {
        AccessGate::new(&AccessConfig {
            token: "correct-horse-battery-staple".into(),
            secure_cookie: false,
        })
    }

    fn with_cookie(set_cookie: &str) -> HeaderMap {
        let pair = set_cookie.split(';').next().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("theme=light; {pair}")).unwrap(),
        );
        headers
    }

    #[test]
    fn a_correct_token_opens_an_http_only_strict_session() {
        let gate = gate();
        let cookie = gate.login("correct-horse-battery-staple").unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
        assert!(
            !cookie.contains("correct-horse"),
            "the token never reaches the cookie"
        );
        assert!(gate.authenticated(&with_cookie(&cookie)));
        assert!(!gate.authenticated(&HeaderMap::new()));
        assert!(!gate.authenticated(&with_cookie("doctor_session=forged")));
    }

    #[test]
    fn logout_ends_the_session() {
        let gate = gate();
        let cookie = gate.login("correct-horse-battery-staple").unwrap();
        let headers = with_cookie(&cookie);
        assert!(gate.logout(&headers).contains("Max-Age=0"));
        assert!(!gate.authenticated(&headers));
    }

    #[test]
    fn repeated_wrong_tokens_throttle_even_the_right_one() {
        let gate = gate();
        for _ in 0..MAX_FAILURES {
            assert_eq!(
                gate.login("wrong-token-wrong-token"),
                Err(LoginError::Invalid)
            );
        }
        assert_eq!(
            gate.login("correct-horse-battery-staple"),
            Err(LoginError::Throttled)
        );
    }

    #[test]
    fn secure_cookie_is_added_only_when_configured() {
        let gate = AccessGate::new(&AccessConfig {
            token: "correct-horse-battery-staple".into(),
            secure_cookie: true,
        });
        assert!(
            gate.login("correct-horse-battery-staple")
                .unwrap()
                .ends_with("; Secure")
        );
        assert!(
            format!(
                "{:?}",
                AccessConfig {
                    token: "secret-value-0000".into(),
                    secure_cookie: false
                }
            )
            .contains("<redacted>")
        );
    }

    #[test]
    fn comparison_rejects_different_lengths_and_bytes() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
