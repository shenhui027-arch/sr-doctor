//! Task 72 (design C1, approved 2026-11-05): one StarRocks connection saved
//! on this server so the page connects on open and collection keeps running
//! without a page.
//!
//! - The password is sealed with the notification master key (AES-256-GCM,
//!   purpose `connection:starrocks`) and is write-only: no response, log or
//!   export carries it.
//! - One supervisor keeps one background session open with it and registers
//!   the single bounded collector on that session (user, 2026-11-05: "数据采集
//!   不能断开"). A lost session is reopened with backoff; a refused password is
//!   marked "needs re-entry" and never tried again; remote clusters still wait
//!   for G1.
//! - Saving is refused while a network deployment keeps the public default
//!   login password, since the login would then unlock the cluster.
use std::sync::Mutex;

use doctor_contracts::{
    SavedConnectionBackground as Background, SavedConnectionState,
    SavedConnectionUnavailableReason as Unavailable, SavedConnectionView, TlsMode,
};
use doctor_history::StoredConnection;

use super::*;
use crate::history::unix_now;

const PURPOSE: &str = "connection:starrocks";
/// How often the supervisor checks (and keeps alive) its background session.
const WATCH_PERIOD: Duration = Duration::from_secs(30);
const FIRST_RETRY_SECONDS: u64 = 60;
const MAX_RETRY_SECONDS: u64 = 1_800;

pub(crate) struct SavedConnectionRuntime {
    changed: tokio::sync::Notify,
    background: Mutex<Background>,
    /// The background session while one is open, so a clear can stop its collector at once.
    background_id: Mutex<Option<String>>,
}

impl SavedConnectionRuntime {
    pub(crate) fn new() -> Self {
        Self {
            changed: tokio::sync::Notify::new(),
            background: Mutex::new(Background::Off),
            background_id: Mutex::new(None),
        }
    }

    fn set(&self, background: Background) {
        *self.background.lock().unwrap_or_else(|p| p.into_inner()) = background;
    }

    pub(crate) fn background_id(&self) -> Option<String> {
        self.background_id
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn set_background_id(&self, id: Option<String>) {
        *self.background_id.lock().unwrap_or_else(|p| p.into_inner()) = id;
    }

    pub(crate) fn background(&self) -> Background {
        *self.background.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wakes the supervisor after a save or a forget.
    pub(crate) fn changed(&self) {
        self.changed.notify_one();
    }
}

fn unavailable(state: &ApiState) -> Option<Unavailable> {
    if state.history.store().is_none() {
        Some(Unavailable::HistoryUnavailable)
    } else if state.config.notify_key.is_none() {
        Some(Unavailable::KeyUnavailable)
    } else if state.config.deployment_mode == DeploymentMode::Network
        // The password alone decides (task 72 review): a renamed account that
        // keeps the public default password is just as open.
        && state
            .config
            .access
            .as_ref()
            .is_some_and(|access| access.password == crate::access::DEFAULT_ADMIN_PASSWORD)
    {
        Some(Unavailable::DefaultAdminPassword)
    } else {
        None
    }
}

fn tls_text(mode: &TlsMode) -> Option<&'static str> {
    match mode {
        TlsMode::Plaintext => Some("plaintext"),
        TlsMode::VerifyFull => Some("verify_full"),
        TlsMode::DockerLoopbackPlaintext => None,
    }
}

fn tls_mode(text: &str) -> Option<TlsMode> {
    match text {
        "plaintext" => Some(TlsMode::Plaintext),
        "verify_full" => Some(TlsMode::VerifyFull),
        _ => None,
    }
}

fn saved_failure(state: &ApiState, status: StatusCode, code: &'static str) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        status,
        code,
        "saved_connection",
        code,
        "change_input",
    )
}

pub(crate) fn view(state: &ApiState) -> Result<SavedConnectionView, ApiFailure> {
    let reason = unavailable(state);
    let stored = match state.history.store() {
        Some(store) => store
            .saved_connection()
            .map_err(|_| internal_failure(state))?,
        None => None,
    };
    let saved_state = match (&reason, &stored) {
        (Some(_), _) => SavedConnectionState::Unavailable,
        (None, None) => SavedConnectionState::None,
        (None, Some(saved)) if saved.needs_reentry => SavedConnectionState::NeedsReentry,
        (None, Some(_)) => SavedConnectionState::Saved,
    };
    Ok(SavedConnectionView {
        state: saved_state,
        unavailable_reason: reason,
        host: stored.as_ref().map(|saved| saved.host.clone()),
        port: stored.as_ref().map(|saved| saved.port),
        username: stored.as_ref().map(|saved| saved.username.clone()),
        tls_mode: stored.as_ref().and_then(|saved| tls_mode(&saved.tls_mode)),
        background: state.saved.background(),
    })
}

/// Checked before connecting, so a refused save never opens a session.
pub(crate) fn ensure_can_save(
    state: &ApiState,
    request: &ConnectionCreateRequest,
) -> Result<(), ApiFailure> {
    if unavailable(state).is_some() {
        return Err(saved_failure(
            state,
            StatusCode::CONFLICT,
            "saved_connection_unavailable",
        ));
    }
    if tls_text(&request.tls_mode).is_none() {
        return Err(saved_failure(
            state,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ));
    }
    Ok(())
}

/// Seals the password of a connection that just succeeded and wakes the
/// supervisor. The fields are what the user typed; nothing is echoed.
pub(crate) fn save(
    state: &ApiState,
    host: &str,
    port: u16,
    username: &str,
    tls: TlsMode,
    password: &str,
) -> Result<(), ApiFailure> {
    let (Some(store), Some(key), Some(tls)) = (
        state.history.store(),
        state.config.notify_key.as_ref(),
        tls_text(&tls),
    ) else {
        return Err(saved_failure(
            state,
            StatusCode::CONFLICT,
            "saved_connection_unavailable",
        ));
    };
    let secret = key
        .key
        .seal(PURPOSE, password.as_bytes())
        .ok_or_else(|| internal_failure(state))?;
    store
        .save_connection(&StoredConnection {
            host: host.trim().to_owned(),
            port,
            username: username.trim().to_owned(),
            tls_mode: tls.to_owned(),
            secret,
            needs_reentry: false,
            updated_at: unix_now(),
        })
        .map_err(|_| internal_failure(state))?;
    state.saved.changed();
    Ok(())
}

enum Loaded {
    Nothing,
    Ready {
        request: Box<ConnectionCreateRequest>,
        host: String,
        port: u16,
        /// The sealed blob that was opened, so a failure marks only this one.
        tried: Vec<u8>,
    },
}

/// The saved connection as a connect request, or nothing when it is
/// missing, unusable or needs re-entry. A blob that no longer opens (another
/// master key, damage) is marked for re-entry instead of being retried.
fn load(state: &ApiState) -> Loaded {
    if unavailable(state).is_some() {
        return Loaded::Nothing;
    }
    let (Some(store), Some(key)) = (state.history.store(), state.config.notify_key.as_ref()) else {
        return Loaded::Nothing;
    };
    let Ok(Some(saved)) = store.saved_connection() else {
        return Loaded::Nothing;
    };
    if saved.needs_reentry {
        return Loaded::Nothing;
    }
    let password = key
        .key
        .open(PURPOSE, &saved.secret)
        .and_then(|plain| String::from_utf8(plain).ok());
    let (Some(password), Some(tls)) = (password, tls_mode(&saved.tls_mode)) else {
        let _ = store.mark_saved_connection_needs_reentry(&saved.secret, unix_now());
        return Loaded::Nothing;
    };
    Loaded::Ready {
        request: Box::new(ConnectionCreateRequest {
            host: saved.host.clone(),
            port: saved.port,
            username: saved.username.clone(),
            password,
            catalog: None,
            database: None,
            audit_table: None,
            tls_mode: tls,
            save_credentials: None,
        }),
        host: saved.host,
        port: saved.port,
        tried: saved.secret,
    }
}

/// Whether the connection that was opened is still the one saved and usable.
fn still_saved(state: &ApiState, tried: &[u8]) -> bool {
    unavailable(state).is_none()
        && state
            .history
            .store()
            .and_then(|store| store.saved_connection().ok().flatten())
            .is_some_and(|saved| !saved.needs_reentry && saved.secret == tried)
}

fn refused(error: &ConnectorError) -> bool {
    error.code == ConnectorErrorCode::AuthenticationFailed
}

fn mark_needs_reentry(state: &ApiState, tried: &[u8]) {
    if let Some(store) = state.history.store() {
        let _ = store.mark_saved_connection_needs_reentry(tried, unix_now());
    }
}

/// GET: whether a connection is saved; never the password.
pub(crate) async fn read(State(state): State<ApiState>) -> Result<Response, ApiFailure> {
    Ok(crate::audit::no_store(view(&state)?))
}

/// DELETE: forget it. The background session and its collector stop.
pub(crate) async fn forget(State(state): State<ApiState>) -> Result<StatusCode, ApiFailure> {
    let Some(store) = state.history.store() else {
        return Ok(StatusCode::NO_CONTENT);
    };
    store
        .forget_saved_connection()
        .map_err(|_| internal_failure(&state))?;
    state.saved.changed();
    Ok(StatusCode::NO_CONTENT)
}

/// POST: open a page session with the saved connection (the page does this
/// on open). A refused password marks it for re-entry.
pub(crate) async fn connect(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Json<PublicConnectionSessionView>, ApiFailure> {
    let _: serde_json::Value = connector_json_request(&state, request).await?;
    let loader = state.clone();
    let Loaded::Ready {
        request,
        host,
        port,
        tried,
    } = tokio::task::spawn_blocking(move || load(&loader))
        .await
        .map_err(|_| internal_failure(&state))?
    else {
        return Err(saved_failure(
            &state,
            StatusCode::CONFLICT,
            "saved_connection_unavailable",
        ));
    };
    let remote = !doctor_connector::is_literal_loopback(&host);
    let sessions = Arc::clone(&state.sessions);
    let result = tokio::task::spawn_blocking(move || {
        sessions.connect(connector_connection_request(*request))
    })
    .await
    .map_err(|_| internal_failure(&state))?;
    let session = match result {
        Ok(session) => session,
        Err(error) => {
            if refused(&error) {
                mark_needs_reentry(&state, &tried);
                state.saved.changed();
            }
            return Err(connector_failure(&state, error));
        }
    };
    history::start_collector(&state, &session.connection_id, &host, port, remote);
    Ok(Json(public_connection_view(session)))
}

/// Waits for a save or forget, or for `delay` to pass. Returns whether the
/// saved connection changed.
async fn changed_within(state: &ApiState, delay: Duration) -> bool {
    tokio::select! {
        _ = state.saved.changed.notified() => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

/// Keeps one background session open with the saved connection while one is
/// saved, and registers the collector on it.
pub(crate) async fn supervisor(state: ApiState) {
    let mut retry = FIRST_RETRY_SECONDS;
    loop {
        let loader = state.clone();
        let loaded = tokio::task::spawn_blocking(move || load(&loader))
            .await
            .unwrap_or(Loaded::Nothing);
        let Loaded::Ready {
            request,
            host,
            port,
            tried,
        } = loaded
        else {
            state.saved.set(Background::Off);
            state.saved.changed.notified().await;
            retry = FIRST_RETRY_SECONDS;
            continue;
        };
        let remote = !doctor_connector::is_literal_loopback(&host);
        if remote && !state.history.config.allow_remote_collection {
            state.saved.set(Background::ProductionGate);
            state.saved.changed.notified().await;
            continue;
        }
        state.saved.set(Background::Connecting);
        let sessions = Arc::clone(&state.sessions);
        let result = tokio::task::spawn_blocking(move || {
            sessions.connect(connector_connection_request(*request))
        })
        .await;
        let session = match result {
            Ok(Ok(session)) => session,
            Ok(Err(error)) if refused(&error) => {
                mark_needs_reentry(&state, &tried);
                eprintln!("saved_connection_refused code=authentication_failed");
                continue;
            }
            Ok(Err(error)) => {
                eprintln!("saved_connection_retry stage={}", error.stage);
                state.saved.set(Background::Retrying);
                if !changed_within(&state, Duration::from_secs(retry)).await {
                    retry = (retry * 2).min(MAX_RETRY_SECONDS);
                }
                continue;
            }
            Err(_) => {
                state.saved.set(Background::Retrying);
                changed_within(&state, Duration::from_secs(retry)).await;
                continue;
            }
        };
        let id = session.connection_id.clone();
        if session.state != ConnectionState::Connected {
            close(&state, &id).await;
            state.saved.set(Background::UnsupportedVersion);
            state.saved.changed.notified().await;
            continue;
        }
        retry = FIRST_RETRY_SECONDS;
        // Forgotten, replaced or cleared while connecting: this session is not
        // the saved connection any more (task 72 review). The check and the
        // registration share the cycle gate with a clear, which takes it
        // exclusively: a clear either comes first (the check fails) or sees
        // the registered collector and stops it.
        let registered = {
            let _gate = state
                .history
                .cycle_gate
                .read()
                .unwrap_or_else(|poison| poison.into_inner());
            let saved = still_saved(&state, &tried);
            if saved {
                history::start_collector(&state, &id, &host, port, remote);
                state.saved.set_background_id(Some(id.clone()));
            }
            saved
        };
        if !registered {
            close(&state, &id).await;
            continue;
        }
        let report = |state: &ApiState| {
            state.saved.set(if state.history.collector_running(&id) {
                Background::Collecting
            } else {
                Background::Standby
            })
        };
        report(&state);
        // Touching the session keeps it open while collection is paused or
        // waits behind a page session; a session the server dropped (remote
        // timeout, broken connection) is reopened.
        let changed = loop {
            if changed_within(&state, WATCH_PERIOD).await {
                break true;
            }
            let sessions = Arc::clone(&state.sessions);
            let watched = id.clone();
            let alive = tokio::task::spawn_blocking(move || sessions.resume(&watched).is_ok())
                .await
                .unwrap_or(false);
            if !alive {
                break false;
            }
            report(&state);
        };
        // Stop first, then withdraw the ID, under the cycle gate: a clear sees
        // either the ID (and stops the collector itself) or a stopped collector.
        {
            let _gate = state
                .history
                .cycle_gate
                .read()
                .unwrap_or_else(|poison| poison.into_inner());
            history::stop_collector(&state, &id);
            state.saved.set_background_id(None);
        }
        close(&state, &id).await;
        if !changed {
            state.saved.set(Background::Retrying);
            if changed_within(&state, Duration::from_secs(retry)).await {
                retry = FIRST_RETRY_SECONDS;
            }
        }
    }
}

async fn close(state: &ApiState, connection_id: &str) {
    let sessions = Arc::clone(&state.sessions);
    let id = connection_id.to_owned();
    let _ = tokio::task::spawn_blocking(move || sessions.close(&id)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c1_tls_modes_round_trip_and_the_docker_lab_mode_is_never_saved() {
        for mode in [TlsMode::Plaintext, TlsMode::VerifyFull] {
            assert_eq!(tls_mode(tls_text(&mode).unwrap()), Some(mode));
        }
        assert_eq!(tls_text(&TlsMode::DockerLoopbackPlaintext), None);
        assert_eq!(tls_mode("docker_loopback_plaintext"), None);
    }
}
