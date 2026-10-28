//! Local HTTP trust-boundary shell for the offline analysis and comparison closure.

// ApiFailure carries the bounded, public error envelope inline; boxing all handler seams would
// only move this fixed payload to the heap and widen the current API change.
#![allow(clippy::result_large_err)]

mod access;
mod audit;
mod governance;
mod history;
mod history_trend;
pub use history::{HistoryConfig, SlicePlan, plan_slices};

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write as _},
    future::IntoFuture,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{
        ConnectInfo, DefaultBodyLimit, FromRequest, Multipart, Path as AxumPath, Request, State,
        multipart::MultipartError,
    },
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{
            ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
            ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_HEADERS,
            ACCESS_CONTROL_REQUEST_METHOD, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH,
            CONTENT_TYPE, HOST, ORIGIN, SET_COOKIE, VARY,
        },
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, options},
};
use doctor_connector::{
    CapabilityState, ConnectionSessionView as ConnectorSessionView, ConnectionState,
    ConnectorError, ConnectorErrorCode, CreateConnectionRequest, FetchedQueryContext,
    MysqlTextRemoteFactory, QueryBrowseResult, QuerySearch, QuerySource, RemoteFactory,
    RunningProfileCandidates, SessionManager, TlsMode as ConnectorTlsMode, canonical_query_id,
    is_valid_query_id,
};
use doctor_contracts::{
    AccessLoginRequest, AccessSessionStatus, AnalysisAttempt, AnalysisOutcome,
    AnalysisSnapshotReport, BuildCapability, Capabilities, ConnectedComparisonInterpretationState,
    ConnectedComparisonValidationContext, ConnectedPlanCheckAction,
    ConnectedProfileComparisonRequest, ConnectedProfileComparisonResponse,
    ConnectedProfileComparisonSide, ConnectedProfileFetchAction, ConnectedQueryListRequest,
    ConnectedQueryListResponse, ConnectedQueryRef, ConnectedQuerySqlRequest,
    ConnectedQuerySqlResponse, ConnectedTimestampState, ConnectionCapabilityReport,
    ConnectionCapabilityState as PublicCapabilityState, ConnectionCreateRequest,
    ConnectionSessionState, ConnectionSessionView as PublicConnectionSessionView, CsrfCapability,
    ErrorEnvelope, ExplainRequest, ExplainResponse, ExportRequest, InputOrigin,
    PlanAdvisorResponse, PlanCheckAvailableReason, PlanCheckUnavailableReason,
    ProfileFetchAvailableReason, ProfileFetchKind, ProfileFetchRequest, ProfileFetchResponse,
    ProfileFetchState, ProfileFetchUnavailableReason, QueryDiscoverySource, QueryFilterScope,
    RequestedSourceKind, RunningProfileCandidateListRequest, RunningProfileCandidateListResponse,
    RunningProfileSnapshotRequest, RunningProfileSnapshotResponse, RuntimeProfileRequest,
    SCHEMA_VERSION, SourceKind, TimeZoneEvidence, TlsMode as PublicTlsMode, VersionEvidence,
    VersionPackCapability,
};
use doctor_core::{
    AdapterContext, AdapterError, AdapterLimits, CancelSignal, ExplainSnapshotInput, InputEnvelope,
    PublicReportError, VersionResolutionError, adapt_input_cancellable, analysis_attempt,
    assemble_explain_snapshot_for_mode, assemble_public_report_cancellable, compare_attempts,
    evaluate_rules, failed_attempt, is_profile_topology_line,
    resolve_version_and_graph_cancellable, v01_analysis_receipt,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const DEFAULT_BIND_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8000);
/// The only non-loopback listener supported by the product. Docker Compose
/// publishes this container-only socket back to the Windows host loopback.
pub const DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8000);
const ANALYSES_PATH: &str = "/api/v1/analyses";
const COMPARISONS_PATH: &str = "/api/v1/comparisons";
const EXPORTS_PATH: &str = "/api/v1/exports";
const CONNECTIONS_PATH: &str = "/api/v1/connections";
const CONNECTIONS_PREFIX: &str = "/api/v1/connections/";
const CONNECTION_ID_PATH: &str = "/api/v1/connections/{id}";
const CONNECTION_SEARCH_PATH: &str = "/api/v1/connections/{id}/queries/search";
const CONNECTION_QUERY_SQL_PATH: &str = "/api/v1/connections/{id}/queries/sql";
const CONNECTION_PROFILE_PATH: &str = "/api/v1/connections/{id}/profiles/fetch";
const CONNECTION_PROFILE_COMPARE_PATH: &str = "/api/v1/connections/{id}/profiles/compare";
const CONNECTION_RUNNING_PROFILE_CANDIDATES_PATH: &str =
    "/api/v1/connections/{id}/profiles/running-candidates";
const CONNECTION_RUNNING_PROFILE_SNAPSHOT_PATH: &str =
    "/api/v1/connections/{id}/profiles/running-snapshot";
const CONNECTION_ADVISOR_PATH: &str = "/api/v1/connections/{id}/advisor-guides";
const CONNECTION_EXPLAIN_PATH: &str = "/api/v1/connections/{id}/explain";
const CONNECTION_RUNTIME_PROFILE_PATH: &str = "/api/v1/connections/{id}/runtime-profile";
const CAPABILITIES_PATH: &str = "/api/v1/capabilities";
const HEALTH_PATH: &str = "/api/v1/health";
/// Access-token login (network deployments): GET status, POST log in, DELETE log out.
const AUTH_SESSION_PATH: &str = "/api/v1/auth/session";
const CSRF_HEADER: &str = "x-doctor-csrf";
const MAX_HTTP_BODY_BYTES: usize = 34 * 1024 * 1024;
const SESSION_EXPIRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentMode {
    /// Native local process. Its listener and all peers remain loopback-only.
    Local,
    /// Container listener published only to the host loopback by Docker.
    ///
    /// `public_port` is the host-side published port. It is deliberately
    /// independent from the fixed container listener port (8000).
    DockerLoopbackPublished { public_port: u16 },
    /// A server reached over the network (e.g. a Linux host or container):
    /// any listener and any peer, but every API call needs the access-token
    /// session, the browser must be same-origin, and writes need the CSRF nonce.
    Network,
}

pub use access::{AccessConfig, MIN_TOKEN_CHARS};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiConfig {
    pub bind_addr: SocketAddr,
    pub deployment_mode: DeploymentMode,
    /// Explicit development-proxy hosts. Wildcards are never interpreted.
    pub allowed_dev_hosts: BTreeSet<String>,
    /// Explicit Vite development origins. Wildcards are never interpreted.
    pub allowed_dev_origins: BTreeSet<String>,
    pub budgets: ResourceBudgets,
    /// Optional same-origin Vite build directory for the local Candidate package.
    pub frontend_dist: Option<PathBuf>,
    /// Compile-time release identity. Runtime environment variables never override it.
    pub build: BuildCapability,
    /// Stage B history store and collection; disabled when its path is unset.
    pub history: HistoryConfig,
    /// Access token required by network deployments (optional elsewhere).
    pub access: Option<AccessConfig>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind_addr: DEFAULT_BIND_ADDR,
            deployment_mode: DeploymentMode::Local,
            allowed_dev_hosts: BTreeSet::new(),
            allowed_dev_origins: BTreeSet::new(),
            budgets: ResourceBudgets::default(),
            frontend_dist: None,
            history: HistoryConfig::default(),
            access: None,
            build: embedded_build_capability(),
        }
    }
}

fn embedded_build_capability() -> BuildCapability {
    BuildCapability {
        git_commit: option_env!("DOCTOR_BUILD_GIT_COMMIT")
            .unwrap_or("development")
            .to_owned(),
        candidate_label: option_env!("DOCTOR_CANDIDATE_LABEL")
            .unwrap_or("development")
            .to_owned(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceBudgets {
    pub single_input_bytes: usize,
    pub comparison_body_bytes: usize,
    pub text_lines: usize,
    pub text_line_bytes: usize,
    pub json_depth: usize,
    pub graph_operator_nodes: usize,
    pub normalized_metrics: usize,
    pub retained_diagnostics: usize,
    pub analysis_wall: Duration,
    pub comparison_wall: Duration,
    pub concurrent_requests: usize,
}

impl Default for ResourceBudgets {
    fn default() -> Self {
        Self {
            single_input_bytes: 16 * 1024 * 1024,
            comparison_body_bytes: 34 * 1024 * 1024,
            text_lines: 250_000,
            text_line_bytes: 256 * 1024,
            json_depth: 128,
            graph_operator_nodes: 20_000,
            normalized_metrics: 250_000,
            retained_diagnostics: 10_000,
            analysis_wall: Duration::from_secs(10),
            comparison_wall: Duration::from_secs(20),
            concurrent_requests: 2,
        }
    }
}

impl ResourceBudgets {
    fn capability_values(&self) -> [(&'static str, u128); 11] {
        [
            ("single_input_bytes", self.single_input_bytes as u128),
            ("comparison_body_bytes", self.comparison_body_bytes as u128),
            ("text_lines", self.text_lines as u128),
            ("text_line_bytes", self.text_line_bytes as u128),
            ("json_depth", self.json_depth as u128),
            ("graph_operator_nodes", self.graph_operator_nodes as u128),
            ("normalized_metrics", self.normalized_metrics as u128),
            ("retained_diagnostics", self.retained_diagnostics as u128),
            ("analysis_wall_ms", self.analysis_wall.as_millis()),
            ("comparison_wall_ms", self.comparison_wall.as_millis()),
            ("concurrent_requests", self.concurrent_requests as u128),
        ]
    }

    fn validate(&self) -> Result<(), ApiSetupError> {
        if self.concurrent_requests == 0 {
            return Err(ApiSetupError::ZeroConcurrentRequests);
        }
        self.capability_values()
            .into_iter()
            .find(|(_, value)| *value > u32::MAX as u128)
            .map_or(Ok(()), |(name, _)| {
                Err(ApiSetupError::CapabilityLimitOutOfRange(name))
            })
    }

    pub fn capability_limits(&self) -> BTreeMap<String, u32> {
        self.capability_values()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value as u32))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkKind {
    Analysis,
    Comparison,
    Export,
    Runtime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetViolation {
    InputTooLarge,
    TooManyLines,
    LineTooLong,
    TooDeep,
    TooManyNodes,
    TooManyMetrics,
    AnalysisTimeout,
    AnalysisBusy,
}

impl BudgetViolation {
    fn failure(self, request_id: String) -> ApiFailure {
        let (code, status, recoverability) = match self {
            Self::InputTooLarge => (
                "input_too_large",
                StatusCode::PAYLOAD_TOO_LARGE,
                "reduce_input",
            ),
            Self::TooManyLines => (
                "too_many_lines",
                StatusCode::PAYLOAD_TOO_LARGE,
                "reduce_input",
            ),
            Self::LineTooLong => (
                "line_too_long",
                StatusCode::PAYLOAD_TOO_LARGE,
                "reduce_input",
            ),
            Self::TooDeep => ("too_deep", StatusCode::PAYLOAD_TOO_LARGE, "reduce_input"),
            Self::TooManyNodes => (
                "too_many_nodes",
                StatusCode::PAYLOAD_TOO_LARGE,
                "reduce_input",
            ),
            Self::TooManyMetrics => (
                "too_many_metrics",
                StatusCode::PAYLOAD_TOO_LARGE,
                "reduce_input",
            ),
            Self::AnalysisTimeout => (
                "analysis_timeout",
                StatusCode::REQUEST_TIMEOUT,
                "reduce_input",
            ),
            Self::AnalysisBusy => ("analysis_busy", StatusCode::TOO_MANY_REQUESTS, "retry_same"),
        };
        let mut failure = ApiFailure::new(request_id, status, code, "limits", code, recoverability);
        // Busy and timeout are the two outcomes of the shared work slots; the
        // reason tells a client which budget refused it (limits: capabilities).
        let reason = match self {
            Self::AnalysisTimeout => Some("wall_clock_budget_exceeded"),
            Self::AnalysisBusy => Some("concurrent_request_limit_reached"),
            _ => None,
        };
        if let Some(reason) = reason {
            failure
                .envelope
                .details
                .insert("reason".to_owned(), reason.to_owned());
        }
        failure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticRetention {
    pub retained: usize,
    pub truncated: bool,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

#[derive(Debug)]
pub struct MonotonicClock(Instant);

impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

pub trait Cancellation: Send + Sync {
    fn is_cancelled(&self) -> bool;

    /// The work has produced its final bytes; from here on it only frees what
    /// it built.
    fn work_complete(&self) {}
}

#[derive(Debug)]
pub struct NeverCancelled;

impl Cancellation for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct BudgetGuard {
    limits: ResourceBudgets,
    clock: Arc<dyn Clock>,
    active: Arc<AtomicUsize>,
}

impl BudgetGuard {
    pub fn new(limits: ResourceBudgets) -> Self {
        Self::with_clock(limits, Arc::new(MonotonicClock::default()))
    }

    pub fn with_clock(limits: ResourceBudgets, clock: Arc<dyn Clock>) -> Self {
        Self {
            limits,
            clock,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn limits(&self) -> &ResourceBudgets {
        &self.limits
    }

    pub fn check_input_bytes(&self, bytes: usize, work: WorkKind) -> Result<(), BudgetViolation> {
        let limit = match work {
            WorkKind::Analysis | WorkKind::Runtime => self.limits.single_input_bytes,
            WorkKind::Comparison | WorkKind::Export => self.limits.comparison_body_bytes,
        };
        (bytes <= limit)
            .then_some(())
            .ok_or(BudgetViolation::InputTooLarge)
    }

    pub fn check_text(&self, text: &str) -> Result<(), BudgetViolation> {
        let line_count =
            usize::from(!text.is_empty()) + text.bytes().filter(|byte| *byte == b'\n').count();
        if line_count > self.limits.text_lines {
            return Err(BudgetViolation::TooManyLines);
        }
        // Same rule as the adapter: the FE Topology line is exempt from the per-line limit.
        if text.lines().any(|line| {
            line.len() > self.limits.text_line_bytes && !is_profile_topology_line(line.as_bytes())
        }) {
            return Err(BudgetViolation::LineTooLong);
        }
        Ok(())
    }

    pub fn check_json_depth(&self, depth: usize) -> Result<(), BudgetViolation> {
        (depth <= self.limits.json_depth)
            .then_some(())
            .ok_or(BudgetViolation::TooDeep)
    }

    pub fn check_nodes(&self, nodes: usize) -> Result<(), BudgetViolation> {
        (nodes <= self.limits.graph_operator_nodes)
            .then_some(())
            .ok_or(BudgetViolation::TooManyNodes)
    }

    pub fn check_metrics(&self, metrics: usize) -> Result<(), BudgetViolation> {
        (metrics <= self.limits.normalized_metrics)
            .then_some(())
            .ok_or(BudgetViolation::TooManyMetrics)
    }

    pub fn retain_diagnostics(&self, diagnostics: usize) -> DiagnosticRetention {
        DiagnosticRetention {
            retained: diagnostics.min(self.limits.retained_diagnostics),
            truncated: diagnostics > self.limits.retained_diagnostics,
        }
    }

    pub fn begin(&self, work: WorkKind) -> Result<WorkLease, BudgetViolation> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.limits.concurrent_requests).then_some(active + 1)
            })
            .map_err(|_| BudgetViolation::AnalysisBusy)?;
        let timeout = match work {
            WorkKind::Analysis => self.limits.analysis_wall,
            WorkKind::Comparison | WorkKind::Export | WorkKind::Runtime => {
                self.limits.comparison_wall
            }
        };
        Ok(WorkLease {
            slot: Some(Arc::new(ActiveSlot(Arc::clone(&self.active)))),
            clock: Arc::clone(&self.clock),
            deadline: self.clock.now() + timeout,
        })
    }
}

/// One of the `concurrent_requests` slots; freed when the last lease share drops.
struct ActiveSlot(Arc<AtomicUsize>);

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct WorkLease {
    /// `None` only for the deadline view a blocking analysis works with; its
    /// slot share is held by `WorkerStop`.
    slot: Option<Arc<ActiveSlot>>,
    clock: Arc<dyn Clock>,
    deadline: Duration,
}

impl WorkLease {
    pub fn check(&self, cancellation: &dyn Cancellation) -> Result<(), BudgetViolation> {
        if cancellation.is_cancelled() || self.clock.now() > self.deadline {
            return Err(BudgetViolation::AnalysisTimeout);
        }
        Ok(())
    }

    /// Same deadline, no slot share: the blocking analysis checks this view
    /// and holds its slot share in `WorkerStop` instead.
    fn deadline_only(&self) -> Self {
        Self {
            slot: None,
            clock: Arc::clone(&self.clock),
            deadline: self.deadline,
        }
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_sub(self.clock.now())
    }
}

/// Set when the HTTP side stops waiting (deadline or dropped request); the
/// blocking analysis observes it at its next stage boundary.
#[derive(Default)]
struct StopFlag(AtomicBool);

impl Cancellation for StopFlag {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

struct StopOnDrop(Arc<StopFlag>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.0.store(true, Ordering::Release);
    }
}

/// What a blocking analysis polls: the stop flag or the lease deadline. It
/// holds the work's share of the slot and drops it the first time the work
/// observes a stop, or when the response bytes are complete. From then on the
/// work only returns and frees what it built; it computes and allocates
/// nothing more, so the slot does not wait for that deallocation (v0.6-08).
struct WorkerStop {
    flag: Arc<StopFlag>,
    clock: Arc<dyn Clock>,
    deadline: Duration,
    slot: Mutex<Option<Arc<ActiveSlot>>>,
}

impl WorkerStop {
    fn release_slot(&self) {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

impl Cancellation for WorkerStop {
    fn is_cancelled(&self) -> bool {
        let stopped = self.flag.is_cancelled() || self.clock.now() > self.deadline;
        if stopped {
            self.release_slot();
        }
        stopped
    }

    fn work_complete(&self) {
        self.release_slot();
    }
}

#[derive(Clone)]
pub struct ApiState {
    config: ApiConfig,
    nonce: Arc<str>,
    budgets: BudgetGuard,
    request_sequence: Arc<AtomicU64>,
    sessions: Arc<SessionManager>,
    history: Arc<history::HistoryRuntime>,
    access: Option<Arc<access::AccessGate>>,
}

impl ApiState {
    pub fn new(config: ApiConfig) -> Result<Self, ApiSetupError> {
        let mut entropy = [0_u8; 32];
        getrandom::fill(&mut entropy).map_err(ApiSetupError::Random)?;
        let mut nonce = String::with_capacity(entropy.len() * 2);
        for byte in entropy {
            write!(nonce, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self::with_seams(config, nonce, Arc::new(MonotonicClock::default()))
    }

    pub fn with_seams(
        config: ApiConfig,
        nonce: impl Into<String>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ApiSetupError> {
        Self::with_session_manager(
            config,
            nonce,
            clock,
            Arc::new(SessionManager::new(Arc::new(MysqlTextRemoteFactory))),
        )
    }

    /// Test seams may supply a deterministic remote factory without widening
    /// the product's MySQL transport surface.
    pub fn with_connector_factory(
        config: ApiConfig,
        nonce: impl Into<String>,
        clock: Arc<dyn Clock>,
        factory: Arc<dyn RemoteFactory>,
    ) -> Result<Self, ApiSetupError> {
        Self::with_session_manager(config, nonce, clock, Arc::new(SessionManager::new(factory)))
    }

    fn with_session_manager(
        config: ApiConfig,
        nonce: impl Into<String>,
        clock: Arc<dyn Clock>,
        sessions: Arc<SessionManager>,
    ) -> Result<Self, ApiSetupError> {
        match config.deployment_mode {
            DeploymentMode::Local if !config.bind_addr.ip().is_loopback() => {
                return Err(ApiSetupError::NonLoopbackBind(config.bind_addr));
            }
            DeploymentMode::DockerLoopbackPublished { public_port } => {
                if public_port == 0 {
                    return Err(ApiSetupError::InvalidDockerPublicPort);
                }
                if config.bind_addr != DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR {
                    return Err(ApiSetupError::InvalidDockerBind(config.bind_addr));
                }
            }
            _ => {}
        }
        // Reachable from the network means nothing answers without the token.
        if let Some(access) = &config.access {
            if access.token.chars().count() < access::MIN_TOKEN_CHARS {
                return Err(ApiSetupError::AccessTokenTooShort);
            }
        } else if config.deployment_mode == DeploymentMode::Network {
            return Err(ApiSetupError::AccessTokenRequired);
        }
        config.budgets.validate()?;
        let nonce = nonce.into();
        if nonce.is_empty() {
            return Err(ApiSetupError::EmptyNonce);
        }
        let budgets = BudgetGuard::with_clock(config.budgets.clone(), clock);
        let history = Arc::new(history::HistoryRuntime::open(config.history.clone()));
        let access = config
            .access
            .as_ref()
            .map(|access| Arc::new(access::AccessGate::new(access)));
        Ok(Self {
            config,
            nonce: Arc::<str>::from(nonce),
            budgets,
            request_sequence: Arc::new(AtomicU64::new(1)),
            sessions,
            history,
            access,
        })
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.config.bind_addr
    }
    pub fn budgets(&self) -> &BudgetGuard {
        &self.budgets
    }

    fn next_request_id(&self) -> String {
        format!(
            "req-{:016x}",
            self.request_sequence.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn allowed_host(&self, host: &str) -> bool {
        let port = match self.config.deployment_mode {
            DeploymentMode::Local => self.config.bind_addr.port(),
            DeploymentMode::DockerLoopbackPublished { public_port } => public_port,
            // Reached by whatever name or address the operator gives users; the
            // access token, not the Host header, is what keeps others out.
            DeploymentMode::Network => {
                return !host.is_empty() && host.bytes().all(|byte| byte.is_ascii_graphic());
            }
        };
        host == format!("127.0.0.1:{port}")
            || host == format!("localhost:{port}")
            || self.config.allowed_dev_hosts.contains(host)
    }

    fn allowed_origin(&self, origin: &str, host: &str) -> bool {
        origin == format!("http://{host}")
            // Behind a TLS-terminating proxy the browser origin is https.
            || (self.config.deployment_mode == DeploymentMode::Network
                && origin == format!("https://{host}"))
            || self.config.allowed_dev_origins.contains(origin)
    }

    fn explicit_dev_origin(&self, origin: &str) -> bool {
        self.config.allowed_dev_origins.contains(origin)
    }

    fn capabilities(&self) -> Capabilities {
        let components = v01_analysis_receipt().components;
        Capabilities {
            api_version: "v0.1".to_owned(),
            schema_version: SCHEMA_VERSION.to_owned(),
            supported_inputs: vec![SourceKind::JsonProfile, SourceKind::TextProfile],
            version_packs: components
                .iter()
                .filter(|component| component.id == "starrocks-3.5.14")
                .map(|component| VersionPackCapability {
                    pack_id: component.id.clone(),
                    product_pack_version: component.version.clone(),
                    upstream_commit: "23a56ec517db763ea9bc7ee50b19357acdee0303".into(),
                    manifest_hash: component.manifest_hash.clone(),
                })
                .collect(),
            rule_sets: components
                .iter()
                .filter(|component| component.id == "doctor-v0.1")
                .map(|component| format!("{}@{}", component.id, component.version))
                .collect(),
            limits: self.budgets.limits().capability_limits(),
            csrf: CsrfCapability {
                nonce: self.nonce.to_string(),
                required_when_origin_present: true,
            },
            build: self.config.build.clone(),
        }
    }

    fn authorize(
        &self,
        request: &Request,
        peer: Option<SocketAddr>,
    ) -> Result<Option<String>, ApiFailure> {
        let request_id = self.next_request_id();
        if !peer.is_some_and(|address| self.allowed_peer(address.ip())) {
            return Err(ApiFailure::new(
                request_id,
                StatusCode::FORBIDDEN,
                "non_loopback_peer",
                "security",
                "non_loopback_peer",
                "change_input",
            ));
        }
        let host = request
            .headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !self.allowed_host(host) {
            return Err(ApiFailure::new(
                request_id,
                StatusCode::FORBIDDEN,
                "host_not_allowed",
                "security",
                "host_not_allowed",
                "change_input",
            ));
        }
        let origin = request
            .headers()
            .get(ORIGIN)
            .map(|value| value.to_str().ok().map(str::to_owned));
        let origin = match origin {
            Some(Some(origin)) if self.allowed_origin(&origin, host) => Some(origin),
            Some(_) => {
                return Err(ApiFailure::new(
                    request_id,
                    StatusCode::FORBIDDEN,
                    "origin_not_allowed",
                    "security",
                    "origin_not_allowed",
                    "change_input",
                ));
            }
            None => None,
        };
        let path = request.uri().path();
        if let Some(access) = &self.access {
            // Static assets are public code; every API answer needs the session,
            // except the health probe and the login endpoint itself.
            let exempt = path == HEALTH_PATH || path == AUTH_SESSION_PATH;
            if path.starts_with("/api/")
                && !exempt
                && request.method() != Method::OPTIONS
                && !access.authenticated(request.headers())
            {
                return Err(ApiFailure::new(
                    request_id,
                    StatusCode::UNAUTHORIZED,
                    "auth_required",
                    "security",
                    "auth_required",
                    "change_input",
                ));
            }
            // Logging in cannot carry the nonce (it is read after login), so it
            // must at least come from this origin's own page.
            if path == AUTH_SESSION_PATH && is_write(request.method()) && origin.is_none() {
                return Err(ApiFailure::new(
                    request_id,
                    StatusCode::FORBIDDEN,
                    "origin_not_allowed",
                    "security",
                    "origin_not_allowed",
                    "change_input",
                ));
            }
        }
        if is_write(request.method()) {
            if (matches!(
                self.config.deployment_mode,
                DeploymentMode::DockerLoopbackPublished { .. } | DeploymentMode::Network
            ) || origin.is_some())
                && path != AUTH_SESSION_PATH
                && request
                    .headers()
                    .get(CSRF_HEADER)
                    .is_none_or(|value| value.as_bytes() != self.nonce.as_bytes())
            {
                return Err(ApiFailure::new(
                    request_id,
                    StatusCode::FORBIDDEN,
                    "csrf_invalid",
                    "security",
                    "csrf_invalid",
                    "change_input",
                ));
            }
            if !allowed_content_type(request.headers()) {
                return Err(ApiFailure::new(
                    request_id,
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_media_type",
                    "request",
                    "unsupported_media_type",
                    "change_input",
                ));
            }
            if let Some(bytes) = content_length(request.headers()) {
                // The outer JSON envelope may contain one 16 MiB profile plus its JSON escaping.
                self.budgets
                    .check_input_bytes(bytes, WorkKind::Comparison)
                    .map_err(|violation| violation.failure(request_id.clone()))?;
            }
        }
        Ok(origin)
    }

    fn allowed_peer(&self, peer: IpAddr) -> bool {
        match self.config.deployment_mode {
            DeploymentMode::Local => peer.is_loopback(),
            DeploymentMode::DockerLoopbackPublished { .. } => docker_peer(peer),
            DeploymentMode::Network => true,
        }
    }
}

#[derive(Debug)]
pub enum ApiSetupError {
    NonLoopbackBind(SocketAddr),
    InvalidDockerBind(SocketAddr),
    InvalidDockerPublicPort,
    AccessTokenRequired,
    AccessTokenTooShort,
    EmptyNonce,
    ZeroConcurrentRequests,
    CapabilityLimitOutOfRange(&'static str),
    Random(getrandom::Error),
}

impl fmt::Display for ApiSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonLoopbackBind(address) => {
                write!(formatter, "bind address must be loopback: {address}")
            }
            Self::InvalidDockerBind(address) => write!(
                formatter,
                "docker loopback-published mode requires bind address {DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR}: {address}"
            ),
            Self::InvalidDockerPublicPort => {
                formatter.write_str("docker loopback-published public port must be positive")
            }
            Self::AccessTokenRequired => formatter.write_str(
                "network mode requires an access token (DOCTOR_ACCESS_TOKEN_FILE or DOCTOR_ACCESS_TOKEN)",
            ),
            Self::AccessTokenTooShort => write!(
                formatter,
                "the access token must have at least {} characters",
                access::MIN_TOKEN_CHARS
            ),
            Self::EmptyNonce => formatter.write_str("CSRF nonce must not be empty"),
            Self::ZeroConcurrentRequests => {
                formatter.write_str("concurrent request budget must be positive")
            }
            Self::CapabilityLimitOutOfRange(name) => {
                write!(formatter, "capability limit exceeds u32: {name}")
            }
            Self::Random(error) => write!(formatter, "cannot initialize local nonce: {error}"),
        }
    }
}

impl std::error::Error for ApiSetupError {}

#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    envelope: ErrorEnvelope,
}

impl ApiFailure {
    fn new(
        request_id: String,
        status: StatusCode,
        code: &str,
        stage: &str,
        message_id: &str,
        recoverability: &str,
    ) -> Self {
        Self {
            status,
            envelope: ErrorEnvelope {
                request_id,
                code: code.to_owned(),
                stage: stage.to_owned(),
                message_id: message_id.to_owned(),
                recoverability: recoverability.to_owned(),
                affected_side: None,
                source_refs: Vec::new(),
                details: BTreeMap::new(),
            },
        }
    }

    #[cfg(test)]
    fn code(&self) -> &str {
        &self.envelope.code
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(self.envelope)).into_response()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalysisRequest {
    source_kind: RequestedSourceKind,
    content: String,
    filename: Option<String>,
    version_hint: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComparisonRequest {
    baseline: AnalysisRequest,
    candidate: AnalysisRequest,
}

#[derive(Serialize)]
struct ResponseMeta {
    request_id: String,
}

#[derive(Serialize)]
struct AnalysisResponse {
    meta: ResponseMeta,
    outcome: AnalysisOutcome,
}

#[derive(Serialize)]
struct ComparisonResponse {
    meta: ResponseMeta,
    comparison: doctor_contracts::Comparison,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(
            AUTH_SESSION_PATH,
            get(auth_status).post(auth_login).delete(auth_logout),
        )
        .route(CAPABILITIES_PATH, get(capabilities))
        .route(ANALYSES_PATH, options(preflight).post(analyses))
        .route(COMPARISONS_PATH, options(preflight).post(comparisons))
        .route(EXPORTS_PATH, options(preflight).post(exports))
        .route(CONNECTIONS_PATH, options(preflight).post(create_connection))
        .route(
            CONNECTION_ID_PATH,
            options(delete_preflight).delete(close_connection),
        )
        .route(
            CONNECTION_SEARCH_PATH,
            options(preflight).post(search_connection_queries),
        )
        .route(
            CONNECTION_QUERY_SQL_PATH,
            options(preflight).post(fetch_connection_query_sql),
        )
        .route(
            "/api/v1/connections/{id}/profiles/official-analysis",
            options(preflight).post(read_official_profile),
        )
        .route(
            "/api/v1/connections/{id}/audit/occurrences",
            options(preflight).post(audit::occurrences),
        )
        .route(
            "/api/v1/connections/{id}/audit/families",
            options(preflight).post(audit::families),
        )
        .route(
            "/api/v1/connections/{id}/audit/fingerprint-families",
            options(preflight).post(audit::fingerprint_families),
        )
        .route(
            "/api/v1/history/status",
            options(preflight).post(history::status),
        )
        .route(
            "/api/v1/history/collection",
            options(preflight).post(history::collection),
        )
        .route(
            "/api/v1/history/clear",
            options(preflight).post(history::clear),
        )
        .route(
            "/api/v1/history/trends",
            options(preflight).post(history_trend::trends),
        )
        .route(
            "/api/v1/history/series",
            options(preflight).post(history_trend::series),
        )
        .route(
            "/api/v1/history/alerts",
            options(preflight).post(governance::alerts),
        )
        .route(
            "/api/v1/history/alerts/action",
            options(preflight).post(governance::alert_action),
        )
        .route(
            "/api/v1/governance/list",
            options(preflight).post(governance::governance_list),
        )
        .route(
            "/api/v1/governance/create",
            options(preflight).post(governance::governance_create),
        )
        .route(
            "/api/v1/governance/transition",
            options(preflight).post(governance::governance_transition),
        )
        .route(
            "/api/v1/governance/pin",
            options(preflight).post(governance::governance_pin),
        )
        .route(
            "/api/v1/governance/change",
            options(preflight).post(governance::governance_change),
        )
        .route(
            "/api/v1/governance/verify",
            options(preflight).post(governance::governance_verify),
        )
        .route(
            "/api/v1/connections/{id}/audit/similar-sql",
            options(preflight).post(audit::similar_sql),
        )
        .route(
            "/api/v1/connections/{id}/audit/family-detail",
            options(preflight).post(audit::family_detail),
        )
        .route(
            "/api/v1/connections/{id}/audit/overview",
            options(preflight).post(audit::overview),
        )
        .route(
            "/api/v1/connections/{id}/audit/active-at",
            options(preflight).post(audit::active_at),
        )
        .route(
            "/api/v1/connections/{id}/audit/sql",
            options(preflight).post(audit::sql),
        )
        .route(
            CONNECTION_PROFILE_PATH,
            options(preflight).post(fetch_connection_profile),
        )
        .route(
            CONNECTION_RUNNING_PROFILE_CANDIDATES_PATH,
            options(preflight).post(list_running_profile_candidates),
        )
        .route(
            CONNECTION_RUNNING_PROFILE_SNAPSHOT_PATH,
            options(preflight).post(fetch_running_profile_snapshot),
        )
        .route(
            CONNECTION_PROFILE_COMPARE_PATH,
            options(preflight).post(compare_connection_profiles),
        )
        .route(CONNECTION_ADVISOR_PATH, get(connection_advisor_guides))
        .route(
            CONNECTION_EXPLAIN_PATH,
            options(preflight).post(explain_connection),
        )
        .route(
            CONNECTION_RUNTIME_PROFILE_PATH,
            options(preflight).post(runtime_profile_connection),
        )
        .fallback(static_or_not_ready)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            trust_boundary,
        ))
        .with_state(state)
}

pub async fn serve(state: ApiState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(state.bind_addr()).await?;
    let server = axum::serve(
        listener,
        router(state.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .into_future();
    tokio::pin!(server);
    let mut expiry = tokio::time::interval(SESSION_EXPIRY_INTERVAL);
    loop {
        tokio::select! {
            result = &mut server => return result,
            _ = expiry.tick() => expire_idle_once(Arc::clone(&state.sessions)).await,
        }
    }
}

async fn expire_idle_once(sessions: Arc<SessionManager>) {
    let _ = tokio::task::spawn_blocking(move || sessions.expire_idle()).await;
}

async fn capabilities(State(state): State<ApiState>) -> Response {
    let mut response = Json(state.capabilities()).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn health() -> Json<BTreeMap<&'static str, &'static str>> {
    Json(BTreeMap::from([("status", "ok")]))
}

/// Whether this server needs the access token, and whether this browser has a session.
async fn auth_status(State(state): State<ApiState>, request: Request) -> Response {
    let status = AccessSessionStatus {
        required: state.access.is_some(),
        authenticated: state
            .access
            .as_ref()
            .is_none_or(|access| access.authenticated(request.headers())),
    };
    no_store(Json(status).into_response())
}

async fn auth_login(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let Some(access) = state.access.clone() else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let login: AccessLoginRequest = connector_json_request(&state, request).await?;
    match access.login(&login.token) {
        Ok(cookie) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                SET_COOKIE,
                HeaderValue::from_str(&cookie).map_err(|_| internal_failure(&state))?,
            );
            Ok(no_store(response))
        }
        Err(error) => Err(ApiFailure::new(
            state.next_request_id(),
            if error == access::LoginError::Throttled {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::UNAUTHORIZED
            },
            if error == access::LoginError::Throttled {
                "auth_throttled"
            } else {
                "auth_invalid"
            },
            "security",
            if error == access::LoginError::Throttled {
                "auth_throttled"
            } else {
                "auth_invalid"
            },
            if error == access::LoginError::Throttled {
                "retry_later"
            } else {
                "change_input"
            },
        )),
    }
}

async fn auth_logout(State(state): State<ApiState>, request: Request) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(access) = &state.access
        && let Ok(cookie) = HeaderValue::from_str(&access.logout(request.headers()))
    {
        response.headers_mut().insert(SET_COOKIE, cookie);
    }
    no_store(response)
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn preflight(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    preflight_for(state, request, "POST")
}

async fn delete_preflight(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    preflight_for(state, request, "DELETE")
}

fn preflight_for(
    state: ApiState,
    request: Request,
    expected_method: &'static str,
) -> Result<Response, ApiFailure> {
    if request
        .headers()
        .get(ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|value| value.to_str().ok())
        != Some(expected_method)
    {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request",
            "preflight_method_invalid",
            "change_input",
        ));
    }
    let requested_headers = request
        .headers()
        .get(ACCESS_CONTROL_REQUEST_HEADERS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let requested = requested_headers
        .split(',')
        .map(|header| header.trim().to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let allowed = BTreeSet::from(["content-type".to_owned(), CSRF_HEADER.to_owned()]);
    if requested != allowed {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request",
            "preflight_headers_invalid",
            "change_input",
        ));
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(expected_method),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, X-Doctor-CSRF"),
    );
    Ok(response)
}

async fn not_ready(State(state): State<ApiState>) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::NOT_IMPLEMENTED,
        "not_ready",
        "request",
        "endpoint_not_ready",
        "restart",
    )
}

async fn analyses(State(state): State<ApiState>, request: Request) -> Result<Response, ApiFailure> {
    let lease = state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request = analysis_request_from_http(&state, request, &lease).await?;
    blocking_analysis(&state, &lease, move |state, lease, cancellation| {
        let report = analyze_request(state, request, lease, cancellation)?;
        let body = AnalysisResponse {
            meta: ResponseMeta {
                request_id: state.next_request_id(),
            },
            outcome: AnalysisOutcome { report },
        };
        json_response(state, lease, cancellation, body)
    })
    .await
}

async fn comparisons(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let lease = state
        .budgets
        .begin(WorkKind::Comparison)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request = comparison_request_from_http(&state, request, &lease).await?;
    blocking_analysis(&state, &lease, move |state, lease, cancellation| {
        let baseline =
            comparison_attempt(state, request.baseline, "baseline", lease, cancellation)?;
        let candidate =
            comparison_attempt(state, request.candidate, "candidate", lease, cancellation)?;
        let body = ComparisonResponse {
            meta: ResponseMeta {
                request_id: state.next_request_id(),
            },
            comparison: compare_attempts(baseline, candidate),
        };
        json_response(state, lease, cancellation, body)
    })
    .await
}

async fn exports(State(state): State<ApiState>, request: Request) -> Result<Response, ApiFailure> {
    let lease = state
        .budgets
        .begin(WorkKind::Export)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    if !is_json_request(request.headers()) {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "request",
            "export_requires_json",
            "change_input",
        ));
    }
    let body = Bytes::from_request(request, &state)
        .await
        .map_err(|_| input_too_large(&state))?;
    state
        .budgets
        .check_input_bytes(body.len(), WorkKind::Export)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request: ExportRequest = parse_json_request(&state, &body)?;
    let artifact = doctor_export::assemble(request).map_err(|_| {
        ApiFailure::new(
            state.next_request_id(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "export_invalid_snapshot",
            "export",
            "export_invalid_snapshot",
            "reanalyze_input",
        )
    })?;
    let bytes = doctor_export::artifact_bytes(&artifact).map_err(|_| {
        ApiFailure::new(
            state.next_request_id(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "export_failed",
            "export",
            "export_failed",
            "restart",
        )
    })?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"sr-doctor-report.json\""),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn create_connection(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Json<PublicConnectionSessionView>, ApiFailure> {
    let request: ConnectionCreateRequest = connector_json_request(&state, request).await?;
    let (host, port) = (request.host.clone(), request.port);
    // Remote means "not this machine", whatever the transport: a plaintext
    // connection to another host stays behind the G1 collection gate.
    let remote = !doctor_connector::is_literal_loopback(&host);
    let session = connector_call(&state, move |sessions| {
        sessions.connect(connector_connection_request(request))
    })
    .await?;
    history::start_collector(&state, &session.connection_id, &host, port, remote);
    Ok(Json(public_connection_view(session)))
}

async fn close_connection(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
) -> Result<StatusCode, ApiFailure> {
    history::stop_collector(&state, &connection_id);
    let sessions = Arc::clone(&state.sessions);
    tokio::task::spawn_blocking(move || sessions.close(&connection_id))
        .await
        .map_err(|_| internal_failure(&state))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn search_connection_queries(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Json<ConnectedQueryListResponse>, ApiFailure> {
    let request: ConnectedQueryListRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let end_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| internal_failure(&state))?
        .as_secs();
    let result = connector_call(&state, move |sessions| {
        sessions.search(
            &connection_id,
            QuerySearch {
                start_seconds: end_seconds.saturating_sub(30 * 24 * 60 * 60),
                end_seconds,
                state: request.status,
                digest: request.digest,
                limit: request.limit,
                source: QuerySource::Auto,
                statement_filter: request.statement_filter,
            },
        )
    })
    .await?;
    Ok(Json(public_query_result(
        &state,
        &request.connection_id,
        result,
    )?))
}

async fn fetch_connection_query_sql(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Json<ConnectedQuerySqlResponse>, ApiFailure> {
    let request: ConnectedQuerySqlRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let query_id = request.query_id;
    let discovery_generation = request.discovery_generation;
    let query = connector_call(&state, move |sessions| {
        sessions.query_sql(&connection_id, &query_id, discovery_generation)
    })
    .await?;
    Ok(Json(ConnectedQuerySqlResponse {
        query_id: query.query_id,
        discovery_generation: query.discovery_generation,
        sql: query.sql,
    }))
}

async fn read_official_profile(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Json<doctor_contracts::OfficialProfileReadResponse>, ApiFailure> {
    use doctor_connector::{
        AnalyzeProfileRequest, OfficialProfileContent, OfficialProfileUnavailableReason,
    };
    use doctor_contracts::{
        OfficialProfileArtifactKind, OfficialProfileDisplayNormalization,
        OfficialProfileReadRequest, OfficialProfileReadResponse, official_profile_display_hash,
        official_profile_display_text, official_profile_text_hash,
    };
    let lease = state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request: OfficialProfileReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let reading = connector_call(&state, move |sessions| {
        sessions.analyze_profile(
            &connection_id,
            AnalyzeProfileRequest {
                query_id: request.query_id,
                identity: request.identity,
                node_ids: request.node_ids,
            },
        )
    })
    .await?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let rows = match reading.content {
        OfficialProfileContent::Available { rows } => rows,
        OfficialProfileContent::Unavailable { reason } => {
            let reason = match reason {
                OfficialProfileUnavailableReason::LifecycleUnknown => {
                    "official_profile_lifecycle_unknown"
                }
                OfficialProfileUnavailableReason::Empty => "official_profile_empty",
                OfficialProfileUnavailableReason::PermissionDenied => {
                    "official_profile_permission_denied"
                }
                OfficialProfileUnavailableReason::RemoteUnsupported => {
                    "official_profile_remote_unsupported"
                }
                OfficialProfileUnavailableReason::NotRetained => "official_profile_not_retained",
            };
            return Err(ApiFailure::new(
                state.next_request_id(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "official_profile_unavailable",
                "analyze_profile",
                reason,
                "change_input",
            ));
        }
    };
    let query = match reading.query_context.as_ref() {
        Some(context) => public_observed_query_ref(&state, &request.connection_id, context)?,
        None => ConnectedQueryRef {
            connection_id: request.connection_id,
            query_id: reading.query_id.clone(),
            discovery_source: None,
            discovery_generation: None,
            observed_at_utc: None,
            observed_at_local: None,
            timestamp_state: ConnectedTimestampState::Unavailable,
            status: "finished".to_owned(),
            digest: None,
            database: None,
            resource_group: None,
            query_time_ms: None,
            profile_fetch: ConnectedProfileFetchAction::Available {
                reason: ProfileFetchAvailableReason::ProfileFetched,
            },
            plan_check: ConnectedPlanCheckAction::Unavailable {
                reason: PlanCheckUnavailableReason::StatementMissingOrNotSelect,
            },
            source_refs: vec!["runtime:last_query_id".to_owned()],
        },
    };
    let text = rows.join("\n");
    let display_text = official_profile_display_text(&text);
    let read_at_utc = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| internal_failure(&state))?;
    Ok(Json(OfficialProfileReadResponse {
        kind: OfficialProfileArtifactKind::AnalyzeProfileText,
        query,
        identity: reading.identity,
        detected_version: Some(VersionEvidence {
            value: reading.detected_version,
            source_ref: "connection:current_version".into(),
        }),
        node_ids: reading.node_ids,
        read_at_utc,
        profile_observed_at_utc: None,
        source_hash: official_profile_text_hash(&text),
        text,
        display_hash: official_profile_display_hash(&display_text),
        display_text,
        display_normalization: OfficialProfileDisplayNormalization::AnsiSgrStripControlsEscapeV1,
        source_refs: vec!["analyze_profile:official_text".into()],
        limitations: if reading.query_context.is_some() {
            vec![
                "official_interpretation_not_raw_profile".into(),
                "profile_observation_time_unknown".into(),
                "query_status_from_discovery_not_fresh_probe".into(),
                "result_rows_joined_with_newline".into(),
            ]
        } else {
            vec![
                "official_interpretation_not_raw_profile".into(),
                "profile_observation_time_unknown".into(),
                "query_status_from_verified_standard_execution".into(),
                "result_rows_joined_with_newline".into(),
            ]
        },
    }))
}

async fn list_running_profile_candidates(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Json<RunningProfileCandidateListResponse>, ApiFailure> {
    let request: RunningProfileCandidateListRequest =
        connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let result = connector_call(&state, move |sessions| {
        sessions.list_running_profile_candidates(
            &connection_id,
            request.limit,
            request.statement_filter,
        )
    })
    .await?;
    let read_at_utc = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| internal_failure(&state))?;
    Ok(Json(public_running_profile_candidates(
        &state,
        &request.connection_id,
        result,
        read_at_utc,
    )?))
}

async fn fetch_running_profile_snapshot(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let lease = state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request: RunningProfileSnapshotRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let query_id = request.query_id.clone();
    let discovery_generation = request.discovery_generation;
    let profile = connector_call(&state, move |sessions| {
        sessions.fetch_running_profile_snapshot(&connection_id, &query_id, discovery_generation)
    })
    .await?;
    let read_at_utc = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| internal_failure(&state))?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let query = public_observed_query_ref(&state, &request.connection_id, &profile.query_context)?;
    if !query.status.eq_ignore_ascii_case("running") {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_GATEWAY,
            "connector_response_invalid",
            "query_source",
            "running_snapshot_query_context_invalid",
            "retry_same",
        ));
    }
    let list_observed_state = query.status.clone();
    blocking_analysis(&state, &lease, move |state, lease, cancellation| {
        let report = analyze_input(state, profile.into_adapter_input(), lease, cancellation)?;
        let source_kind = report.input.detected_kind.clone();
        let profile_lifecycle = report.profile_context.query_state.clone();
        let body = RunningProfileSnapshotResponse {
            query,
            list_observed_state,
            profile_lifecycle,
            read_at_utc,
            profile_observed_at_utc: None,
            input_origin: InputOrigin::ConnectedProfileFetch,
            source_kind,
            source_hash: report.input.source_hash.clone(),
            analysis_key: report.analysis_key.clone(),
            analysis_receipt: report.analysis_receipt.clone(),
            outcome: AnalysisOutcome { report },
            source_refs: vec![
                "profile:query_id".to_owned(),
                "profile_list:running_snapshot".to_owned(),
            ],
            limitations: vec![
                "profile_raw_text_omitted_by_contract".to_owned(),
                "profile_observation_time_unknown".to_owned(),
                "query_status_from_discovery_not_fresh_probe".to_owned(),
                "list_observed_state_and_profile_lifecycle_are_independent".to_owned(),
            ],
        };
        json_response(state, lease, cancellation, body)
    })
    .await
}

async fn fetch_connection_profile(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: ProfileFetchRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let lease = state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let query_id = request.query_id.clone();
    let discovery_generation = request.discovery_generation;
    let profile = connector_call(&state, move |sessions| {
        sessions.fetch_profile(&connection_id, &query_id, discovery_generation)
    })
    .await?;
    let query_context = profile.query_context.clone();
    blocking_analysis(&state, &lease, move |state, lease, cancellation| {
        let report = analyze_input(state, profile.into_adapter_input(), lease, cancellation)?;
        let source_kind = report.input.detected_kind.clone();
        let query = public_fetched_query_ref(state, &request.connection_id, &query_context)?;
        let input_origin = match request.kind {
            ProfileFetchKind::Baseline => InputOrigin::ConnectedProfileFetch,
            ProfileFetchKind::CandidateUserExternalRerun => {
                InputOrigin::CandidateProfileUserExternalRerun
            }
        };
        let body = ProfileFetchResponse {
            query,
            input_origin,
            source_kind,
            state: ProfileFetchState::Fetched,
            official_profile_eligibility: None,
            source_hash: report.input.source_hash.clone(),
            analysis_key: report.analysis_key.clone(),
            analysis_receipt: report.analysis_receipt.clone(),
            outcome: AnalysisOutcome { report },
            source_refs: vec!["profile:query_id".to_owned()],
            limitations: vec![
                "raw_connector_profile_bytes_not_returned_by_connected_fetch_response".to_owned(),
                "profile_cache_availability_is_observed_at_fetch_time".to_owned(),
            ],
        };
        json_response(state, lease, cancellation, body)
    })
    .await
}

/// Compares two runtime Profiles fetched through the same opaque connection
/// session. Candidate execution remains outside this product: this endpoint
/// reads an already-created query profile and never invokes `EXPLAIN` or SQL.
async fn compare_connection_profiles(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let lease = state
        .budgets
        .begin(WorkKind::Comparison)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request: ConnectedProfileComparisonRequest =
        connector_json_request_for(&state, request, WorkKind::Comparison).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    require_distinct_valid_profile_query_ids(
        &state,
        &request.baseline_query_id,
        &request.candidate_query_id,
    )?;

    let baseline_query_id = request.baseline_query_id.clone();
    let candidate_query_id = request.candidate_query_id.clone();
    let discovery_generation = request.discovery_generation;
    let (baseline_profile, candidate_profile) = connector_call(&state, move |sessions| {
        sessions.fetch_profile_pair(
            &connection_id,
            &baseline_query_id,
            &candidate_query_id,
            discovery_generation,
        )
    })
    .await?;

    let baseline_query_context = baseline_profile.query_context.clone();
    let candidate_query_context = candidate_profile.query_context.clone();
    let baseline_input = baseline_profile.into_adapter_input();
    let candidate_input = candidate_profile.into_adapter_input();
    let combined_input_bytes = baseline_input
        .content
        .len()
        .checked_add(candidate_input.content.len())
        .ok_or_else(|| input_too_large(&state))?;
    state
        .budgets
        .check_input_bytes(combined_input_bytes, WorkKind::Comparison)
        .map_err(|violation| violation.failure(state.next_request_id()))?;

    blocking_analysis(&state, &lease, move |state, lease, cancellation| {
        let baseline = analysis_attempt(
            analyze_input(state, baseline_input, lease, cancellation)
                .map_err(|failure| with_affected_side(failure, "baseline"))?,
        );
        let candidate = analysis_attempt(
            analyze_input(state, candidate_input, lease, cancellation)
                .map_err(|failure| with_affected_side(failure, "candidate"))?,
        );
        let body = ConnectedProfileComparisonResponse {
            baseline: ConnectedProfileComparisonSide {
                query: public_fetched_query_ref(
                    state,
                    &request.connection_id,
                    &baseline_query_context,
                )?,
                input_origin: InputOrigin::ConnectedProfileFetch,
            },
            candidate: ConnectedProfileComparisonSide {
                query: public_fetched_query_ref(
                    state,
                    &request.connection_id,
                    &candidate_query_context,
                )?,
                input_origin: InputOrigin::CandidateProfileUserExternalRerun,
            },
            comparison: compare_attempts(baseline, candidate),
            validation_context: ConnectedComparisonValidationContext {
                interpretation_state: ConnectedComparisonInterpretationState::ObservedOnly,
                same_connection_session: true,
                same_discovery_generation: true,
                fetch_order: "baseline_then_candidate".to_owned(),
                candidate_attestation: "user_external_controlled_rerun".to_owned(),
            },
            limitations: vec![
                "candidate_profile_user_external_rerun_is_user_responsibility".to_owned(),
                "raw_connector_profile_bytes_not_returned_by_connected_profile_comparison"
                    .to_owned(),
                "observed_changes_do_not_prove_causal_improvement".to_owned(),
            ],
        };
        json_response(state, lease, cancellation, body)
    })
    .await
}

async fn connection_advisor_guides(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
) -> Result<Json<PlanAdvisorResponse>, ApiFailure> {
    let advisor = connector_call(&state, move |sessions| sessions.advisor(&connection_id)).await?;
    Ok(Json(PlanAdvisorResponse {
        capability: PublicCapabilityState::Available,
        recommendations: Vec::new(),
        limitations: vec![
            "advisor_rows_not_normalized_in_v0_1".to_owned(),
            format!("advisor_row_count:{}", advisor.rows.len()),
        ],
    }))
}

async fn explain_connection(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Json<ExplainResponse>, ApiFailure> {
    // Ordinary EXPLAIN can be an expensive remote read.  It shares the analysis
    // lease rather than queueing outside the established 10-second / two-slot
    // product budget.
    let lease = state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request: ExplainRequest = connector_json_request(&state, request).await?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let (explain_input, mode) = match request {
        ExplainRequest::UserSql {
            connection_id: body_connection_id,
            candidate_sql,
            mode,
        } => {
            require_connection_id(&state, &connection_id, &body_connection_id)?;
            if !ordinary_select(&candidate_sql) {
                return Err(ApiFailure::new(
                    state.next_request_id(),
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "statement_not_allowed",
                    "security",
                    "explain_user_sql_must_be_one_plain_select_or_with",
                    "change_input",
                ));
            }
            (ExplainInput::CandidateSql(candidate_sql), mode)
        }
        ExplainRequest::PlanSqlReference {
            connection_id: body_connection_id,
            query_id,
            discovery_generation,
            plan_sql_ref,
            mode,
        } => {
            require_connection_id(&state, &connection_id, &body_connection_id)?;
            (
                ExplainInput::PlanSqlRef {
                    query_id,
                    discovery_generation,
                    plan_sql_ref,
                },
                mode,
            )
        }
    };
    if matches!(
        &explain_input,
        ExplainInput::PlanSqlRef { query_id, .. } if !is_valid_query_id(query_id)
    ) {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request",
            "profile_query_id_invalid",
            "change_input",
        ));
    }
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let explain = connector_call(&state, move |sessions| match explain_input {
        ExplainInput::CandidateSql(candidate_sql) => {
            sessions.explain(&connection_id, &candidate_sql, mode)
        }
        ExplainInput::PlanSqlRef {
            query_id,
            discovery_generation,
            plan_sql_ref,
        } => sessions.explain_reference(
            &connection_id,
            &query_id,
            discovery_generation,
            &plan_sql_ref,
            mode,
        ),
    })
    .await?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let snapshot = assemble_explain_snapshot_for_mode(
        ExplainSnapshotInput {
            rows: explain.rows,
            detected_version: explain.detected_version,
            default_database: explain.default_database,
        },
        explain.mode,
    );
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    Ok(Json(ExplainResponse { snapshot }))
}

async fn runtime_profile_connection(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    // Runtime mode is explicit and uses the longer comparison wall budget;
    // the transport separately fixes query_timeout=10 on the remote session.
    let lease = state
        .budgets
        .begin(WorkKind::Runtime)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let request: RuntimeProfileRequest =
        connector_json_request_for(&state, request, WorkKind::Runtime).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let body_connection_id = request.connection_id;
    let candidate_sql = request.candidate_sql;
    let location_sql = candidate_sql.clone();
    let execution_attempt_id = request.execution_attempt_id;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let profile = connector_call(&state, move |sessions| {
        sessions.execute_profiled_select_with_attempt(
            &connection_id,
            &candidate_sql,
            execution_attempt_id.as_deref(),
        )
    })
    .await?;
    let official_profile_eligibility =
        profile
            .verified_standard_execution_attempt_id
            .clone()
            .map(
                |execution_attempt_id| doctor_contracts::OfficialProfileEligibility {
                    execution_attempt_id,
                    method: doctor_contracts::OfficialProfileEligibilityMethod::StandardProfile,
                },
            );
    let query_id = profile.query_id.clone();
    blocking_analysis(&state, &lease, move |state, lease, cancellation| {
        let mut report = analyze_input(state, profile.into_adapter_input(), lease, cancellation)?;
        doctor_core::attach_sql_locations(&mut report, &location_sql, &query_id);
        let body = ProfileFetchResponse {
            query: ConnectedQueryRef {
                connection_id: body_connection_id,
                query_id,
                discovery_source: None,
                discovery_generation: None,
                observed_at_utc: None,
                observed_at_local: None,
                timestamp_state: ConnectedTimestampState::Unavailable,
                status: "finished".to_owned(),
                digest: None,
                database: None,
                resource_group: None,
                query_time_ms: None,
                profile_fetch: ConnectedProfileFetchAction::Available {
                    reason: ProfileFetchAvailableReason::ProfileFetched,
                },
                plan_check: ConnectedPlanCheckAction::Unavailable {
                    reason: PlanCheckUnavailableReason::StatementMissingOrNotSelect,
                },
                source_refs: vec!["runtime:last_query_id".to_owned()],
            },
            input_origin: InputOrigin::ConnectedSqlRuntime,
            source_kind: report.input.detected_kind.clone(),
            state: ProfileFetchState::Fetched,
            official_profile_eligibility,
            source_hash: report.input.source_hash.clone(),
            analysis_key: report.analysis_key.clone(),
            analysis_receipt: report.analysis_receipt.clone(),
            outcome: AnalysisOutcome { report },
            source_refs: vec![
                "runtime:last_query_id".to_owned(),
                "runtime:get_query_profile".to_owned(),
            ],
            limitations: vec![
                "runtime_sql_only_bounded_location_excerpt_returned_not_persisted".to_owned(),
                "runtime_query_timeout_10_seconds".to_owned(),
                "runtime_result_rows_are_discarded".to_owned(),
            ],
        };
        json_response(state, lease, cancellation, body)
    })
    .await
}

enum ExplainInput {
    CandidateSql(String),
    PlanSqlRef {
        query_id: String,
        discovery_generation: u32,
        plan_sql_ref: String,
    },
}

async fn connector_json_request<T: for<'de> Deserialize<'de>>(
    state: &ApiState,
    request: Request,
) -> Result<T, ApiFailure> {
    connector_json_request_for(state, request, WorkKind::Analysis).await
}

async fn connector_json_request_for<T: for<'de> Deserialize<'de>>(
    state: &ApiState,
    request: Request,
    work_kind: WorkKind,
) -> Result<T, ApiFailure> {
    if !is_json_request(request.headers()) {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "request",
            "connector_requires_json",
            "change_input",
        ));
    }
    let body = Bytes::from_request(request, state)
        .await
        .map_err(|_| input_too_large(state))?;
    state
        .budgets
        .check_input_bytes(body.len(), work_kind)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    parse_json_request(state, &body)
}

/// Only Audit uses this control; ordinary/native execution retains its own contract.
async fn audit_connector_call<T: Send + 'static>(
    state: &ApiState,
    lease: WorkLease,
    connection_id: String,
    call: impl FnOnce(
        &SessionManager,
        &Arc<doctor_connector::OperationControl>,
        &ApiState,
    ) -> Result<T, ApiFailure>
    + Send
    + 'static,
) -> Result<T, ApiFailure> {
    let remaining = lease.deadline.saturating_sub(lease.clock.now());
    let clock = Arc::clone(&lease.clock);
    let deadline = lease.deadline;
    let control = doctor_connector::OperationControl::with_expiry(
        remaining,
        Arc::new(move || clock.now() >= deadline),
    );
    struct CancelOnDrop(Arc<doctor_connector::OperationControl>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    let _cancel = CancelOnDrop(Arc::clone(&control));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let timer_control = Arc::clone(&control);
    // This task is not owned by the handler. Worker completion (including panic)
    // drops the sender, so the timer always terminates without waiting its deadline.
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(remaining) => timer_control.stop(),
            _ = done_rx => {},
        }
    });
    let worker_state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _done = done_tx;
        let _lease = lease; // Permit remains held even when the HTTP future is dropped.
        struct FinishOnDrop<'a> {
            sessions: &'a SessionManager,
            connection_id: &'a str,
            control: &'a Arc<doctor_connector::OperationControl>,
            armed: bool,
        }
        impl Drop for FinishOnDrop<'_> {
            fn drop(&mut self) {
                if self.armed {
                    self.control.stop();
                    let _ = self
                        .sessions
                        .finish_audit_operation(self.connection_id, self.control);
                }
            }
        }
        let mut cleanup = FinishOnDrop {
            sessions: &worker_state.sessions,
            connection_id: &connection_id,
            control: &control,
            armed: true,
        };
        let result = call(&worker_state.sessions, &control, &worker_state);
        if _lease.check(&NeverCancelled).is_err() {
            control.stop();
        }
        let finished = worker_state
            .sessions
            .finish_audit_operation(&connection_id, &control);
        cleanup.armed = false;
        finished.map_err(|error| {
            if control.acquired_session() {
                connector_failure(&worker_state, error)
            } else {
                BudgetViolation::AnalysisTimeout.failure(worker_state.next_request_id())
            }
        })?;
        result
    })
    .await
    .map_err(|_| internal_failure(state))?
}

/// Runs CPU-bound analysis (adapt, resolve, rules, assemble, response
/// serialization) on the blocking pool instead of an async worker.
///
/// The HTTP side waits at most until the lease deadline and then answers
/// `analysis_timeout`; it never returns a partial report. When it stops
/// waiting (deadline or dropped request) it raises the stop flag, which the
/// work observes at its next poll: a stage boundary, or a poll inside adapt,
/// assemble and serialization (v0.6-08). The work holds a share of the slot
/// until it observes the stop, so an abandoned analysis keeps its slot while
/// it still computes and cannot push the number of running analyses past
/// `concurrent_requests`.
async fn blocking_analysis<T: Send + 'static>(
    state: &ApiState,
    lease: &WorkLease,
    work: impl FnOnce(&ApiState, &WorkLease, &dyn Cancellation) -> Result<T, ApiFailure>
    + Send
    + 'static,
) -> Result<T, ApiFailure> {
    let stop = Arc::new(StopFlag::default());
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let remaining = lease.remaining();
    let worker_state = state.clone();
    let worker_lease = lease.deadline_only();
    let worker_stop = WorkerStop {
        flag: stop,
        clock: Arc::clone(&lease.clock),
        deadline: lease.deadline,
        slot: Mutex::new(lease.slot.clone()),
    };
    let job = tokio::task::spawn_blocking(move || work(&worker_state, &worker_lease, &worker_stop));
    match tokio::time::timeout(remaining, job).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(internal_failure(state)),
        Err(_) => Err(BudgetViolation::AnalysisTimeout.failure(state.next_request_id())),
    }
}

/// Serializes inside the analysis work, so the response bytes count against
/// the slot and the deadline like the rest of the analysis. The writer polls
/// the lease while it grows, so a large report stops mid-serialization.
fn json_response(
    state: &ApiState,
    lease: &WorkLease,
    cancellation: &dyn Cancellation,
    body: impl Serialize,
) -> Result<Response, ApiFailure> {
    let timeout = || BudgetViolation::AnalysisTimeout.failure(state.next_request_id());
    let mut writer = PollingWriter {
        bytes: Vec::with_capacity(128),
        unpolled: 0,
        stopped: &|| lease.check(cancellation).is_err(),
    };
    if let Err(error) = serde_json::to_writer(&mut writer, &body) {
        return Err(if error.is_io() {
            timeout()
        } else {
            internal_failure(state)
        });
    }
    lease.check(cancellation).map_err(|_| timeout())?;
    // `body` (the report) is freed after this returns, outside the slot.
    cancellation.work_complete();
    // Same header as axum's `Json` response.
    Ok((
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        writer.bytes,
    )
        .into_response())
}

/// Response body buffer that polls `stopped` once per `POLL_BYTES` written
/// and fails the write once it returns true.
struct PollingWriter<'a> {
    bytes: Vec<u8>,
    unpolled: usize,
    stopped: &'a dyn Fn() -> bool,
}

impl PollingWriter<'_> {
    const POLL_BYTES: usize = 256 * 1024;
}

impl std::io::Write for PollingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.unpolled += buf.len();
        if self.unpolled >= Self::POLL_BYTES {
            self.unpolled = 0;
            if (self.stopped)() {
                // Not `Interrupted`: `write_all` retries that kind.
                return Err(std::io::Error::other("analysis stopped"));
            }
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn connector_call<T: Send + 'static>(
    state: &ApiState,
    call: impl FnOnce(&SessionManager) -> Result<T, ConnectorError> + Send + 'static,
) -> Result<T, ApiFailure> {
    let sessions = Arc::clone(&state.sessions);
    tokio::task::spawn_blocking(move || call(&sessions))
        .await
        .map_err(|_| internal_failure(state))?
        .map_err(|error| connector_failure(state, error))
}

fn connector_connection_request(request: ConnectionCreateRequest) -> CreateConnectionRequest {
    CreateConnectionRequest {
        host: request.host,
        port: request.port,
        username: request.username,
        password: request.password,
        default_catalog: request.catalog,
        default_database: request.database,
        audit_table: request.audit_table,
        tls_mode: match request.tls_mode {
            PublicTlsMode::VerifyFull => ConnectorTlsMode::VerifyFull,
            PublicTlsMode::DockerLoopbackPlaintext => ConnectorTlsMode::DisabledLoopbackDocker,
            PublicTlsMode::Plaintext => ConnectorTlsMode::Plaintext,
        },
    }
}

fn public_connection_view(view: ConnectorSessionView) -> PublicConnectionSessionView {
    PublicConnectionSessionView {
        connection_id: view.connection_id,
        state: match view.state {
            ConnectionState::Connected => ConnectionSessionState::Ready,
            ConnectionState::Unsupported => ConnectionSessionState::Unsupported,
        },
        tls_mode: match view.tls_mode {
            ConnectorTlsMode::VerifyFull => PublicTlsMode::VerifyFull,
            ConnectorTlsMode::DisabledLoopbackDocker => PublicTlsMode::DockerLoopbackPlaintext,
            ConnectorTlsMode::Plaintext => PublicTlsMode::Plaintext,
        },
        idle_timeout_seconds: view.idle_timeout_seconds.min(u64::from(u32::MAX)) as u32,
        detected_version: Some(VersionEvidence {
            value: view.detected_version,
            source_ref: "connection:current_version".to_owned(),
        }),
        time_zone: view.time_zone.map(|evidence| TimeZoneEvidence {
            session_time_zone: evidence.session_time_zone,
            system_time_zone: evidence.system_time_zone,
            effective_time_zone: evidence.effective_time_zone,
            current_utc_offset_seconds: evidence.current_utc_offset_seconds,
            source_ref: "connection:session_time_zone".to_owned(),
        }),
        capabilities: ConnectionCapabilityReport {
            version_probe: PublicCapabilityState::Available,
            time_zone: public_capability(view.capabilities.time_zone),
            audit: public_capability(view.capabilities.audit_loader),
            audit_user_filter: public_capability(view.capabilities.audit_user_filter),
            audit_statement_filter: public_capability(view.capabilities.audit_statement_filter),
            profile_list: public_capability(view.capabilities.profile_list),
            profile_fetch: public_capability(view.capabilities.get_query_profile),
            explain_costs: public_capability(view.capabilities.explain),
            plan_advisor: public_capability(view.capabilities.plan_advisor),
        },
        limitations: view.limitations,
    }
}

fn public_capability(capability: CapabilityState) -> PublicCapabilityState {
    match capability {
        CapabilityState::Available => PublicCapabilityState::Available,
        CapabilityState::Unavailable => PublicCapabilityState::Unavailable,
        CapabilityState::Forbidden => PublicCapabilityState::Forbidden,
        CapabilityState::Unknown => PublicCapabilityState::Unknown,
    }
}

fn public_running_profile_candidates(
    state: &ApiState,
    connection_id: &str,
    result: RunningProfileCandidates,
    read_at_utc: String,
) -> Result<RunningProfileCandidateListResponse, ApiFailure> {
    let RunningProfileCandidates {
        discovery_generation,
        limit,
        selected_row_count,
        rows,
        plan_sql_references,
        ambiguous_query_ids,
        rejected_row_count: _,
        at_limit,
        local_truncated,
        time_zone: _,
        statement_filter,
    } = result;
    let candidates = rows
        .iter()
        .enumerate()
        .filter_map(|(row_index, row)| {
            let query_id = row.first()?;
            let duplicate_query_id = ambiguous_query_ids.contains(query_id);
            profile_list_query_ref(
                connection_id,
                discovery_generation,
                row_index,
                row,
                plan_sql_references.get(query_id),
                duplicate_query_id,
            )
            .filter(|query| {
                is_valid_query_id(&query.query_id)
                    && !duplicate_query_id
                    && query.status.eq_ignore_ascii_case("running")
            })
        })
        .collect::<Vec<_>>();
    let rejected = selected_row_count.saturating_sub(candidates.len());
    let returned = u32::try_from(candidates.len()).map_err(|_| internal_failure(state))?;
    let rejected = u32::try_from(rejected).map_err(|_| internal_failure(state))?;
    let mut limitations = vec![
        "query_text_omitted_by_contract".to_owned(),
        "profile_list_time_zone_unverified".to_owned(),
        "profile_list_duration_not_normalized_in_v0_1".to_owned(),
        "profile_list_observation_time_unknown".to_owned(),
        "running_snapshot_requires_current_session_generation".to_owned(),
    ];
    if at_limit {
        limitations.push("profile_list_source_at_remote_limit".to_owned());
    }
    if local_truncated {
        limitations.push("running_profile_candidates_locally_truncated".to_owned());
    }
    if rejected > 0 {
        limitations.push(format!("running_profile_candidates_rejected:{rejected}"));
    }
    Ok(RunningProfileCandidateListResponse {
        source: QueryDiscoverySource::ProfileList,
        discovery_generation,
        limit,
        returned,
        at_limit,
        local_truncated,
        rejected,
        read_at_utc,
        profile_observed_at_utc: None,
        candidates,
        limitations,
        statement_filter,
    })
}

fn public_query_result(
    state: &ApiState,
    connection_id: &str,
    result: QueryBrowseResult,
) -> Result<ConnectedQueryListResponse, ApiFailure> {
    let QueryBrowseResult {
        source: connector_source,
        discovery_generation,
        rows,
        plan_sql_references,
        ambiguous_query_ids,
        rejected_row_count,
        degraded_reason,
        time_zone,
        statement_filter,
    } = result;
    let source = match connector_source {
        QuerySource::AuditLoader => QueryDiscoverySource::Audit,
        QuerySource::ProfileList => QueryDiscoverySource::ProfileList,
        QuerySource::Auto => {
            return Err(ApiFailure::new(
                state.next_request_id(),
                StatusCode::BAD_GATEWAY,
                "connector_response_invalid",
                "query_source",
                "query_source_unresolved",
                "retry_same",
            ));
        }
    };
    let filter_scope = match connector_source {
        QuerySource::AuditLoader => QueryFilterScope::AuditWindow,
        QuerySource::ProfileList => QueryFilterScope::LatestProfileCacheRows,
        QuerySource::Auto => {
            return Err(ApiFailure::new(
                state.next_request_id(),
                StatusCode::BAD_GATEWAY,
                "connector_response_invalid",
                "query_source",
                "query_source_unresolved",
                "retry_same",
            ));
        }
    };
    let mut limitations = vec!["query_text_omitted_by_contract".to_owned()];
    if let Some(reason) = degraded_reason {
        limitations.push(reason);
    }
    if rejected_row_count > 0 {
        limitations.push(format!("query_rows_rejected_by_shape:{rejected_row_count}"));
    }
    let queries = match source {
        QueryDiscoverySource::Audit => {
            let time_zone_verified = time_zone.is_some();
            if time_zone_verified {
                limitations.push(
                    "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch"
                        .to_owned(),
                );
            } else {
                limitations.push("audit_timestamp_time_zone_unverified".to_owned());
            }
            let row_count = rows.len();
            let queries = rows
                .iter()
                .enumerate()
                .filter_map(|(row_index, row)| {
                    let plan_sql_ref = row
                        .first()
                        .and_then(|query_id| plan_sql_references.get(query_id));
                    audit_query_ref(
                        connection_id,
                        discovery_generation,
                        row_index,
                        row,
                        time_zone_verified,
                        plan_sql_ref,
                        row.first()
                            .is_some_and(|query_id| ambiguous_query_ids.contains(query_id)),
                    )
                })
                .collect::<Vec<_>>();
            if queries.len() != row_count {
                limitations.push("audit_rows_not_normalized_in_v0_1".to_owned());
            }
            if queries.iter().any(|query| query.observed_at_utc.is_none()) {
                limitations.push("audit_timestamp_normalization_partial".to_owned());
            }
            queries
        }
        QueryDiscoverySource::ProfileList => {
            limitations.push("profile_list_time_zone_unverified".to_owned());
            limitations.push("profile_list_duration_not_normalized_in_v0_1".to_owned());
            let row_count = rows.len();
            let queries = rows
                .iter()
                .enumerate()
                .filter_map(|(row_index, row)| {
                    let plan_sql_ref = row
                        .first()
                        .and_then(|query_id| plan_sql_references.get(query_id));
                    profile_list_query_ref(
                        connection_id,
                        discovery_generation,
                        row_index,
                        row,
                        plan_sql_ref,
                        row.first()
                            .is_some_and(|query_id| ambiguous_query_ids.contains(query_id)),
                    )
                })
                .collect::<Vec<_>>();
            if queries.len() != row_count {
                limitations.push("profile_list_rows_rejected_by_shape".to_owned());
            }
            queries
        }
    };
    Ok(ConnectedQueryListResponse {
        source,
        discovery_generation,
        filter_scope,
        queries,
        limitations,
        statement_filter,
    })
}

fn profile_list_query_ref(
    connection_id: &str,
    discovery_generation: u32,
    row_index: usize,
    row: &[String],
    plan_sql_ref: Option<&String>,
    duplicate_query_id: bool,
) -> Option<ConnectedQueryRef> {
    let [query_id, start_time_without_zone, _display_duration, state] = row else {
        return None;
    };
    let query_id = bounded_text(query_id, 36)?;
    let observed_at_local = bounded_text(start_time_without_zone, 64);
    let status = bounded_text(state, 32)?;
    Some(ConnectedQueryRef {
        connection_id: connection_id.to_owned(),
        profile_fetch: profile_fetch_action(&query_id, &status, duplicate_query_id),
        plan_check: plan_check_action(&query_id, plan_sql_ref, duplicate_query_id),
        query_id,
        discovery_source: Some(QueryDiscoverySource::ProfileList),
        discovery_generation: Some(discovery_generation),
        observed_at_utc: None,
        timestamp_state: if observed_at_local.is_some() {
            ConnectedTimestampState::RawWallTimeOnly
        } else {
            ConnectedTimestampState::Unavailable
        },
        observed_at_local,
        status,
        digest: None,
        database: None,
        resource_group: None,
        query_time_ms: None,
        source_refs: vec![format!("profile_list:row:{row_index}")],
    })
}

fn audit_query_ref(
    connection_id: &str,
    discovery_generation: u32,
    row_index: usize,
    row: &[String],
    time_zone_verified: bool,
    plan_sql_ref: Option<&String>,
    duplicate_query_id: bool,
) -> Option<ConnectedQueryRef> {
    let [
        query_id,
        raw_observed_at,
        event_epoch_seconds,
        state,
        query_time,
        _,
        _,
        _,
        _,
        database,
        resource_group,
        digest,
    ] = row
    else {
        return None;
    };
    let query_id = bounded_text(query_id, 36)?;
    let observed_at_local = bounded_text(raw_observed_at, 64)?;
    let status = if state.eq_ignore_ascii_case("EOF") {
        "finished"
    } else if state.eq_ignore_ascii_case("ERR") {
        "error"
    } else {
        "unknown"
    }
    .to_owned();
    let observed_at_utc = time_zone_verified
        .then(|| unix_seconds_rfc3339(event_epoch_seconds))
        .flatten();
    Some(ConnectedQueryRef {
        connection_id: connection_id.to_owned(),
        profile_fetch: profile_fetch_action(&query_id, &status, duplicate_query_id),
        plan_check: plan_check_action(&query_id, plan_sql_ref, duplicate_query_id),
        query_id,
        discovery_source: Some(QueryDiscoverySource::Audit),
        discovery_generation: Some(discovery_generation),
        timestamp_state: if observed_at_utc.is_some() {
            ConnectedTimestampState::ReaderSessionInterpreted
        } else {
            ConnectedTimestampState::RawWallTimeOnly
        },
        observed_at_utc,
        observed_at_local: Some(observed_at_local),
        status,
        digest: bounded_text(digest, 128),
        database: bounded_text(database, 256),
        resource_group: bounded_text(resource_group, 256),
        query_time_ms: query_time.parse().ok(),
        source_refs: vec![
            format!("audit:row:{row_index}:timestamp"),
            format!("audit:row:{row_index}:epoch"),
        ],
    })
}

fn unix_seconds_rfc3339(value: &str) -> Option<String> {
    // StarRocks 3.5.14 returns 0 both for the Unix epoch and for values outside
    // UNIX_TIMESTAMP's supported 1970..2038 range.  Reject the ambiguous 0
    // instead of manufacturing a 1970 observation.
    let seconds = value
        .parse::<i64>()
        .ok()
        .filter(|seconds| (1..=2_147_483_647).contains(seconds))?;
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

fn public_fetched_query_ref(
    state: &ApiState,
    connection_id: &str,
    context: &FetchedQueryContext,
) -> Result<ConnectedQueryRef, ApiFailure> {
    let mut query = public_observed_query_ref(state, connection_id, context)?;
    query.profile_fetch = ConnectedProfileFetchAction::Available {
        reason: ProfileFetchAvailableReason::ProfileFetched,
    };
    query.source_refs.push("profile:query_id".to_owned());
    Ok(query)
}

fn public_observed_query_ref(
    state: &ApiState,
    connection_id: &str,
    context: &FetchedQueryContext,
) -> Result<ConnectedQueryRef, ApiFailure> {
    match context.source {
        QuerySource::AuditLoader => audit_query_ref(
            connection_id,
            context.discovery_generation,
            context.row_index,
            &context.row,
            context.time_zone.is_some(),
            context.plan_sql_ref.as_ref(),
            false,
        ),
        QuerySource::ProfileList => profile_list_query_ref(
            connection_id,
            context.discovery_generation,
            context.row_index,
            &context.row,
            context.plan_sql_ref.as_ref(),
            false,
        ),
        QuerySource::Auto => None,
    }
    .ok_or_else(|| {
        ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_GATEWAY,
            "connector_response_invalid",
            "query_source",
            "fetched_query_context_invalid",
            "retry_same",
        )
    })
}

fn profile_fetch_action(
    query_id: &str,
    status: &str,
    duplicate_query_id: bool,
) -> ConnectedProfileFetchAction {
    if !is_valid_query_id(query_id) {
        ConnectedProfileFetchAction::Unavailable {
            reason: ProfileFetchUnavailableReason::QueryIdInvalid,
        }
    } else if duplicate_query_id {
        ConnectedProfileFetchAction::Unavailable {
            reason: ProfileFetchUnavailableReason::QueryIdDuplicate,
        }
    } else if !status.eq_ignore_ascii_case("finished") {
        ConnectedProfileFetchAction::Unavailable {
            reason: ProfileFetchUnavailableReason::QueryNotFinished,
        }
    } else {
        ConnectedProfileFetchAction::Available {
            reason: ProfileFetchAvailableReason::CanBeAttempted,
        }
    }
}

fn plan_check_action(
    query_id: &str,
    plan_sql_ref: Option<&String>,
    duplicate_query_id: bool,
) -> ConnectedPlanCheckAction {
    if !is_valid_query_id(query_id) {
        return ConnectedPlanCheckAction::Unavailable {
            reason: PlanCheckUnavailableReason::QueryIdInvalid,
        };
    }
    if duplicate_query_id {
        return ConnectedPlanCheckAction::Unavailable {
            reason: PlanCheckUnavailableReason::QueryIdDuplicate,
        };
    }
    match plan_sql_ref {
        Some(reference) => ConnectedPlanCheckAction::Available {
            reason: PlanCheckAvailableReason::StatementRetained,
            plan_sql_ref: reference.clone(),
        },
        None => ConnectedPlanCheckAction::Unavailable {
            reason: PlanCheckUnavailableReason::StatementMissingOrNotSelect,
        },
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> Option<String> {
    (!value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn require_connection_id(
    state: &ApiState,
    path_connection_id: &str,
    body_connection_id: &str,
) -> Result<(), ApiFailure> {
    (path_connection_id == body_connection_id)
        .then_some(())
        .ok_or_else(|| {
            ApiFailure::new(
                state.next_request_id(),
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "request",
                "connection_id_mismatch",
                "change_input",
            )
        })
}

fn require_distinct_valid_profile_query_ids(
    state: &ApiState,
    baseline_query_id: &str,
    candidate_query_id: &str,
) -> Result<(), ApiFailure> {
    if !is_valid_query_id(baseline_query_id) || !is_valid_query_id(candidate_query_id) {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request",
            "profile_query_id_invalid",
            "change_input",
        ));
    }
    (canonical_query_id(baseline_query_id) != canonical_query_id(candidate_query_id))
        .then_some(())
        .ok_or_else(|| {
            ApiFailure::new(
                state.next_request_id(),
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "request",
                "profile_query_ids_must_differ",
                "change_input",
            )
        })
}

fn with_affected_side(mut failure: ApiFailure, side: &str) -> ApiFailure {
    failure.envelope.affected_side = Some(side.to_owned());
    failure
}

fn ordinary_select(candidate_sql: &str) -> bool {
    let first = candidate_sql.split_whitespace().next();
    matches!(first, Some(keyword) if keyword.eq_ignore_ascii_case("select") || keyword.eq_ignore_ascii_case("with"))
        && !candidate_sql.contains(';')
}

fn connector_failure(state: &ApiState, error: ConnectorError) -> ApiFailure {
    let (status, code, recoverability) = match error.code {
        ConnectorErrorCode::InvalidRequest | ConnectorErrorCode::TlsRequired => (
            StatusCode::BAD_REQUEST,
            match error.code {
                ConnectorErrorCode::TlsRequired => "tls_required",
                _ => "invalid_request",
            },
            "change_input",
        ),
        ConnectorErrorCode::ConnectionFailed => {
            (StatusCode::BAD_GATEWAY, "connection_failed", "restart")
        }
        ConnectorErrorCode::TlsFailed => (StatusCode::BAD_GATEWAY, "tls_failed", "change_input"),
        ConnectorErrorCode::AuthenticationFailed => (
            StatusCode::UNAUTHORIZED,
            "authentication_failed",
            "change_input",
        ),
        ConnectorErrorCode::PermissionDenied => {
            (StatusCode::FORBIDDEN, "permission_denied", "change_input")
        }
        ConnectorErrorCode::CapabilityUnavailable => (
            StatusCode::CONFLICT,
            "capability_unavailable",
            "change_input",
        ),
        ConnectorErrorCode::UnsupportedVersion => {
            (StatusCode::CONFLICT, "unsupported_version", "change_input")
        }
        ConnectorErrorCode::ConnectionExpired => {
            (StatusCode::GONE, "connection_expired", "restart")
        }
        ConnectorErrorCode::ProfileUnavailable => {
            (StatusCode::CONFLICT, "profile_unavailable", "change_input")
        }
        ConnectorErrorCode::QueryDiscoveryStale => (
            StatusCode::CONFLICT,
            "query_discovery_stale",
            "change_input",
        ),
        ConnectorErrorCode::QuerySqlUnavailable => (
            StatusCode::NOT_FOUND,
            "query_sql_unavailable",
            "change_input",
        ),
        ConnectorErrorCode::StatementNotAllowed => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "statement_not_allowed",
            "change_input",
        ),
        ConnectorErrorCode::ExplainRejected => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "explain_rejected",
            "change_input",
        ),
        ConnectorErrorCode::RemoteTimeout => {
            (StatusCode::GATEWAY_TIMEOUT, "remote_timeout", "restart")
        }
        ConnectorErrorCode::ExecutionAttemptSubmitted => (
            StatusCode::CONFLICT,
            "execution_attempt_already_submitted",
            "status_unknown",
        ),
        ConnectorErrorCode::SessionLimit => {
            (StatusCode::TOO_MANY_REQUESTS, "session_limit", "retry_same")
        }
    };
    let mut failure = ApiFailure::new(
        state.next_request_id(),
        status,
        code,
        error.stage,
        code,
        recoverability,
    );
    if let Some(limitation) = error.limitation {
        failure
            .envelope
            .details
            .insert("limitation".to_owned(), limitation.to_owned());
    }
    if let Some(side) = error.affected_side {
        failure.envelope.affected_side = Some(side.to_owned());
    }
    failure
}

fn parse_json_request<T: for<'de> Deserialize<'de>>(
    state: &ApiState,
    body: &[u8],
) -> Result<T, ApiFailure> {
    serde_json::from_slice(body).map_err(|_| {
        ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request",
            "json_request_invalid",
            "change_input",
        )
    })
}

async fn analysis_request_from_http(
    state: &ApiState,
    request: Request,
    lease: &WorkLease,
) -> Result<AnalysisRequest, ApiFailure> {
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    if is_json_request(request.headers()) {
        let body = Bytes::from_request(request, state)
            .await
            .map_err(|_| input_too_large(state))?;
        state
            .budgets
            .check_input_bytes(body.len(), WorkKind::Analysis)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let parsed = parse_json_request(state, &body)?;
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        return Ok(parsed);
    }
    let multipart = Multipart::from_request(request, state)
        .await
        .map_err(|_| multipart_invalid(state, "multipart_invalid"))?;
    let parsed = multipart_analysis_request(state, multipart, lease).await?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    Ok(parsed)
}

async fn comparison_request_from_http(
    state: &ApiState,
    request: Request,
    lease: &WorkLease,
) -> Result<ComparisonRequest, ApiFailure> {
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    if is_json_request(request.headers()) {
        let body = Bytes::from_request(request, state)
            .await
            .map_err(|_| input_too_large(state))?;
        state
            .budgets
            .check_input_bytes(body.len(), WorkKind::Comparison)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let parsed = parse_json_request(state, &body)?;
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        return Ok(parsed);
    }
    let multipart = Multipart::from_request(request, state)
        .await
        .map_err(|_| multipart_invalid(state, "multipart_invalid"))?;
    let parsed = multipart_comparison_request(state, multipart, lease).await?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    Ok(parsed)
}

#[derive(Default)]
struct MultipartParts {
    profile: Option<Vec<u8>>,
    uploaded_filename: Option<String>,
    source_kind: Option<String>,
    filename: Option<String>,
    version_hint: Option<String>,
}

impl MultipartParts {
    fn into_request(self, state: &ApiState) -> Result<AnalysisRequest, ApiFailure> {
        let content = self
            .profile
            .ok_or_else(|| multipart_invalid(state, "multipart_profile_missing"))?;
        let content = String::from_utf8(content)
            .map_err(|_| multipart_invalid(state, "multipart_profile_not_utf8"))?;
        let source_kind = match self.source_kind.as_deref() {
            Some("auto") => RequestedSourceKind::Auto,
            Some("json_profile") => RequestedSourceKind::JsonProfile,
            Some("text_profile") => RequestedSourceKind::TextProfile,
            Some(_) => return Err(multipart_invalid(state, "multipart_source_kind_invalid")),
            None => return Err(multipart_invalid(state, "multipart_source_kind_missing")),
        };
        Ok(AnalysisRequest {
            source_kind,
            content,
            filename: self.filename.or(self.uploaded_filename),
            version_hint: self.version_hint,
        })
    }
}

async fn multipart_analysis_request(
    state: &ApiState,
    mut multipart: Multipart,
    lease: &WorkLease,
) -> Result<AnalysisRequest, ApiFailure> {
    let mut parts = MultipartParts::default();
    loop {
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let Some(field) = multipart
            .next_field()
            .await
            .map_err(|error| multipart_error(state, error))?
        else {
            break;
        };
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let name = field
            .name()
            .map(str::to_owned)
            .ok_or_else(|| multipart_invalid(state, "multipart_field_name_missing"))?;
        let nested = field.content_type().is_some_and(|content_type| {
            content_type
                .split('/')
                .next()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("multipart"))
        });
        if nested {
            return Err(multipart_invalid(state, "multipart_nested_disallowed"));
        }
        match name.as_str() {
            "profile" => {
                if field.file_name().is_none() {
                    return Err(multipart_invalid(state, "multipart_profile_file_required"));
                }
                let filename = field.file_name().map(str::to_owned);
                assign_once(
                    &mut parts.profile,
                    multipart_bytes(field, state, lease).await?,
                    state,
                )?;
                parts.uploaded_filename = filename;
            }
            "source_kind" => assign_once(
                &mut parts.source_kind,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "filename" => assign_once(
                &mut parts.filename,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "version_hint" => assign_once(
                &mut parts.version_hint,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            _ => return Err(multipart_invalid(state, "multipart_field_unknown")),
        }
    }
    parts.into_request(state)
}

async fn multipart_comparison_request(
    state: &ApiState,
    mut multipart: Multipart,
    lease: &WorkLease,
) -> Result<ComparisonRequest, ApiFailure> {
    let mut baseline = MultipartParts::default();
    let mut candidate = MultipartParts::default();
    loop {
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let Some(field) = multipart
            .next_field()
            .await
            .map_err(|error| multipart_error(state, error))?
        else {
            break;
        };
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let name = field
            .name()
            .map(str::to_owned)
            .ok_or_else(|| multipart_invalid(state, "multipart_field_name_missing"))?;
        let nested = field.content_type().is_some_and(|content_type| {
            content_type
                .split('/')
                .next()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("multipart"))
        });
        if nested {
            return Err(multipart_invalid(state, "multipart_nested_disallowed"));
        }
        match name.as_str() {
            "baseline_profile" => {
                multipart_profile_field(&mut baseline, field, state, lease).await?
            }
            "candidate_profile" => {
                multipart_profile_field(&mut candidate, field, state, lease).await?
            }
            "baseline_source_kind" => assign_once(
                &mut baseline.source_kind,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "candidate_source_kind" => assign_once(
                &mut candidate.source_kind,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "baseline_filename" => assign_once(
                &mut baseline.filename,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "candidate_filename" => assign_once(
                &mut candidate.filename,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "baseline_version_hint" => assign_once(
                &mut baseline.version_hint,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            "candidate_version_hint" => assign_once(
                &mut candidate.version_hint,
                multipart_text(field, state, lease).await?,
                state,
            )?,
            _ => return Err(multipart_invalid(state, "multipart_field_unknown")),
        }
    }
    Ok(ComparisonRequest {
        baseline: baseline.into_request(state)?,
        candidate: candidate.into_request(state)?,
    })
}

async fn multipart_profile_field(
    parts: &mut MultipartParts,
    field: axum::extract::multipart::Field<'_>,
    state: &ApiState,
    lease: &WorkLease,
) -> Result<(), ApiFailure> {
    if field.file_name().is_none() {
        return Err(multipart_invalid(state, "multipart_profile_file_required"));
    }
    let filename = field.file_name().map(str::to_owned);
    assign_once(
        &mut parts.profile,
        multipart_bytes(field, state, lease).await?,
        state,
    )?;
    parts.uploaded_filename = filename;
    Ok(())
}

async fn multipart_text(
    field: axum::extract::multipart::Field<'_>,
    state: &ApiState,
    lease: &WorkLease,
) -> Result<String, ApiFailure> {
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    let value = field
        .text()
        .await
        .map_err(|error| multipart_error(state, error))?;
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    (value.len() <= 255)
        .then_some(value)
        .ok_or_else(|| multipart_invalid(state, "multipart_field_too_long"))
}

async fn multipart_bytes(
    mut field: axum::extract::multipart::Field<'_>,
    state: &ApiState,
    lease: &WorkLease,
) -> Result<Vec<u8>, ApiFailure> {
    let mut value = Vec::new();
    let limit = state.budgets.limits().single_input_bytes;
    loop {
        lease
            .check(&NeverCancelled)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
        let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| multipart_error(state, error))?
        else {
            break;
        };
        append_profile_chunk(&mut value, &chunk, limit)
            .map_err(|violation| violation.failure(state.next_request_id()))?;
    }
    lease
        .check(&NeverCancelled)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    Ok(value)
}

fn append_profile_chunk(
    value: &mut Vec<u8>,
    chunk: &[u8],
    limit: usize,
) -> Result<(), BudgetViolation> {
    let next = value
        .len()
        .checked_add(chunk.len())
        .ok_or(BudgetViolation::InputTooLarge)?;
    if next > limit {
        return Err(BudgetViolation::InputTooLarge);
    }
    value.extend_from_slice(chunk);
    Ok(())
}

fn assign_once<T>(slot: &mut Option<T>, value: T, state: &ApiState) -> Result<(), ApiFailure> {
    if slot.is_some() {
        return Err(multipart_invalid(state, "multipart_field_duplicate"));
    }
    *slot = Some(value);
    Ok(())
}

fn multipart_error(state: &ApiState, error: MultipartError) -> ApiFailure {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        input_too_large(state)
    } else {
        multipart_invalid(state, "multipart_invalid")
    }
}

fn input_too_large(state: &ApiState) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "input_too_large",
        "limits",
        "input_too_large",
        "reduce_input",
    )
}

fn multipart_invalid(state: &ApiState, message_id: &str) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "request",
        message_id,
        "change_input",
    )
}

fn is_json_request(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
        })
}

fn comparison_attempt(
    state: &ApiState,
    request: AnalysisRequest,
    side: &str,
    lease: &WorkLease,
    cancellation: &dyn Cancellation,
) -> Result<AnalysisAttempt, ApiFailure> {
    match analyze_request(state, request, lease, cancellation) {
        Ok(report) => Ok(analysis_attempt(report)),
        Err(failure) if comparison_side_failure(&failure) => Ok(failed_attempt(
            &failure.envelope.code,
            &failure.envelope.stage,
            &failure.envelope.message_id,
            &failure.envelope.recoverability,
            side,
        )),
        Err(failure) => Err(failure),
    }
}

fn comparison_side_failure(failure: &ApiFailure) -> bool {
    failure.status == StatusCode::UNPROCESSABLE_ENTITY
        && matches!(
            failure.envelope.code.as_str(),
            "parse_failed" | "kind_mismatch" | "unsupported_input"
        )
}

fn analyze_request(
    state: &ApiState,
    request: AnalysisRequest,
    lease: &WorkLease,
    cancellation: &dyn Cancellation,
) -> Result<AnalysisSnapshotReport, ApiFailure> {
    let mut input = InputEnvelope::new(request.source_kind, request.content.into_bytes());
    input.filename = sanitized_filename(request.filename);
    input.version_hint = request.version_hint;
    analyze_input(state, input, lease, cancellation)
}

/// Stage boundaries are cancellation points, and adapt and assemble also poll
/// the lease inside their long loops (v0.6-08): a stopped or expired analysis
/// discards what it has and never reaches the next stage.
fn analyze_input(
    state: &ApiState,
    mut input: InputEnvelope,
    lease: &WorkLease,
    cancellation: &dyn Cancellation,
) -> Result<AnalysisSnapshotReport, ApiFailure> {
    let timeout = || BudgetViolation::AnalysisTimeout.failure(state.next_request_id());
    let checkpoint = || lease.check(cancellation).map_err(|_| timeout());
    let stopped = || lease.check(cancellation).is_err();
    let cancel = CancelSignal::new(&stopped);
    state
        .budgets
        .check_input_bytes(input.content.len(), WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))?;
    checkpoint()?;
    let limits = adapter_limits(state.budgets.limits());
    let context =
        AdapterContext::new(v01_analysis_receipt()).map_err(|_| internal_failure(state))?;
    input.version_hint = validated_version_hint(state, input.version_hint)?;
    let mut output =
        adapt_input_cancellable(input, &limits, &context, &cancel).map_err(|error| {
            if error.is_cancelled() {
                timeout()
            } else {
                adapter_failure(state, error)
            }
        })?;
    checkpoint()?;
    let graph_evidence = output.graph_evidence.clone();
    resolve_version_and_graph_cancellable(&mut output, graph_evidence, &cancel).map_err(
        |error| match error {
            VersionResolutionError::Cancelled => timeout(),
            _ => internal_failure(state),
        },
    )?;
    checkpoint()?;
    evaluate_rules(&mut output).map_err(|_| internal_failure(state))?;
    checkpoint()?;
    let report =
        assemble_public_report_cancellable(output, &cancel).map_err(|error| match error {
            PublicReportError::Cancelled => timeout(),
            _ => internal_failure(state),
        })?;
    checkpoint()?;
    Ok(report)
}

fn adapter_limits(budgets: &ResourceBudgets) -> AdapterLimits {
    AdapterLimits {
        max_bytes: budgets.single_input_bytes,
        max_text_lines: budgets.text_lines,
        max_text_line_bytes: budgets.text_line_bytes,
        max_json_depth: budgets.json_depth,
        max_json_nodes: budgets.graph_operator_nodes,
        max_normalized_metrics: budgets.normalized_metrics,
    }
}

fn validated_version_hint(
    state: &ApiState,
    hint: Option<String>,
) -> Result<Option<String>, ApiFailure> {
    match hint.as_deref() {
        None | Some("auto") => Ok(hint),
        Some(release)
            if matches!(
                doctor_core::resolve_release(release),
                doctor_core::ReleaseResolution::Audited(_)
            ) =>
        {
            Ok(hint)
        }
        Some(_) => Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request",
            "version_hint_invalid",
            "change_input",
        )),
    }
}

fn sanitized_filename(filename: Option<String>) -> Option<String> {
    let filename = filename?;
    let basename = filename.rsplit(['/', '\\']).next()?.trim();
    if basename.contains(':') {
        return None;
    }
    let display = basename
        .chars()
        .filter(|character| !character.is_control())
        .take(255)
        .collect::<String>();
    (!display.is_empty()).then_some(display)
}

fn adapter_failure(state: &ApiState, error: AdapterError) -> ApiFailure {
    let status = match error.code.as_str() {
        "input_too_large" | "too_many_lines" | "line_too_long" | "too_deep" | "too_many_nodes"
        | "too_many_metrics" => StatusCode::PAYLOAD_TOO_LARGE,
        "parse_failed" | "kind_mismatch" | "unsupported_input" => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    ApiFailure {
        status,
        envelope: ErrorEnvelope {
            request_id: state.next_request_id(),
            code: error.code,
            stage: error.stage,
            message_id: error.message_id,
            recoverability: error.recoverability,
            affected_side: None,
            source_refs: error
                .source_refs
                .into_iter()
                .map(|source| source.ref_id)
                .collect(),
            details: error
                .details
                .into_iter()
                .filter(|(key, _)| matches!(key.as_str(), "actual" | "limit"))
                .collect(),
        },
    }
}

fn internal_failure(state: &ApiState) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "internal",
        "internal_error",
        "report_bug",
    )
}

async fn static_or_not_ready(State(state): State<ApiState>, request: Request) -> Response {
    if request.method() == Method::GET
        && let Some(root) = state.config.frontend_dist.as_deref()
        && let Some(path) = static_asset_path(root, request.uri().path())
        && let Ok(metadata) = std::fs::metadata(&path)
        && metadata.is_file()
        && metadata.len() <= MAX_HTTP_BODY_BYTES as u64
        && let Ok(bytes) = std::fs::read(&path)
    {
        let mut response = bytes.into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, static_content_type(&path));
        return response;
    }
    not_ready(State(state)).await.into_response()
}

fn static_asset_path(root: &Path, uri_path: &str) -> Option<PathBuf> {
    if uri_path.starts_with("/api/") {
        return None;
    }
    let relative = uri_path.trim_start_matches('/');
    let relative = if relative.is_empty() {
        Path::new("index.html")
    } else {
        Path::new(relative)
    };
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let candidate = root.join(relative).canonicalize().ok()?;
    candidate
        .is_file()
        .then_some(candidate)
        .filter(|candidate| candidate.starts_with(&root))
}

fn static_content_type(path: &Path) -> HeaderValue {
    let value = match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };
    HeaderValue::from_static(value)
}

async fn trust_boundary(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0);
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let origin = match state.authorize(&request, peer) {
        Ok(origin) => origin,
        Err(failure) => return failure.into_response(),
    };
    let mut response = next.run(request).await;
    if let Some(origin) = origin.filter(|origin| state.explicit_dev_origin(origin))
        && is_cors_path(&path)
        && (method != Method::OPTIONS || response.status().is_success())
    {
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(&origin).expect("configured origin is a header value"),
        );
        response
            .headers_mut()
            .append(VARY, HeaderValue::from_static("Origin"));
    }
    response
}

fn is_write(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn is_cors_path(path: &str) -> bool {
    matches!(
        path,
        HEALTH_PATH
            | CAPABILITIES_PATH
            | ANALYSES_PATH
            | COMPARISONS_PATH
            | EXPORTS_PATH
            | CONNECTIONS_PATH
    ) || path.starts_with(CONNECTIONS_PREFIX)
}

fn docker_peer(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(address) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    }
}

fn content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn allowed_content_type(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mut parts = value.split(';');
    let media_type = parts.next().unwrap_or_default().trim();
    if media_type.eq_ignore_ascii_case("application/json") {
        return true;
    }
    media_type.eq_ignore_ascii_case("multipart/form-data")
        && parts.any(|part| {
            let Some((name, boundary)) = part.trim().split_once('=') else {
                return false;
            };
            name.trim().eq_ignore_ascii_case("boundary")
                && !boundary.trim().trim_matches('"').is_empty()
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    };

    use axum::{
        body::{Body, to_bytes},
        http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
    };
    use doctor_connector::{Remote, RemoteError, RemoteFactory, RemoteOperation, RemoteResponse};
    use serde_json::from_slice;
    use tower::ServiceExt;

    use super::*;

    static NEXT_STATIC_TEST: AtomicU64 = AtomicU64::new(1);

    /// `audit_connector_call` also arms a real-time watchdog for the lease's remaining budget.
    /// Tests that drive Audit expiry through `TestClock` use this wall so scheduler latency
    /// can never fire that watchdog first; only the injected clock decides expiry.
    const CLOCK_DRIVEN_ANALYSIS_WALL: Duration = Duration::from_secs(60);
    const PAST_CLOCK_DRIVEN_ANALYSIS_WALL_MS: u64 =
        CLOCK_DRIVEN_ANALYSIS_WALL.as_millis() as u64 + 1;

    #[derive(Default)]
    struct TestClock(AtomicU64);

    impl TestClock {
        fn set_ms(&self, value: u64) {
            self.0.store(value, Ordering::Relaxed);
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::Relaxed))
        }
    }

    #[derive(Default)]
    struct TestCancellation(AtomicBool);

    impl Cancellation for TestCancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    fn state_with(config: ApiConfig) -> ApiState {
        ApiState::with_seams(config, "fresh-nonce", Arc::new(TestClock::default())).unwrap()
    }

    #[tokio::test]
    async fn capabilities_advertise_the_current_analysis_receipt_and_pack() {
        let build = BuildCapability {
            git_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            candidate_label: "v0.1-test-candidate".into(),
        };
        let response = router(state_with(ApiConfig {
            build: build.clone(),
            ..ApiConfig::default()
        }))
        .oneshot(request(
            Method::GET,
            CAPABILITIES_PATH,
            "127.0.0.1:8000",
            SocketAddr::from(([127, 0, 0, 1], 41000)),
        ))
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let capabilities: Capabilities = from_slice(&body).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../version-packs/starrocks_3_5_14/version-pack.json"
        ))
        .unwrap();
        assert_eq!(capabilities.version_packs.len(), 1);
        let advertised = &capabilities.version_packs[0];
        assert_eq!(advertised.pack_id, manifest["pack_id"].as_str().unwrap());
        assert_eq!(
            advertised.product_pack_version,
            manifest["product_pack_version"].as_str().unwrap()
        );
        assert_eq!(
            advertised.upstream_commit,
            manifest["upstream_commit"].as_str().unwrap()
        );
        assert_eq!(
            advertised.manifest_hash,
            manifest["semantic_manifest_hash"].as_str().unwrap()
        );
        let receipt = v01_analysis_receipt();
        let pack = receipt
            .components
            .iter()
            .find(|component| component.id == advertised.pack_id)
            .unwrap();
        assert_eq!(advertised.product_pack_version, pack.version);
        assert_eq!(advertised.manifest_hash, pack.manifest_hash);
        let rule_set = receipt
            .components
            .iter()
            .find(|component| component.id == "doctor-v0.1")
            .unwrap();
        assert_eq!(
            capabilities.rule_sets,
            vec![format!("{}@{}", rule_set.id, rule_set.version)]
        );
        assert_eq!(capabilities.build, build);
    }

    fn request(method: Method, path: &str, host: &str, peer: SocketAddr) -> Request {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, host)
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }

    fn json_request(
        method: Method,
        path: &str,
        host: &str,
        peer: SocketAddr,
        body: &str,
    ) -> Request {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, host)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }

    fn multipart_request(path: &str, host: &str, peer: SocketAddr, body: String) -> Request {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(HOST, host)
            .header(
                CONTENT_TYPE,
                "multipart/form-data; boundary=doctor-test-boundary",
            )
            .body(Body::from(body))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }

    fn multipart_body(parts: &[(&str, Option<&str>, &str)]) -> String {
        let mut body = String::new();
        for (name, filename, value) in parts {
            body.push_str("--doctor-test-boundary\r\nContent-Disposition: form-data; name=\"");
            body.push_str(name);
            body.push('"');
            if let Some(filename) = filename {
                body.push_str("; filename=\"");
                body.push_str(filename);
                body.push('"');
            }
            body.push_str("\r\n\r\n");
            body.push_str(value);
            body.push_str("\r\n");
        }
        body.push_str("--doctor-test-boundary--\r\n");
        body
    }

    async fn envelope(response: Response) -> ErrorEnvelope {
        from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[derive(Clone)]
    struct ConnectorTestFactory {
        version: String,
        profile: String,
        profile_error: Option<RemoteError>,
        audit_rows: Vec<Vec<String>>,
        audit_schema_columns: Vec<String>,
        audit_family_population_max: Option<String>,
        profile_list_rows: Vec<Vec<String>>,
        advisor_rows: Vec<Vec<String>>,
        explain_rows: Vec<String>,
        closes: Arc<AtomicUsize>,
        profile_calls: Arc<AtomicUsize>,
        explain_calls: Arc<AtomicUsize>,
        explain_expected_mode: Option<doctor_contracts::ExplainMode>,
        official_calls: Arc<AtomicUsize>,
        official_result: Option<Result<Vec<String>, RemoteError>>,
        official_expected: Option<(String, Vec<u32>)>,
        official_hook: Option<Arc<dyn Fn() + Send + Sync>>,
        runtime_calls: Arc<AtomicUsize>,
        explain_hook: Option<Arc<dyn Fn() + Send + Sync>>,
        family_detail_hook: Option<Arc<dyn Fn() + Send + Sync>>,
        family_detail_error: Option<RemoteError>,
        family_detail_rows: Option<Vec<Vec<Option<String>>>>,
        family_detail_calls: Arc<AtomicUsize>,
        audit_sql_calls: Arc<AtomicUsize>,
        profile_list_calls: Arc<AtomicUsize>,
    }

    impl ConnectorTestFactory {
        fn connected() -> Self {
            Self {
                version: "3.5.14".into(),
                profile:
                    "StarRocks Profile 3.5.14\nOperatorTotalTime: 500ms\nconnector-profile-secret\n"
                        .into(),
                profile_error: None,
                audit_family_population_max: Some("600".into()),
                audit_rows: ["3514", "3515"]
                    .into_iter()
                    .map(|suffix| {
                        vec![
                            format!("00000000-0000-0000-0000-00000000{suffix}"),
                            "2026-08-30 00:00:00".into(),
                            "1788048000".into(),
                            "EOF".into(),
                            "500".into(),
                            "0".into(),
                            "0".into(),
                            "0".into(),
                            "0".into(),
                            "demo".into(),
                            "".into(),
                            format!("sha256:fixture-{suffix}"),
                            format!("SELECT secret_canary_{suffix} FROM t"),
                        ]
                    })
                    .collect(),
                audit_schema_columns: [
                    "queryId",
                    "timestamp",
                    "state",
                    "queryTime",
                    "scanBytes",
                    "scanRows",
                    "cpuCostNs",
                    "memCostBytes",
                    "returnRows",
                    "catalog",
                    "db",
                    "resourceGroup",
                    "digest",
                    "stmt",
                    "errorCode",
                    "pendingTimeMs",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
                profile_list_rows: vec![vec![
                    "00000000-0000-0000-0000-000000003514".into(),
                    "2026-08-30 00:00:00".into(),
                    "500ms".into(),
                    "Finished".into(),
                    "SELECT secret_canary FROM t".into(),
                ]],
                advisor_rows: vec![vec!["advisor-raw-secret".into()]],
                explain_rows: vec!["explain-raw-secret".into()],
                closes: Arc::new(AtomicUsize::new(0)),
                profile_calls: Arc::new(AtomicUsize::new(0)),
                explain_calls: Arc::new(AtomicUsize::new(0)),
                explain_expected_mode: None,
                official_calls: Arc::new(AtomicUsize::new(0)),
                official_result: None,
                official_expected: None,
                official_hook: None,
                runtime_calls: Arc::new(AtomicUsize::new(0)),
                explain_hook: None,
                family_detail_hook: None,
                family_detail_error: None,
                family_detail_rows: None,
                family_detail_calls: Arc::new(AtomicUsize::new(0)),
                audit_sql_calls: Arc::new(AtomicUsize::new(0)),
                profile_list_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_audit_user_column(mut self) -> Self {
            self.audit_schema_columns.push("user".into());
            self
        }

        fn with_audit_statement_filter_column(mut self) -> Self {
            self.audit_schema_columns.push("isQuery".into());
            self
        }
    }

    fn api_family_detail_rows(
        target: &doctor_connector::AuditFamilyTarget,
        candidates: &[String],
    ) -> Vec<Vec<Option<String>>> {
        let id = "00000000-0000-0000-0000-000000003514";
        let mut summary = vec![Some("summary".into())];
        summary.extend(
            [
                "1", "1", "0", "0", "0", "1", "0", "0", "0", "0", "0", "0", "0",
            ]
            .map(|v| Some(v.into())),
        );
        summary.extend(vec![None; 18]);
        let mut rows = vec![summary];
        for role in ["recent", "slowest", "profile_known"] {
            if role == "profile_known" && !candidates.iter().any(|candidate| candidate == id) {
                continue;
            }
            let mut row = vec![Some(role.into())];
            row.extend(vec![None; 13]);
            row.extend([
                Some(id.into()),
                Some("2026-08-30 00:00:00".into()),
                Some("1788048000".into()),
                Some("EOF".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into()),
                Some(target.catalog.clone()),
                Some(target.database.clone()),
                None,
                Some(target.native_digest.clone()),
                Some("".into()),
                Some("1".into()),
                Some("0".into()),
                Some("1".into()),
            ]);
            rows.push(row);
        }
        rows
    }

    struct ConnectorTestRemote(ConnectorTestFactory);

    impl Remote for ConnectorTestRemote {
        fn run(&mut self, operation: RemoteOperation) -> Result<RemoteResponse, RemoteError> {
            match operation {
                RemoteOperation::CurrentVersion => {
                    Ok(RemoteResponse::Version(self.0.version.clone()))
                }
                RemoteOperation::SessionTimeZone => Ok(RemoteResponse::TimeZone(
                    doctor_connector::SessionTimeZone {
                        session_time_zone: "SYSTEM".into(),
                        system_time_zone: "Etc/UTC".into(),
                        effective_time_zone: "Etc/UTC".into(),
                        current_utc_offset_seconds: 0,
                    },
                )),
                RemoteOperation::ShowProfileList { .. } => {
                    self.0.profile_list_calls.fetch_add(1, Ordering::Relaxed);
                    Ok(RemoteResponse::Rows(self.0.profile_list_rows.clone()))
                }
                RemoteOperation::AuditProbe { .. } | RemoteOperation::ShowPlanAdvisor => {
                    Ok(RemoteResponse::Rows(self.0.advisor_rows.clone()))
                }
                RemoteOperation::AuditSearch { .. } => {
                    Ok(RemoteResponse::Rows(self.0.audit_rows.clone()))
                }
                RemoteOperation::AuditSearchHiddenCounts { .. } => {
                    Ok(RemoteResponse::Rows(vec![vec!["3".into(), "4".into()]]))
                }
                RemoteOperation::AuditSchemaProbe { .. } => Ok(RemoteResponse::NullableRows(
                    self.0
                        .audit_schema_columns
                        .iter()
                        .map(|name| {
                            let source_type = match name.as_str() {
                                "timestamp" => "DATETIME",
                                "queryTime" | "scanBytes" | "scanRows" | "cpuCostNs"
                                | "memCostBytes" | "returnRows" => "BIGINT",
                                "pendingTimeMs" => "BIGINT",
                                "isQuery" => "TINYINT",
                                _ => "VARCHAR",
                            };
                            vec![Some(name.clone()), Some(source_type.into())]
                        })
                        .collect(),
                )),
                RemoteOperation::AuditOccurrences { .. } => Ok(RemoteResponse::NullableRows(
                    self.0
                        .audit_rows
                        .iter()
                        .enumerate()
                        .map(|(index, row)| {
                            vec![
                                Some(row[0].clone()),
                                Some(row[1].clone()),
                                Some(row[2].clone()),
                                Some(row[3].clone()),
                                Some(row[4].clone()),
                                if index == 0 { None } else { Some("0".into()) },
                                Some("9007199254740992".into()),
                                Some("-1".into()),
                                Some("9223372036854775808".into()),
                                Some("0".into()),
                                Some("default_catalog".into()),
                                Some(row[9].clone()),
                                None,
                                Some("a0b1".into()),
                                Some("".into()),
                                Some("1".into()),
                                Some(if index == 0 { "-1" } else { "0" }.into()),
                                Some("1".into()),
                            ]
                        })
                        .collect(),
                )),
                RemoteOperation::AuditCollectionSlice {
                    table,
                    scope,
                    schema,
                } => {
                    doctor_connector::audit_collection_slice_statement(&table, &scope, &schema)
                        .unwrap();
                    let row = |index: u32, sql: &str| {
                        let mut row: Vec<Option<String>> = vec![Some("1".into()); 23];
                        row[0] = Some(format!("00000000-0000-0000-0000-00000000000{index}"));
                        row[1] = Some("2026-08-30 00:00:00".into());
                        row[2] = Some((scope.window.start_seconds + index).to_string());
                        row[3] = Some("EOF".into());
                        (row[10], row[11], row[13]) =
                            (Some("default_catalog".into()), Some("demo".into()), None);
                        row[18] = Some(sql.into());
                        (row[19], row[20], row[21]) =
                            (Some("0".into()), Some("0".into()), Some("1".into()));
                        row[22] = Some("2".into());
                        row
                    };
                    Ok(RemoteResponse::NullableRows(vec![
                        row(1, "SELECT id FROM t WHERE name = 'history_secret_literal'"),
                        row(2, "SELECT id FROM t WHERE name = 'other'"),
                    ]))
                }
                RemoteOperation::AuditFingerprintEvents {
                    table,
                    scope,
                    schema,
                } => {
                    let sql = doctor_connector::audit_fingerprint_event_statement(
                        &table, &scope, &schema,
                    )
                    .unwrap();
                    assert!(sql.contains("`digest` NOT REGEXP") && sql.contains("LIMIT 500"));
                    // Four eligible in the window, three read: a partial source.
                    Ok(RemoteResponse::NullableRows(
                        ["0", "1", "2"]
                            .into_iter()
                            .map(|index| {
                                vec![
                                    Some(format!("00000000-0000-0000-0000-00000000000{index}")),
                                    Some("2026-08-30 00:00:00".into()),
                                    Some("1788048000".into()),
                                    Some(if index == "1" { "ERR" } else { "EOF" }.into()),
                                    Some("10".into()),
                                    Some("100".into()),
                                    Some("1".into()),
                                    Some("1000".into()),
                                    Some("2048".into()),
                                    Some("1".into()),
                                    Some("default_catalog".into()),
                                    Some("demo".into()),
                                    None,
                                    None,
                                    None,
                                    Some("1".into()),
                                    None,
                                    Some("1".into()),
                                    Some("4".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                RemoteOperation::AuditFingerprintSamples { query_ids, .. } => {
                    Ok(RemoteResponse::NullableRows(
                        query_ids
                            .into_iter()
                            .map(|query_id| {
                                let (sql, truncated) = match query_id.chars().last() {
                                    Some('0') => ("SELECT id FROM t WHERE id = 1", "0"),
                                    Some('1') => {
                                        ("select id from demo.t where id = 'secret_literal'", "0")
                                    }
                                    _ => ("SELECT id FROM t WHERE id IN (1", "1"),
                                };
                                vec![
                                    Some(query_id),
                                    Some(sql.into()),
                                    Some("0".into()),
                                    Some(truncated.into()),
                                    Some("1".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                RemoteOperation::AuditFamilyDetail {
                    table,
                    parent,
                    target,
                    candidates,
                    schema,
                } => {
                    self.0.family_detail_calls.fetch_add(1, Ordering::Relaxed);
                    if let Some(hook) = &self.0.family_detail_hook {
                        hook();
                    }
                    if let Some(error) = self.0.family_detail_error {
                        return Err(error);
                    }
                    assert!(parent.native_digest.is_none());
                    let statement = doctor_connector::audit_family_detail_statement(
                        &table,
                        &parent,
                        &target,
                        &candidates,
                        &schema,
                    )
                    .unwrap();
                    assert!(!statement.contains("AS `statement`"));
                    assert!(candidates.len() <= 500);
                    Ok(RemoteResponse::NullableRows(
                        self.0
                            .family_detail_rows
                            .clone()
                            .unwrap_or_else(|| api_family_detail_rows(&target, &candidates)),
                    ))
                }
                RemoteOperation::AuditFamilyDetailSqlLookup {
                    table,
                    parent,
                    target,
                    query_id,
                    schema,
                } => {
                    self.0.audit_sql_calls.fetch_add(1, Ordering::Relaxed);
                    assert!(parent.native_digest.is_none());
                    doctor_connector::audit_family_detail_sql_lookup_statement(
                        &table, &parent, &target, &query_id, &schema,
                    )
                    .unwrap();
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some(query_id),
                        Some("2026-08-30 00:00:00".into()),
                        Some("SELECT secret_canary FROM t".into()),
                    ]]))
                }
                RemoteOperation::AuditFamilySamples { targets, .. } => {
                    Ok(RemoteResponse::NullableRows(
                        targets
                            .into_iter()
                            .map(|target| {
                                vec![
                                    Some(target.catalog),
                                    Some(target.database),
                                    Some(target.native_digest),
                                    Some("SELECT secret_canary FROM t".into()),
                                    Some("0".into()),
                                    Some("0".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                // A bounded prefix, distinct from the full on-demand SQL canary.
                RemoteOperation::AuditOccurrenceSamples { query_ids, .. } => {
                    Ok(RemoteResponse::NullableRows(
                        query_ids
                            .into_iter()
                            .map(|query_id| {
                                vec![
                                    Some(query_id),
                                    Some("SELECT occurrence_prefix".into()),
                                    Some("0".into()),
                                    Some("1".into()),
                                    Some("1".into()),
                                ]
                            })
                            .collect(),
                    ))
                }
                RemoteOperation::AuditFamilies { .. } => {
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some("default_catalog".into()),
                        Some("demo".into()),
                        Some("a0b1".into()),
                        Some("600".into()),
                        Some("1".into()),
                        Some("600".into()),
                        Some("300000".into()),
                        Some("599".into()),
                        Some("0".into()),
                        Some("0".into()),
                        None,
                        Some("600".into()),
                        Some("9223372036854775808".into()),
                        Some("500".into()),
                        self.0.audit_family_population_max.clone(),
                    ]]))
                }
                RemoteOperation::AuditCoverage { scope, .. } => {
                    // Full-window oracle deliberately differs from the two visible rows.
                    let hidden = if scope.statement_filter.is_applied() {
                        [Some("7".into()), Some("3".into())]
                    } else {
                        [None, None]
                    };
                    Ok(RemoteResponse::NullableRows(vec![
                        vec![
                            Some("606".into()),
                            Some("0".into()),
                            Some("4".into()),
                            Some("2".into()),
                            Some("38".into()),
                            Some("0".into()),
                            Some("600".into()),
                            Some("1".into()),
                            Some("0".into()),
                            Some("600".into()),
                            Some("600".into()),
                            Some("0".into()),
                            Some("300000".into()),
                            Some("1".into()),
                            Some("599".into()),
                            Some("599".into()),
                            Some("0".into()),
                            Some("0".into()),
                            Some("0".into()),
                            Some("600".into()),
                            Some("0".into()),
                            Some("600".into()),
                            None,
                            Some("0".into()),
                            Some("600".into()),
                            Some("600".into()),
                            Some("0".into()),
                            Some("9223372036854775808".into()),
                            Some("1".into()),
                        ]
                        .into_iter()
                        .chain(hidden)
                        .collect(),
                    ]))
                }
                RemoteOperation::AuditConcurrency { scope, .. } => {
                    let start = i128::from(scope.window.start_seconds) * 1_000;
                    let summary = vec![
                        Some("0".into()),
                        None,
                        None,
                        Some("601".into()),
                        Some("2".into()),
                        Some("1".into()),
                        Some("1".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("0".into()),
                        Some("00:00:00".into()),
                        Some("00:15:00".into()),
                        Some("00:30:00".into()),
                        Some("00:45:00".into()),
                        Some("01:00:00".into()),
                        None,
                    ];
                    let delta = vec![
                        Some("1".into()),
                        Some((start + 1_000).to_string()),
                        Some("-1".into()),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some("00:00:01".into()),
                    ];
                    Ok(RemoteResponse::NullableRows(vec![summary, delta]))
                }
                RemoteOperation::AuditOverviewFamilyTop { .. } => {
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some("default_catalog".into()),
                        Some("demo".into()),
                        Some("a0b1".into()),
                        Some("600".into()),
                        Some("1".into()),
                        Some("600".into()),
                        Some("300000".into()),
                        Some("599".into()),
                        Some("0".into()),
                        Some("0".into()),
                        None,
                        Some("600".into()),
                        Some("9223372036854775808".into()),
                        Some("500".into()),
                        self.0.audit_family_population_max.clone(),
                    ]]))
                }
                RemoteOperation::AuditSqlLookup { request, .. } => {
                    self.0.audit_sql_calls.fetch_add(1, Ordering::Relaxed);
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some(request.query_id),
                        Some("2026-08-30 00:00:00".into()),
                        Some("SELECT secret_canary FROM t".into()),
                    ]]))
                }
                RemoteOperation::AuditActiveAt {
                    at_milliseconds, ..
                } => {
                    let start = at_milliseconds - 1_000;
                    Ok(RemoteResponse::NullableRows(vec![
                        vec![
                            Some("0".into()),
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some("1".into()),
                        ],
                        vec![
                            Some("1".into()),
                            Some("00000000-0000-0000-0000-000000003514".into()),
                            Some("2026-08-30 00:00:00".into()),
                            Some((start / 1_000).to_string()),
                            Some("EOF".into()),
                            Some("2000".into()),
                            Some("0".into()),
                            Some("0".into()),
                            Some("0".into()),
                            Some("0".into()),
                            Some("0".into()),
                            Some("default_catalog".into()),
                            Some("demo".into()),
                            None,
                            Some("a0b1".into()),
                            Some("".into()),
                            Some("1".into()),
                            Some("0".into()),
                            Some("1".into()),
                            None,
                        ],
                    ]))
                }
                RemoteOperation::AuditActiveAtSqlLookup { request, .. } => {
                    Ok(RemoteResponse::NullableRows(vec![vec![
                        Some(request.query_id),
                        Some("2026-08-30 00:00:00".into()),
                        Some("SELECT secret_canary FROM t".into()),
                    ]]))
                }
                RemoteOperation::GetQueryProfile { .. } => {
                    self.0.profile_calls.fetch_add(1, Ordering::Relaxed);
                    match self.0.profile_error {
                        Some(error) => Err(error),
                        None => Ok(RemoteResponse::Profile(self.0.profile.clone())),
                    }
                }
                RemoteOperation::AnalyzeProfile { query_id, node_ids } => {
                    self.0.official_calls.fetch_add(1, Ordering::Relaxed);
                    if let Some(expected) = &self.0.official_expected {
                        assert_eq!(&(query_id, node_ids), expected);
                    }
                    if let Some(hook) = &self.0.official_hook {
                        hook();
                    }
                    self.0
                        .official_result
                        .clone()
                        .expect("existing API fixture must not implicitly read official analysis")
                        .map(RemoteResponse::AnalyzeProfile)
                }
                RemoteOperation::Explain { mode, .. } => {
                    if let Some(expected) = &self.0.explain_expected_mode {
                        assert_eq!(&mode, expected);
                    }
                    self.0.explain_calls.fetch_add(1, Ordering::Relaxed);
                    if let Some(hook) = &self.0.explain_hook {
                        hook();
                    }
                    Ok(RemoteResponse::Explain(self.0.explain_rows.clone()))
                }
                RemoteOperation::ProfiledSelect { .. } => {
                    self.0.runtime_calls.fetch_add(1, Ordering::Relaxed);
                    Ok(RemoteResponse::ProfiledQuery {
                        query_id: "33333333-3333-3333-3333-333333333333".into(),
                        profile: self.0.profile.clone(),
                    })
                }
            }
        }

        fn close(&mut self) {
            self.0.closes.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl RemoteFactory for ConnectorTestFactory {
        fn open(
            &self,
            _: doctor_connector::RemoteConnectOptions,
        ) -> Result<Box<dyn Remote>, RemoteError> {
            Ok(Box::new(ConnectorTestRemote(self.clone())))
        }
    }

    #[derive(Default)]
    struct ConnectorTestClock(AtomicU64);

    impl ConnectorTestClock {
        fn set_seconds(&self, value: u64) {
            self.0.store(value, Ordering::Relaxed);
        }
    }

    impl doctor_connector::Clock for ConnectorTestClock {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.load(Ordering::Relaxed))
        }
    }

    fn connector_state(factory: Arc<ConnectorTestFactory>) -> ApiState {
        ApiState::with_connector_factory(
            ApiConfig::default(),
            "fresh-nonce",
            Arc::new(TestClock::default()),
            factory,
        )
        .unwrap()
    }

    fn connector_request_body() -> &'static str {
        r#"{"host":"127.0.0.1","port":9030,"username":"reader","password":"write-only","catalog":null,"database":null,"audit_table":null,"tls_mode":"docker_loopback_plaintext"}"#
    }

    async fn connector_connection_id(app: Router) -> String {
        let response = app
            .oneshot(json_request(
                Method::POST,
                CONNECTIONS_PATH,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                connector_request_body(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        value["connection_id"].as_str().unwrap().to_owned()
    }

    async fn observe_connector_queries(app: Router, connection_id: &str) -> serde_json::Value {
        let body = serde_json::json!({
            "connection_id": connection_id,
            "limit": 100,
            "status": "finished",
            "digest": null,
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/queries/search"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn query_discovery_reports_the_statement_filter_outcome_without_guessing() {
        async fn discover(
            app: Router,
            connection_id: &str,
            statement_filter: Option<&str>,
        ) -> (StatusCode, serde_json::Value) {
            let mut body = serde_json::json!({
                "connection_id": connection_id, "limit": 100, "status": "finished", "digest": null,
            });
            if let Some(filter) = statement_filter {
                body["statement_filter"] = serde_json::json!(filter);
            }
            let response = app
                .oneshot(json_request(
                    Method::POST,
                    &format!("{CONNECTIONS_PATH}/{connection_id}/queries/search"),
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &body.to_string(),
                ))
                .await
                .unwrap();
            let status = response.status();
            let value = from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap_or(serde_json::Value::Null);
            (status, value)
        }

        let factory =
            Arc::new(ConnectorTestFactory::connected().with_audit_statement_filter_column());
        let app = router(connector_state(factory));
        let id = connector_connection_id(app.clone()).await;
        let (status, applied) =
            discover(app.clone(), &id, Some("exclude_doctor_and_non_query")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(applied["source"], "audit");
        assert_eq!(
            applied["statement_filter"],
            serde_json::json!({"state":"applied", "hidden_non_query_row_count":"3", "hidden_doctor_row_count":"4"})
        );
        // Omitting the field keeps the earlier unfiltered request.
        let (status, legacy) = discover(app.clone(), &id, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            legacy["statement_filter"],
            serde_json::json!({"state":"not_applied"})
        );
        assert_eq!(legacy["queries"].as_array().unwrap().len(), 2);
        let (status, _) = discover(app.clone(), &id, Some("doctor_only")).await;
        assert!(status.is_client_error());

        // No proven `isQuery`: nothing hidden, and the response says so.
        let app = router(connector_state(Arc::new(ConnectorTestFactory::connected())));
        let id = connector_connection_id(app.clone()).await;
        let (status, unproven) = discover(app, &id, Some("exclude_doctor_and_non_query")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            unproven["statement_filter"],
            serde_json::json!({"state":"indistinguishable", "reason":"audit_statement_filter_unavailable"})
        );
        assert_eq!(unproven["queries"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn trust_boundary_rejects_rebinding_cross_site_bad_nonce_and_media_type() {
        let mut config = ApiConfig::default();
        config
            .allowed_dev_origins
            .insert("http://127.0.0.1:5173".into());
        config.allowed_dev_hosts.insert("127.0.0.1:5173".into());
        let app = router(state_with(config));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));

        let rebinding = request(
            Method::POST,
            ANALYSES_PATH,
            "attacker.invalid:8000",
            loopback,
        );
        let response = app.clone().oneshot(rebinding).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "host_not_allowed");

        let mut cross_site = request(Method::POST, ANALYSES_PATH, "127.0.0.1:8000", loopback);
        cross_site
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("https://attacker.invalid"));
        cross_site.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("multipart/form-data; boundary=attack"),
        );
        let response = app.clone().oneshot(cross_site).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "origin_not_allowed");

        let mut bad_nonce = request(Method::POST, ANALYSES_PATH, "127.0.0.1:5173", loopback);
        bad_nonce
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:5173"));
        bad_nonce
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        bad_nonce
            .headers_mut()
            .insert(CSRF_HEADER, HeaderValue::from_static("old-nonce"));
        let response = app.clone().oneshot(bad_nonce).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "csrf_invalid");

        let mut missing_nonce = request(Method::POST, ANALYSES_PATH, "127.0.0.1:5173", loopback);
        missing_nonce
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:5173"));
        missing_nonce
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = app.clone().oneshot(missing_nonce).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "csrf_invalid");

        let mut null_origin = request(Method::GET, CAPABILITIES_PATH, "127.0.0.1:8000", loopback);
        null_origin
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("null"));
        let response = app.clone().oneshot(null_origin).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "origin_not_allowed");

        let mut too_large = request(Method::POST, ANALYSES_PATH, "127.0.0.1:8000", loopback);
        too_large
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        too_large
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("35651585"));
        let response = app.clone().oneshot(too_large).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(envelope(response).await.code, "input_too_large");

        let mut bad_content_type = request(Method::POST, ANALYSES_PATH, "127.0.0.1:8000", loopback);
        bad_content_type
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        *bad_content_type.body_mut() =
            Body::from("profile sql: SELECT secret_value FROM credentials");
        let response = app.oneshot(bad_content_type).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains("secret_value"));
        let error: ErrorEnvelope = from_slice(text.as_bytes()).unwrap();
        assert_eq!(error.code, "unsupported_media_type");
    }

    #[tokio::test]
    async fn trust_boundary_allows_explicit_dev_proxy_and_exact_preflight() {
        let mut config = ApiConfig::default();
        config
            .allowed_dev_origins
            .insert("http://127.0.0.1:5173".into());
        config.allowed_dev_hosts.insert("127.0.0.1:5173".into());
        let app = router(state_with(config));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));

        let mut capability = request(Method::GET, CAPABILITIES_PATH, "127.0.0.1:5173", loopback);
        capability
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:5173"));
        let response = app.clone().oneshot(capability).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://127.0.0.1:5173"
        );
        assert_eq!(response.headers()[VARY], "Origin");
        assert!(
            response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .is_none()
        );
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let capabilities: Capabilities =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(
            capabilities.limits.get("single_input_bytes"),
            Some(&16_777_216)
        );
        assert_eq!(
            capabilities.limits.get("comparison_body_bytes"),
            Some(&35_651_584)
        );
        assert_eq!(capabilities.limits.get("analysis_wall_ms"), Some(&10_000));
        assert_eq!(capabilities.limits.get("concurrent_requests"), Some(&2));

        let health = request(Method::GET, HEALTH_PATH, "127.0.0.1:5173", loopback);
        let response = app.clone().oneshot(health).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let health: BTreeMap<String, String> =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(health.get("status"), Some(&"ok".to_owned()));

        let mut preflight = request(Method::OPTIONS, ANALYSES_PATH, "127.0.0.1:5173", loopback);
        preflight
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:5173"));
        preflight.headers_mut().insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("POST"),
        );
        preflight.headers_mut().insert(
            ACCESS_CONTROL_REQUEST_HEADERS,
            HeaderValue::from_static("Content-Type, X-Doctor-CSRF"),
        );
        let response = app.clone().oneshot(preflight).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://127.0.0.1:5173"
        );
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_METHODS], "POST");
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "Content-Type, X-Doctor-CSRF"
        );

        let profile = r#"{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\"}"}"#;
        let mut dev_post = json_request(
            Method::POST,
            ANALYSES_PATH,
            "127.0.0.1:5173",
            loopback,
            profile,
        );
        dev_post
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:5173"));
        dev_post
            .headers_mut()
            .insert(CSRF_HEADER, HeaderValue::from_static("fresh-nonce"));
        let response = app.clone().oneshot(dev_post).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://127.0.0.1:5173"
        );
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("raw_text"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["outcome"]["report"]["analysis_state"],
            "partial"
        );

        let cli_post = json_request(
            Method::POST,
            ANALYSES_PATH,
            "127.0.0.1:8000",
            loopback,
            profile,
        );
        let response = app.oneshot(cli_post).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn analysis_rejects_no_facts_and_comparison_keeps_failed_side_in_a_200_response() {
        let app = router(state_with(ApiConfig::default()));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let no_facts = json_request(
            Method::POST,
            ANALYSES_PATH,
            "127.0.0.1:8000",
            loopback,
            r#"{"source_kind":"json_profile","content":"{\"fixture\":\"secret-profile\"}"}"#,
        );
        let response = app.clone().oneshot(no_facts).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("secret-profile"));
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&text)
                .unwrap()
                .message_id,
            "no_trusted_facts"
        );

        let invalid_hint = json_request(
            Method::POST,
            ANALYSES_PATH,
            "127.0.0.1:8000",
            loopback,
            r#"{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\"}","version_hint":"secret-version"}"#,
        );
        let response = app.clone().oneshot(invalid_hint).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("secret-version"));
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&text)
                .unwrap()
                .message_id,
            "version_hint_invalid"
        );

        let invalid_comparison_hint = json_request(
            Method::POST,
            COMPARISONS_PATH,
            "127.0.0.1:8000",
            loopback,
            r#"{"baseline":{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\"}"},"candidate":{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"600000\"}","version_hint":"secret-version"}}"#,
        );
        let response = app.clone().oneshot(invalid_comparison_hint).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("secret-version"));
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&text)
                .unwrap()
                .message_id,
            "version_hint_invalid"
        );

        let invalid_shape = json_request(
            Method::POST,
            COMPARISONS_PATH,
            "127.0.0.1:8000",
            loopback,
            r#"{"baseline":{"source_kind":"json_profile","content":"{}"}}"#,
        );
        let response = app.clone().oneshot(invalid_shape).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(envelope(response).await.message_id, "json_request_invalid");

        let comparison = r#"{
          "baseline":{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\"}"},
          "candidate":{"source_kind":"text_profile","content":"secret-profile"}
        }"#;
        let response = app
            .oneshot(json_request(
                Method::POST,
                COMPARISONS_PATH,
                "127.0.0.1:8000",
                loopback,
                comparison,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert!(body["comparison"]["baseline"].get("outcome").is_some());
        assert_eq!(
            body["comparison"]["candidate"]["error"]["message_id"],
            "no_trusted_facts"
        );
        assert_eq!(body["comparison"]["reasons"][0], "candidate_failed");
    }

    #[tokio::test]
    async fn analysis_preserves_adapter_explicit_graph_evidence() {
        let app = router(state_with(ApiConfig::default()));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let content = r#"{
          "profile_version":"3.5.14",
          "operator_total_time_ns":"500000",
          "execution_topology":{
            "complete":true,
            "rootNodeIds":["scan"],
            "nodes":[
              {"nodeId":"scan","planNodeType":"SCAN","operatorProfiles":["native-scan"],"children":["aggregate"]},
              {"nodeId":"aggregate","planNodeType":"AGGREGATE","operatorProfiles":["native-aggregate"],"children":[]}
            ]
          }
        }"#;
        let body = serde_json::json!({
            "source_kind": "json_profile",
            "content": content,
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        let graph = &body["outcome"]["report"]["graph_view"];
        assert_eq!(graph["state"], "complete");
        assert_eq!(graph["topology_source"], "execution_topology");
        assert_eq!(graph["root_node_ids"], serde_json::json!(["scan"]));
        assert_eq!(graph["nodes"].as_array().map(Vec::len), Some(2));
        assert_eq!(graph["edges"].as_array().map(Vec::len), Some(1));
        assert!(
            body["outcome"]["report"]["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic["code"] != "graph_unavailable"))
        );
    }

    #[tokio::test]
    async fn export_endpoint_accepts_only_self_consistent_reports_and_redacts_source_view() {
        let app = router(state_with(ApiConfig::default()));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let analysis = serde_json::json!({
            "source_kind": "json_profile",
            "content": "{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\",\"private_payload\":\"SELECT credential FROM private_table\"}"
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                &analysis,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let analysis: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        let export = serde_json::json!({
            "kind": "analysis",
            "snapshot": analysis["outcome"]["report"].clone()
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                EXPORTS_PATH,
                "127.0.0.1:8000",
                loopback,
                &export,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[CONTENT_DISPOSITION],
            "attachment; filename=\"sr-doctor-report.json\""
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("private_payload"));
        assert!(!text.contains("credential"));
        assert!(!text.contains("private_table"));
        let artifact: doctor_contracts::ExportArtifact = from_slice(text.as_bytes()).unwrap();
        doctor_export::validate_artifact(&artifact).unwrap();

        let mut tampered: serde_json::Value = serde_json::from_str(&export).unwrap();
        tampered["snapshot"]["facts_hash"] = serde_json::json!("0".repeat(64));
        tampered["snapshot"]["source_view"]["plain_text_lines"] = serde_json::json!([{
            "line_no": 1,
            "text": "SELECT leaked_secret",
            "source_ref": "text:1"
        }]);
        let response = app
            .oneshot(json_request(
                Method::POST,
                EXPORTS_PATH,
                "127.0.0.1:8000",
                loopback,
                &tampered.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("leaked_secret"));
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&body).unwrap().code,
            "export_invalid_snapshot"
        );
    }

    #[tokio::test]
    async fn comparison_http_preserves_zero_and_bounded_percentage_semantics() {
        let app = router(state_with(ApiConfig::default()));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let exact_zero = r#"{
          "baseline":{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"0\"}"},
          "candidate":{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"1000\"}"}
        }"#;
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                COMPARISONS_PATH,
                "127.0.0.1:8000",
                loopback,
                exact_zero,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        let delta = body["comparison"]["metric_deltas"]
            .as_array()
            .unwrap()
            .iter()
            .find(|delta| delta["metric_id"] == "OperatorTotalTime")
            .unwrap();
        assert_eq!(delta["percentage_state"], "undefined_baseline_zero");
        assert_eq!(delta["absolute_delta"]["value"], "1000");

        let bounded_zero = r#"{
          "baseline":{"source_kind":"text_profile","content":"StarRocks Profile 3.5.14\nOperatorTotalTime: 0ms\n"},
          "candidate":{"source_kind":"text_profile","content":"StarRocks Profile 3.5.14\nOperatorTotalTime: 1ms\n"}
        }"#;
        let response = app
            .oneshot(json_request(
                Method::POST,
                COMPARISONS_PATH,
                "127.0.0.1:8000",
                loopback,
                bounded_zero,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        let delta = body["comparison"]["metric_deltas"]
            .as_array()
            .unwrap()
            .iter()
            .find(|delta| delta["metric_id"] == "OperatorTotalTime")
            .unwrap();
        assert_eq!(
            delta["percentage_state"],
            "undefined_baseline_interval_contains_zero"
        );
        assert!(delta["bounds"].is_object());
        assert!(delta["percentage_value"].is_null());
    }

    #[tokio::test]
    async fn multipart_imports_are_single_file_bounded_and_redacted() {
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let profile = r#"{"profile_version":"3.5.14","operator_total_time_ns":"500000"}"#;
        let app = router(state_with(ApiConfig::default()));
        let valid = multipart_body(&[
            ("profile", Some("profile.json"), profile),
            ("source_kind", None, "json_profile"),
            ("filename", None, r"C:\\private\\display-profile.json"),
        ]);
        let response = app
            .clone()
            .oneshot(multipart_request(
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                valid,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(
            body["outcome"]["report"]["input"]["filename"],
            "display-profile.json"
        );

        let upper_case_type = multipart_body(&[
            ("profile", Some("profile.json"), profile),
            ("source_kind", None, "json_profile"),
        ]);
        let mut request =
            multipart_request(ANALYSES_PATH, "127.0.0.1:8000", loopback, upper_case_type);
        request.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("MULTIPART/FORM-DATA; Boundary=doctor-test-boundary"),
        );
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let missing = multipart_body(&[("source_kind", None, "json_profile")]);
        let response = app
            .clone()
            .oneshot(multipart_request(
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                missing,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            envelope(response).await.message_id,
            "multipart_profile_missing"
        );

        let duplicate = multipart_body(&[
            ("profile", Some("one.json"), profile),
            ("profile", Some("two.json"), profile),
            ("source_kind", None, "json_profile"),
        ]);
        let response = app
            .clone()
            .oneshot(multipart_request(
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                duplicate,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            envelope(response).await.message_id,
            "multipart_field_duplicate"
        );

        let unknown = multipart_body(&[("secret_field", None, "secret-profile")]);
        let response = app
            .clone()
            .oneshot(multipart_request(
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                unknown,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("secret-profile"));
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&text)
                .unwrap()
                .message_id,
            "multipart_field_unknown"
        );

        let nested = format!(
            "--doctor-test-boundary\r\nContent-Disposition: form-data; name=\"profile\"; filename=\"nested\"\r\nContent-Type: multipart/mixed\r\n\r\n{profile}\r\n--doctor-test-boundary--\r\n"
        );
        let response = app
            .clone()
            .oneshot(multipart_request(
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                nested,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            envelope(response).await.message_id,
            "multipart_nested_disallowed"
        );

        let mut limited = ApiConfig::default();
        limited.budgets.single_input_bytes = 3;
        let limited_app = router(state_with(limited));
        let oversize = multipart_body(&[
            ("profile", Some("big.json"), profile),
            ("source_kind", None, "json_profile"),
        ]);
        let response = limited_app
            .clone()
            .oneshot(multipart_request(
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                oversize,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(envelope(response).await.code, "input_too_large");

        let comparison_oversize = multipart_body(&[
            ("baseline_profile", Some("baseline.json"), profile),
            ("baseline_source_kind", None, "json_profile"),
            ("candidate_profile", Some("candidate.json"), profile),
            ("candidate_source_kind", None, "json_profile"),
        ]);
        let response = limited_app
            .oneshot(multipart_request(
                COMPARISONS_PATH,
                "127.0.0.1:8000",
                loopback,
                comparison_oversize,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(envelope(response).await.code, "input_too_large");

        let mut independently_bounded = ApiConfig::default();
        independently_bounded.budgets.single_input_bytes = profile.len();
        let independently_bounded_app = router(state_with(independently_bounded));
        let comparison_at_two_individual_limits = multipart_body(&[
            ("baseline_profile", Some("baseline.json"), profile),
            ("baseline_source_kind", None, "json_profile"),
            ("candidate_profile", Some("candidate.json"), profile),
            ("candidate_source_kind", None, "json_profile"),
        ]);
        let response = independently_bounded_app
            .oneshot(multipart_request(
                COMPARISONS_PATH,
                "127.0.0.1:8000",
                loopback,
                comparison_at_two_individual_limits,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let comparison = multipart_body(&[
            ("baseline_profile", Some("baseline.json"), profile),
            ("baseline_source_kind", None, "json_profile"),
            ("candidate_profile", Some("candidate.txt"), "secret-profile"),
            ("candidate_source_kind", None, "text_profile"),
        ]);
        let response = app
            .oneshot(multipart_request(
                COMPARISONS_PATH,
                "127.0.0.1:8000",
                loopback,
                comparison,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(
            body["comparison"]["candidate"]["error"]["message_id"],
            "no_trusted_facts"
        );
    }

    #[test]
    fn multipart_profile_limit_rejects_before_retaining_oversize_chunk() {
        let mut retained = b"abc".to_vec();
        assert_eq!(
            append_profile_chunk(&mut retained, b"de", 4),
            Err(BudgetViolation::InputTooLarge)
        );
        assert_eq!(retained, b"abc");
    }

    #[tokio::test]
    async fn static_asset_seam_requires_a_canonical_file_under_the_configured_root() {
        let root = std::env::temp_dir().join(format!(
            "doctor-api-static-{}-{}",
            std::process::id(),
            NEXT_STATIC_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "candidate").unwrap();
        let canonical_root = root.canonicalize().unwrap();
        assert_eq!(static_asset_path(&root, "/api/v1/analyses"), None);
        assert_eq!(static_asset_path(&root, "/../secret"), None);
        assert_eq!(
            static_asset_path(&root, "/"),
            Some(canonical_root.join("index.html"))
        );

        let outside = root.with_extension("outside.html");
        std::fs::write(&outside, "outside").unwrap();
        let escaped = root.join("escape.html");
        #[cfg(windows)]
        let symlink_result = std::os::windows::fs::symlink_file(&outside, &escaped);
        #[cfg(unix)]
        let symlink_result = std::os::unix::fs::symlink(&outside, &escaped);
        if symlink_result.is_ok() {
            assert_eq!(static_asset_path(&root, "/escape.html"), None);
        }

        let config = ApiConfig {
            frontend_dist: Some(root.clone()),
            ..ApiConfig::default()
        };
        let app = router(state_with(config));
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let response = app
            .oneshot(request(Method::GET, "/", "127.0.0.1:8000", loopback))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            Bytes::from_static(b"candidate")
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(outside);
        assert_eq!(
            sanitized_filename(Some(r"C:\\private\\profile.txt".into())),
            Some("profile.txt".into())
        );
        assert_eq!(
            sanitized_filename(Some("/private/profile.txt".into())),
            Some("profile.txt".into())
        );
        assert_eq!(
            sanitized_filename(Some("C:secret-profile.txt".into())),
            None
        );
        assert_eq!(sanitized_filename(Some("\u{0}".into())), None);
    }

    #[test]
    fn trust_boundary_rejects_non_loopback_and_default_never_binds_publicly() {
        assert_eq!(ApiConfig::default().bind_addr, DEFAULT_BIND_ADDR);
        assert!(matches!(
            ApiState::with_seams(
                ApiConfig {
                    bind_addr: SocketAddr::from(([0, 0, 0, 0], 8000)),
                    ..ApiConfig::default()
                },
                "nonce",
                Arc::new(TestClock::default()),
            ),
            Err(ApiSetupError::NonLoopbackBind(_))
        ));
        assert!(matches!(
            ApiState::with_seams(
                ApiConfig {
                    bind_addr: DEFAULT_BIND_ADDR,
                    deployment_mode: DeploymentMode::DockerLoopbackPublished { public_port: 8000 },
                    ..ApiConfig::default()
                },
                "nonce",
                Arc::new(TestClock::default()),
            ),
            Err(ApiSetupError::InvalidDockerBind(_))
        ));
        assert!(matches!(
            ApiState::with_seams(
                ApiConfig {
                    bind_addr: DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR,
                    deployment_mode: DeploymentMode::DockerLoopbackPublished { public_port: 0 },
                    ..ApiConfig::default()
                },
                "nonce",
                Arc::new(TestClock::default()),
            ),
            Err(ApiSetupError::InvalidDockerPublicPort)
        ));

        let state = state_with(ApiConfig::default());
        let request = request(
            Method::GET,
            CAPABILITIES_PATH,
            "127.0.0.1:8000",
            SocketAddr::from(([10, 0, 0, 1], 1)),
        );
        assert_eq!(
            state
                .authorize(
                    &request,
                    request
                        .extensions()
                        .get::<ConnectInfo<SocketAddr>>()
                        .map(|peer| peer.0)
                )
                .unwrap_err()
                .code(),
            "non_loopback_peer"
        );

        let mut zero_concurrency = ApiConfig::default();
        zero_concurrency.budgets.concurrent_requests = 0;
        assert!(matches!(
            ApiState::with_seams(zero_concurrency, "nonce", Arc::new(TestClock::default())),
            Err(ApiSetupError::ZeroConcurrentRequests)
        ));
        let mut over_u32 = ApiConfig::default();
        over_u32.budgets.single_input_bytes = u32::MAX as usize + 1;
        assert!(matches!(
            ApiState::with_seams(over_u32, "nonce", Arc::new(TestClock::default())),
            Err(ApiSetupError::CapabilityLimitOutOfRange(
                "single_input_bytes"
            ))
        ));
    }

    #[test]
    fn budgets_enforce_all_injected_boundaries_and_release_concurrency() {
        let limits = ResourceBudgets {
            single_input_bytes: 2,
            comparison_body_bytes: 3,
            text_lines: 2,
            text_line_bytes: 2,
            json_depth: 2,
            graph_operator_nodes: 2,
            normalized_metrics: 2,
            retained_diagnostics: 2,
            analysis_wall: Duration::from_millis(10),
            comparison_wall: Duration::from_millis(20),
            concurrent_requests: 2,
        };
        let clock = Arc::new(TestClock::default());
        let guard = BudgetGuard::with_clock(limits, clock.clone());
        assert_eq!(guard.check_input_bytes(2, WorkKind::Analysis), Ok(()));
        assert_eq!(
            guard.check_input_bytes(3, WorkKind::Analysis),
            Err(BudgetViolation::InputTooLarge)
        );
        assert_eq!(guard.check_input_bytes(3, WorkKind::Comparison), Ok(()));
        assert_eq!(
            guard.check_input_bytes(4, WorkKind::Comparison),
            Err(BudgetViolation::InputTooLarge)
        );
        assert_eq!(guard.check_text("aa\nbb"), Ok(()));
        assert_eq!(
            guard.check_text("a\nb\nc"),
            Err(BudgetViolation::TooManyLines)
        );
        assert_eq!(guard.check_text("aaa"), Err(BudgetViolation::LineTooLong));
        assert_eq!(guard.check_json_depth(2), Ok(()));
        assert_eq!(guard.check_json_depth(3), Err(BudgetViolation::TooDeep));
        assert_eq!(guard.check_nodes(2), Ok(()));
        assert_eq!(guard.check_nodes(3), Err(BudgetViolation::TooManyNodes));
        assert_eq!(guard.check_metrics(2), Ok(()));
        assert_eq!(guard.check_metrics(3), Err(BudgetViolation::TooManyMetrics));
        assert_eq!(
            guard.retain_diagnostics(3),
            DiagnosticRetention {
                retained: 2,
                truncated: true
            }
        );

        let first = guard.begin(WorkKind::Analysis).unwrap();
        let second = guard.begin(WorkKind::Comparison).unwrap();
        assert!(matches!(
            guard.begin(WorkKind::Analysis),
            Err(BudgetViolation::AnalysisBusy)
        ));
        clock.set_ms(11);
        assert_eq!(
            first.check(&NeverCancelled),
            Err(BudgetViolation::AnalysisTimeout)
        );
        let cancelled = TestCancellation(AtomicBool::new(true));
        assert_eq!(
            second.check(&cancelled),
            Err(BudgetViolation::AnalysisTimeout)
        );
        drop(first);
        assert!(guard.begin(WorkKind::Analysis).is_ok());
    }

    const MINIMAL_ANALYSIS_BODY: &str = r#"{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\"}"}"#;

    async fn json_body(response: Response) -> serde_json::Value {
        from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    fn assert_budget_error(value: &serde_json::Value, code: &str, reason: &str) {
        let keys: BTreeSet<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        // An error envelope only: no outcome, report or comparison beside it.
        assert_eq!(
            keys,
            BTreeSet::from([
                "affected_side",
                "code",
                "details",
                "message_id",
                "recoverability",
                "request_id",
                "source_refs",
                "stage",
            ])
        );
        assert_eq!(value["code"], code);
        assert_eq!(value["stage"], "limits");
        assert_eq!(value["details"], serde_json::json!({ "reason": reason }));
    }

    #[tokio::test]
    async fn analyses_over_the_concurrency_limit_get_a_distinct_busy_envelope_and_recover() {
        let state = state_with(ApiConfig::default());
        let app = router(state.clone());
        let loopback = SocketAddr::from(([127, 0, 0, 1], 41000));
        let comparison = format!(
            r#"{{"baseline":{MINIMAL_ANALYSIS_BODY},"candidate":{MINIMAL_ANALYSIS_BODY}}}"#
        );
        let first = state.budgets.begin(WorkKind::Analysis).unwrap();
        let second = state.budgets.begin(WorkKind::Comparison).unwrap();
        for (path, body) in [
            (ANALYSES_PATH, MINIMAL_ANALYSIS_BODY),
            (COMPARISONS_PATH, comparison.as_str()),
        ] {
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    path,
                    "127.0.0.1:8000",
                    loopback,
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            let value = json_body(response).await;
            assert_budget_error(&value, "analysis_busy", "concurrent_request_limit_reached");
            assert_eq!(value["recoverability"], "retry_same");
        }
        assert_eq!(state.budgets.active.load(Ordering::Acquire), 2);
        drop(first);
        let response = app
            .oneshot(json_request(
                Method::POST,
                ANALYSES_PATH,
                "127.0.0.1:8000",
                loopback,
                MINIMAL_ANALYSIS_BODY,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(json_body(response).await["outcome"]["report"].is_object());
        drop(second);
        assert_eq!(state.budgets.active.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn blocking_analysis_answers_at_the_deadline_and_holds_the_slot_until_the_work_observes_the_stop()
     {
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = Duration::from_millis(50);
        config.budgets.concurrent_requests = 1;
        let state = state_with(config);
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let (poll_tx, poll_rx) = std::sync::mpsc::channel::<()>();
        let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let started = Instant::now();
        let failure = blocking_analysis(&state, &lease, move |_, _, cancellation| {
            // A long step without a poll; the deadline passes meanwhile.
            poll_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            let waiting = Instant::now();
            while !cancellation.is_cancelled() {
                if waiting.elapsed() > Duration::from_secs(3) {
                    return Ok("never stopped");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            stopped_tx.send(()).unwrap();
            // Unwinding: freeing what the analysis built.
            finish_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok("a report finished after the deadline")
        })
        .await
        .unwrap_err();
        drop(lease);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(failure.status, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(failure.code(), "analysis_timeout");
        assert_eq!(
            failure.envelope.details.get("reason").map(String::as_str),
            Some("wall_clock_budget_exceeded")
        );
        // Answered, but the work has not polled yet: it still owns the only slot.
        assert!(matches!(
            state.budgets.begin(WorkKind::Analysis),
            Err(BudgetViolation::AnalysisBusy)
        ));
        poll_tx.send(()).unwrap();
        // The work observed the stop: the slot is free while it unwinds.
        stopped_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        drop(state.budgets.begin(WorkKind::Analysis).unwrap());
        finish_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while state.budgets.active.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_completed_response_frees_the_slot_before_the_report_is_freed() {
        let mut config = ApiConfig::default();
        config.budgets.concurrent_requests = 1;
        let state = state_with(config);
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let (serialized_tx, serialized_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            blocking_analysis(&task_state, &lease, move |state, lease, cancellation| {
                let response = json_response(state, lease, cancellation, "report");
                serialized_tx.send(()).unwrap();
                // Still freeing the report.
                finish_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                response
            })
            .await
        });
        serialized_rx.await.unwrap();
        // The client goes away: the HTTP side drops its share of the lease.
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // The work has not returned yet, but its bytes are complete: the slot is free.
        drop(state.budgets.begin(WorkKind::Analysis).unwrap());
        finish_tx.send(()).unwrap();
    }

    #[tokio::test]
    async fn dropped_analysis_request_stops_the_work_and_releases_the_slot() {
        let state = state_with(ApiConfig::default());
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            blocking_analysis(&task_state, &lease, move |_, _, cancellation| {
                started_tx.send(()).unwrap();
                let waiting = Instant::now();
                while !cancellation.is_cancelled() {
                    if waiting.elapsed() > Duration::from_secs(3) {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                stopped_tx.send(()).unwrap();
                Ok(())
            })
            .await
        });
        started_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        stopped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while state.budgets.active.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    /// Cancelled from the `after`-th check on.
    struct CancelledAfter {
        after: usize,
        checks: AtomicUsize,
    }

    impl Cancellation for CancelledAfter {
        fn is_cancelled(&self) -> bool {
            self.checks.fetch_add(1, Ordering::Relaxed) + 1 >= self.after
        }
    }

    #[test]
    fn every_analysis_cancellation_point_stops_a_cancelled_or_expired_analysis() {
        let clock = Arc::new(TestClock::default());
        let state =
            ApiState::with_seams(ApiConfig::default(), "fresh-nonce", clock.clone()).unwrap();
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let request = || serde_json::from_str::<AnalysisRequest>(MINIMAL_ANALYSIS_BODY).unwrap();
        let uncancelled = CancelledAfter {
            after: usize::MAX,
            checks: AtomicUsize::new(0),
        };
        assert!(analyze_request(&state, request(), &lease, &uncancelled).is_ok());
        let points = uncancelled.checks.load(Ordering::Relaxed);
        // The 5 stage boundaries plus the step polls inside adapt and assemble.
        assert!(points > 5, "only {points} cancellation points");
        for after in 1..=points {
            let cancellation = CancelledAfter {
                after,
                checks: AtomicUsize::new(0),
            };
            let failure = analyze_request(&state, request(), &lease, &cancellation).unwrap_err();
            assert_eq!(failure.code(), "analysis_timeout", "point {after}");
            assert_eq!(failure.status, StatusCode::REQUEST_TIMEOUT, "point {after}");
            // The first raised poll stops the analysis; nothing polls again.
            assert_eq!(cancellation.checks.load(Ordering::Relaxed), after);
        }
        let cancellation = CancelledAfter {
            after: points + 1,
            checks: AtomicUsize::new(0),
        };
        assert!(analyze_request(&state, request(), &lease, &cancellation).is_ok());
        assert_eq!(cancellation.checks.load(Ordering::Relaxed), points);

        assert!(json_response(&state, &lease, &NeverCancelled, "body").is_ok());
        clock.set_ms(10_001);
        let failure = analyze_request(&state, request(), &lease, &NeverCancelled).unwrap_err();
        assert_eq!(failure.status, StatusCode::REQUEST_TIMEOUT);
        // Serialized bytes that miss the deadline are discarded as well.
        let failure = json_response(&state, &lease, &NeverCancelled, "body").unwrap_err();
        assert_eq!(failure.code(), "analysis_timeout");
    }

    #[tokio::test]
    async fn json_response_matches_axum_json_and_stops_a_large_body_mid_serialization() {
        let state = ApiState::new(ApiConfig::default()).unwrap();
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        // About 8 MiB: 32 polls of the writer.
        let body = vec!["x".repeat(1024); 8 * 1024];
        let response = json_response(&state, &lease, &NeverCancelled, &body).unwrap();
        let expected = Json(&body).into_response();
        assert_eq!(response.status(), expected.status());
        assert_eq!(
            response.headers().get(CONTENT_TYPE),
            expected.headers().get(CONTENT_TYPE)
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, serde_json::to_vec(&body).unwrap());

        let cancellation = CancelledAfter {
            after: 3,
            checks: AtomicUsize::new(0),
        };
        let failure = json_response(&state, &lease, &cancellation, &body).unwrap_err();
        assert_eq!(failure.code(), "analysis_timeout");
        // Stopped at the writer's third poll, long before the 32nd.
        assert_eq!(cancellation.checks.load(Ordering::Relaxed), 3);
    }

    fn docker_loopback_published_config(public_port: u16) -> ApiConfig {
        ApiConfig {
            bind_addr: DOCKER_LOOPBACK_PUBLISHED_BIND_ADDR,
            deployment_mode: DeploymentMode::DockerLoopbackPublished { public_port },
            ..ApiConfig::default()
        }
    }

    #[tokio::test]
    async fn docker_loopback_published_accepts_private_peer_and_exact_windows_hosts() {
        let app = router(state_with(docker_loopback_published_config(18000)));
        let private_peer = SocketAddr::from(([172, 18, 0, 2], 41000));

        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                CAPABILITIES_PATH,
                "127.0.0.1:18000",
                private_peer,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                CAPABILITIES_PATH,
                "localhost:18000",
                private_peer,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                CAPABILITIES_PATH,
                "127.0.0.1:8000",
                private_peer,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "host_not_allowed");

        let mut bad_origin = request(
            Method::GET,
            CAPABILITIES_PATH,
            "127.0.0.1:18000",
            private_peer,
        );
        bad_origin
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://localhost:18000"));
        let response = app.clone().oneshot(bad_origin).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "origin_not_allowed");

        let response = app
            .oneshot(request(
                Method::GET,
                CAPABILITIES_PATH,
                "127.0.0.1:18000",
                SocketAddr::from(([8, 8, 8, 8], 41000)),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "non_loopback_peer");
    }

    #[tokio::test]
    async fn docker_loopback_published_requires_csrf_for_every_write() {
        let app = router(state_with(docker_loopback_published_config(18000)));
        let private_peer = SocketAddr::from(([172, 18, 0, 2], 41000));
        let body = r#"{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\"}"}"#;

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                ANALYSES_PATH,
                "127.0.0.1:18000",
                private_peer,
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "csrf_invalid");

        let mut wrong_nonce = json_request(
            Method::POST,
            ANALYSES_PATH,
            "127.0.0.1:18000",
            private_peer,
            body,
        );
        wrong_nonce
            .headers_mut()
            .insert(CSRF_HEADER, HeaderValue::from_static("wrong-nonce"));
        let response = app.clone().oneshot(wrong_nonce).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope(response).await.code, "csrf_invalid");

        let mut valid_nonce = json_request(
            Method::POST,
            ANALYSES_PATH,
            "127.0.0.1:18000",
            private_peer,
            body,
        );
        valid_nonce
            .headers_mut()
            .insert(CSRF_HEADER, HeaderValue::from_static("fresh-nonce"));
        let response = app.oneshot(valid_nonce).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn network_config() -> ApiConfig {
        ApiConfig {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 8000)),
            deployment_mode: DeploymentMode::Network,
            access: Some(AccessConfig {
                token: "correct-horse-battery-staple".into(),
                secure_cookie: false,
            }),
            ..ApiConfig::default()
        }
    }

    #[test]
    fn network_mode_refuses_to_start_without_a_long_enough_token() {
        let mut missing = network_config();
        missing.access = None;
        assert!(matches!(
            ApiState::with_seams(missing, "fresh-nonce", Arc::new(TestClock::default())),
            Err(ApiSetupError::AccessTokenRequired)
        ));
        let mut short = network_config();
        short.access = Some(AccessConfig {
            token: "short".into(),
            secure_cookie: false,
        });
        assert!(matches!(
            ApiState::with_seams(short, "fresh-nonce", Arc::new(TestClock::default())),
            Err(ApiSetupError::AccessTokenTooShort)
        ));
    }

    #[tokio::test]
    async fn network_mode_serves_any_peer_only_after_the_token_login() {
        use axum::http::header::COOKIE;
        let app = router(state_with(network_config()));
        let remote_peer = SocketAddr::from(([203, 0, 113, 9], 41000));
        let host = "10.0.0.5:8000";

        // Health stays open for probes; every other API answer needs the session.
        let response = app
            .clone()
            .oneshot(request(Method::GET, HEALTH_PATH, host, remote_peer))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(request(Method::GET, CAPABILITIES_PATH, host, remote_peer))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(envelope(response).await.code, "auth_required");

        let status = app
            .clone()
            .oneshot(request(Method::GET, AUTH_SESSION_PATH, host, remote_peer))
            .await
            .unwrap();
        let body = to_bytes(status.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"required":true,"authenticated":false}"#);

        // Login must come from this origin's page and carry the right token.
        let login = |token: &str, origin: Option<&'static str>| {
            let mut login = json_request(
                Method::POST,
                AUTH_SESSION_PATH,
                host,
                remote_peer,
                &format!(r#"{{"token":"{token}"}}"#),
            );
            if let Some(origin) = origin {
                login
                    .headers_mut()
                    .insert(ORIGIN, HeaderValue::from_static(origin));
            }
            login
        };
        let response = app
            .clone()
            .oneshot(login("correct-horse-battery-staple", None))
            .await
            .unwrap();
        assert_eq!(envelope(response).await.code, "origin_not_allowed");
        let response = app
            .clone()
            .oneshot(login(
                "correct-horse-battery-staple",
                Some("http://evil.example"),
            ))
            .await
            .unwrap();
        assert_eq!(envelope(response).await.code, "origin_not_allowed");
        let response = app
            .clone()
            .oneshot(login(
                "wrong-token-wrong-token",
                Some("http://10.0.0.5:8000"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(envelope(response).await.code, "auth_invalid");
        let response = app
            .clone()
            .oneshot(login(
                "correct-horse-battery-staple",
                Some("http://10.0.0.5:8000"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let set_cookie = response.headers()[SET_COOKIE].to_str().unwrap().to_owned();
        assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
        assert!(!set_cookie.contains("correct-horse"));
        let cookie = HeaderValue::from_str(set_cookie.split(';').next().unwrap()).unwrap();

        let mut capabilities = request(Method::GET, CAPABILITIES_PATH, host, remote_peer);
        capabilities.headers_mut().insert(COOKIE, cookie.clone());
        let response = app.clone().oneshot(capabilities).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Writes still need the CSRF nonce; a session alone is not enough.
        let body = r#"{"source_kind":"json_profile","content":"{\"profile_version\":\"3.5.14\",\"operator_total_time_ns\":\"500000\"}"}"#;
        let mut write = json_request(Method::POST, ANALYSES_PATH, host, remote_peer, body);
        write.headers_mut().insert(COOKIE, cookie.clone());
        let response = app.clone().oneshot(write).await.unwrap();
        assert_eq!(envelope(response).await.code, "csrf_invalid");
        let mut write = json_request(Method::POST, ANALYSES_PATH, host, remote_peer, body);
        write.headers_mut().insert(COOKIE, cookie.clone());
        write
            .headers_mut()
            .insert(CSRF_HEADER, HeaderValue::from_static("fresh-nonce"));
        write
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("https://10.0.0.5:8000"));
        let response = app.clone().oneshot(write).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Logging out ends the session; like logging in, it must come from this origin.
        let mut logout = request(Method::DELETE, AUTH_SESSION_PATH, host, remote_peer);
        logout.headers_mut().insert(COOKIE, cookie.clone());
        logout
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static("http://10.0.0.5:8000"));
        logout
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        logout
            .headers_mut()
            .insert(CSRF_HEADER, HeaderValue::from_static("fresh-nonce"));
        let response = app.clone().oneshot(logout).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            response.headers()[SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        let mut after = request(Method::GET, CAPABILITIES_PATH, host, remote_peer);
        after.headers_mut().insert(COOKIE, cookie);
        let response = app.oneshot(after).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn docker_peer_allowlist_includes_private_and_link_local_but_not_public_addresses() {
        for peer in [
            "127.0.0.1",
            "172.18.0.2",
            "169.254.1.2",
            "::1",
            "fd00::2",
            "fe80::2",
        ] {
            assert!(docker_peer(peer.parse().unwrap()), "peer={peer}");
        }
        for peer in ["8.8.8.8", "2001:4860:4860::8888"] {
            assert!(!docker_peer(peer.parse().unwrap()), "peer={peer}");
        }
    }

    #[tokio::test]
    async fn official_profile_http_is_bound_and_never_executes_or_fetches_raw_profile() {
        for status in ["Finished", "Running", "Unknown"] {
            let mut factory = ConnectorTestFactory::connected();
            factory.profile_list_rows[0][3] = status.into();
            factory.official_result = Some(Ok(vec![
                "\u{1b}[38;2;250;128;114m<script>not HTML</script>\u{1b}[0m".into(),
                "  JOIN node 27\r".into(),
            ]));
            factory.official_expected =
                Some(("00000000-0000-0000-0000-000000003514".into(), vec![27]));
            let factory = Arc::new(factory);
            let state = connector_state(Arc::clone(&factory));
            let app = router(state.clone());
            let id = connector_connection_id(app.clone()).await;
            let discovery = state
                .sessions
                .search(
                    &id,
                    QuerySearch {
                        start_seconds: 1,
                        end_seconds: 2,
                        state: None,
                        digest: None,
                        limit: 10,
                        source: QuerySource::ProfileList,
                        statement_filter: doctor_contracts::AuditStatementFilter::All,
                    },
                )
                .unwrap();
            let body = serde_json::json!({"connection_id":id, "query_id":"00000000-0000-0000-0000-000000003514",
                "identity":{"kind":"discovery", "discovery_generation":discovery.discovery_generation}, "node_ids":[27]});
            let path = format!("{CONNECTIONS_PATH}/{id}/profiles/official-analysis");
            // Invalid bindings/filters are rejected before the official operation.
            for (field, value) in [
                ("connection_id", serde_json::json!("another-session")),
                ("query_id", serde_json::json!("not-a-uuid")),
                (
                    "identity",
                    serde_json::json!({"kind":"discovery", "discovery_generation":0}),
                ),
                ("node_ids", serde_json::json!([27, 27])),
                ("node_ids", serde_json::json!([-1])),
            ] {
                let mut invalid = body.clone();
                invalid[field] = value;
                let response = app
                    .clone()
                    .oneshot(json_request(
                        Method::POST,
                        &path,
                        "127.0.0.1:8000",
                        SocketAddr::from(([127, 0, 0, 1], 41000)),
                        &invalid.to_string(),
                    ))
                    .await
                    .unwrap();
                assert!(!response.status().is_success());
                assert_eq!(factory.official_calls.load(Ordering::Relaxed), 0);
            }
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    &path,
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &body.to_string(),
                ))
                .await
                .unwrap();
            let status_code = response.status();
            let value: serde_json::Value =
                from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
            if status == "Unknown" {
                assert_eq!(status_code, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(factory.official_calls.load(Ordering::Relaxed), 0);
            } else {
                assert_eq!(status_code, StatusCode::OK, "{value}");
                assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
                assert_eq!(value["kind"], "analyze_profile_text");
                assert_eq!(value["query"]["connection_id"], id);
                assert_eq!(value["query"]["query_id"], body["query_id"]);
                assert_eq!(value["identity"], body["identity"]);
                assert_eq!(
                    value["query"]["discovery_generation"],
                    body["identity"]["discovery_generation"]
                );
                assert_eq!(value["node_ids"], body["node_ids"]);
                let text =
                    "\u{1b}[38;2;250;128;114m<script>not HTML</script>\u{1b}[0m\n  JOIN node 27\r";
                let display_text = "<script>not HTML</script>\n  JOIN node 27\\u{000D}";
                assert_eq!(value["text"], text);
                assert_eq!(
                    value["source_hash"],
                    doctor_contracts::official_profile_text_hash(text)
                );
                assert_eq!(value["display_text"], display_text);
                assert_eq!(
                    value["display_hash"],
                    doctor_contracts::official_profile_display_hash(display_text)
                );
                assert_ne!(value["source_hash"], value["display_hash"]);
                assert_eq!(
                    value["display_normalization"],
                    "ansi_sgr_strip_controls_escape_v1"
                );
                assert!(value["profile_observed_at_utc"].is_null());
                assert!(value["read_at_utc"].as_str().unwrap().ends_with('Z'));
                assert_ne!(value["query"]["profile_fetch"]["reason"], "profile_fetched");
                if status == "Running" {
                    assert_eq!(value["query"]["profile_fetch"]["state"], "unavailable");
                }
                assert!(value.get("outcome").is_none());
            }
            assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn official_profile_http_unavailable_never_falls_back() {
        for (result, expected) in [
            (Ok(vec![]), "official_profile_empty"),
            (
                Err(RemoteError::PermissionDenied),
                "official_profile_permission_denied",
            ),
            (
                Err(RemoteError::Unsupported),
                "official_profile_remote_unsupported",
            ),
        ] {
            let mut factory = ConnectorTestFactory::connected();
            factory.official_result = Some(result);
            factory.official_expected =
                Some(("00000000-0000-0000-0000-000000003514".into(), vec![]));
            let factory = Arc::new(factory);
            let state = connector_state(Arc::clone(&factory));
            let app = router(state.clone());
            let id = connector_connection_id(app.clone()).await;
            let discovery = state
                .sessions
                .search(
                    &id,
                    QuerySearch {
                        start_seconds: 1,
                        end_seconds: 2,
                        state: None,
                        digest: None,
                        limit: 10,
                        source: QuerySource::ProfileList,
                        statement_filter: doctor_contracts::AuditStatementFilter::All,
                    },
                )
                .unwrap();
            let body = serde_json::json!({"connection_id":id, "query_id":"00000000-0000-0000-0000-000000003514",
                "identity":{"kind":"discovery", "discovery_generation":discovery.discovery_generation}, "node_ids":[]});
            let response = app
                .oneshot(json_request(
                    Method::POST,
                    &format!("{CONNECTIONS_PATH}/{id}/profiles/official-analysis"),
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &body.to_string(),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let value: serde_json::Value =
                from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
            assert!(value.to_string().contains(expected), "{value}");
            assert!(value.get("text").is_none());
            assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
            assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn official_profile_http_busy_rejects_before_remote_and_recovers_after_release() {
        let mut fixture = ConnectorTestFactory::connected();
        fixture.official_result = Some(Ok(vec!["official-text-canary".into()]));
        fixture.official_expected = Some((fixture.audit_rows[0][0].clone(), vec![27, 0]));
        let factory = Arc::new(fixture);
        let state = connector_state(Arc::clone(&factory));
        let app = router(state.clone());
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let body = serde_json::json!({
            "connection_id": connection_id,
            "query_id": queries["queries"][0]["query_id"],
            "identity": {"kind":"discovery", "discovery_generation": queries["discovery_generation"]},
            "node_ids": [27, 0],
        })
        .to_string();
        let path = format!("{CONNECTIONS_PATH}/{connection_id}/profiles/official-analysis");
        let first = state.budgets.begin(WorkKind::Analysis).unwrap();
        let second = state.budgets.begin(WorkKind::Analysis).unwrap();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(envelope(response).await.code, "analysis_busy");
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 0);
        assert!(matches!(
            state.budgets.begin(WorkKind::Analysis),
            Err(BudgetViolation::AnalysisBusy)
        ));
        drop(first);
        drop(second);
        let response = app
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
        assert!(state.budgets.begin(WorkKind::Analysis).is_ok());
    }

    #[tokio::test]
    async fn official_profile_http_timeout_discards_artifact_not_the_readable_session() {
        let clock = Arc::new(TestClock::default());
        let advance_clock = Arc::clone(&clock);
        let mut fixture = ConnectorTestFactory::connected();
        fixture.official_result = Some(Ok(vec!["official-text-canary".into()]));
        fixture.official_expected = Some((fixture.audit_rows[0][0].clone(), vec![]));
        fixture.official_hook = Some(Arc::new(move || advance_clock.set_ms(11)));
        let factory = Arc::new(fixture);
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = Duration::from_millis(10);
        let state = ApiState::with_connector_factory(config, "fresh-nonce", clock, factory.clone())
            .unwrap();
        let app = router(state.clone());
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let body = serde_json::json!({
            "connection_id": connection_id,
            "query_id": queries["queries"][0]["query_id"],
            "identity": {"kind":"discovery", "discovery_generation": queries["discovery_generation"]},
            "node_ids": [],
        })
        .to_string();
        let path = format!("{CONNECTIONS_PATH}/{connection_id}/profiles/official-analysis");
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        let value: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(value["code"], "analysis_timeout");
        for field in ["text", "source_hash", "kind", "query"] {
            assert!(value.get(field).is_none());
        }
        assert!(!value.to_string().contains("official-text-canary"));
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
        // The failed request released its permit; both slots can be reacquired.
        let first = state.budgets.begin(WorkKind::Analysis).unwrap();
        let second = state.budgets.begin(WorkKind::Analysis).unwrap();
        drop(first);
        drop(second);
        // This new lease starts at 11ms. The same generation remains readable.
        let response = app
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 2);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn official_profile_http_fatal_output_and_protocol_errors_expire_session() {
        for (result, limitation) in [
            (
                Ok(vec!["x".repeat(
                    doctor_connector::ANALYZE_PROFILE_MAX_OUTPUT_BYTES + 1,
                )]),
                Some("official_profile_output_budget_exceeded"),
            ),
            (
                Err(RemoteError::OutputBudgetExceeded),
                Some("official_profile_output_budget_exceeded"),
            ),
            (Err(RemoteError::Protocol), None),
        ] {
            let mut fixture = ConnectorTestFactory::connected();
            fixture.official_result = Some(result);
            fixture.official_expected = Some((fixture.audit_rows[0][0].clone(), vec![]));
            let factory = Arc::new(fixture);
            let state = connector_state(Arc::clone(&factory));
            let app = router(state.clone());
            let connection_id = connector_connection_id(app.clone()).await;
            let queries = observe_connector_queries(app.clone(), &connection_id).await;
            let body = serde_json::json!({
                "connection_id": connection_id,
                "query_id": queries["queries"][0]["query_id"],
                "identity": {"kind":"discovery", "discovery_generation": queries["discovery_generation"]},
                "node_ids": [],
            })
            .to_string();
            let path = format!("{CONNECTIONS_PATH}/{connection_id}/profiles/official-analysis");
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    &path,
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let error = envelope(response).await;
            assert_eq!(error.code, "connection_failed");
            assert_eq!(error.stage, "analyze_profile");
            assert_eq!(
                error.details.get("limitation").map(String::as_str),
                limitation
            );
            assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
            assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
            let response = app
                .oneshot(json_request(
                    Method::POST,
                    &path,
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::GONE);
            assert_eq!(envelope(response).await.code, "connection_expired");
            assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
            assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
            assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
            let first = state.budgets.begin(WorkKind::Analysis).unwrap();
            let second = state.budgets.begin(WorkKind::Analysis).unwrap();
            drop(first);
            drop(second);
        }
    }

    #[tokio::test]
    async fn running_profile_snapshot_http_lists_bounded_candidates_and_keeps_lifecycle_independent()
     {
        let running_id = "00000000-0000-0000-0000-000000000001";
        let mut fixture = ConnectorTestFactory::connected();
        fixture.profile = concat!(
            "StarRocks Profile 3.5.14\n",
            "QueryState: Finished\n",
            "OperatorTotalTime: 500ms\n",
            "connector-profile-secret\n",
        )
        .into();
        fixture.profile_list_rows = (0..doctor_connector::MAX_QUERY_ROWS)
            .map(|index| match index {
                0 => vec![
                    running_id.into(),
                    "2026-09-06 00:00:00".into(),
                    "1s".into(),
                    "Running".into(),
                    "SELECT connector-profile-secret FROM t".into(),
                ],
                1 => vec![
                    "00000000-0000-0000-0000-000000000002".into(),
                    "2026-09-06 00:00:00".into(),
                    "1s".into(),
                    "Running".into(),
                ],
                _ => vec![
                    format!("00000000-0000-0000-0000-{index:012x}"),
                    "2026-09-06 00:00:00".into(),
                    "1s".into(),
                    "Finished".into(),
                    "SELECT 1".into(),
                ],
            })
            .collect();
        let factory = Arc::new(fixture);
        let state = connector_state(Arc::clone(&factory));
        let app = router(state.clone());
        let connection_id = connector_connection_id(app.clone()).await;
        let candidate_path =
            format!("{CONNECTIONS_PATH}/{connection_id}/profiles/running-candidates");
        let candidate_body = serde_json::json!({"connection_id": connection_id, "limit": 25});
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &candidate_path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &candidate_body.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let candidates: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(candidates["source"], "profile_list");
        assert_eq!(candidates["limit"], 25);
        assert_eq!(candidates["returned"], 1);
        assert_eq!(candidates["at_limit"], true);
        assert_eq!(candidates["local_truncated"], false);
        assert_eq!(candidates["rejected"], 1);
        assert!(candidates["profile_observed_at_utc"].is_null());
        assert_eq!(candidates["candidates"][0]["query_id"], running_id);
        assert_eq!(candidates["candidates"][0]["status"], "Running");
        assert_eq!(
            candidates["candidates"][0]["profile_fetch"]["reason"],
            "query_not_finished"
        );
        assert!(!candidates.to_string().contains("connector-profile-secret"));
        assert_eq!(factory.profile_list_calls.load(Ordering::Relaxed), 2);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 0);

        let snapshot_path = format!("{CONNECTIONS_PATH}/{connection_id}/profiles/running-snapshot");
        let snapshot_request = serde_json::json!({
            "connection_id": connection_id,
            "query_id": running_id,
            "discovery_generation": candidates["discovery_generation"],
        });
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &snapshot_path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &snapshot_request.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(snapshot["query"]["status"], "Running");
        assert_eq!(snapshot["list_observed_state"], "Running");
        assert_eq!(snapshot["profile_lifecycle"], "finished");
        assert!(snapshot["profile_observed_at_utc"].is_null());
        assert!(
            snapshot["read_at_utc"]
                .as_str()
                .is_some_and(|value| value.ends_with('Z'))
        );
        assert_eq!(
            snapshot["query"]["profile_fetch"]["reason"],
            "query_not_finished"
        );
        assert!(!text.contains("connector-profile-secret"));
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 0);

        let raw_calls = factory.profile_calls.load(Ordering::Relaxed);
        for (query_id, generation) in [
            ("not-a-uuid", candidates["discovery_generation"].clone()),
            (running_id, serde_json::json!(u32::MAX)),
        ] {
            let invalid = serde_json::json!({
                "connection_id": connection_id,
                "query_id": query_id,
                "discovery_generation": generation,
            });
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    &snapshot_path,
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &invalid.to_string(),
                ))
                .await
                .unwrap();
            assert!(!response.status().is_success());
            assert_eq!(factory.profile_calls.load(Ordering::Relaxed), raw_calls);
        }

        let other_connection_id = connector_connection_id(app.clone()).await;
        let cross_session = serde_json::json!({
            "connection_id": other_connection_id,
            "query_id": running_id,
            "discovery_generation": candidates["discovery_generation"],
        });
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{other_connection_id}/profiles/running-snapshot"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &cross_session.to_string(),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), raw_calls);

        let mut close = request(
            Method::DELETE,
            &format!("{CONNECTIONS_PATH}/{connection_id}"),
            "127.0.0.1:8000",
            SocketAddr::from(([127, 0, 0, 1], 41000)),
        );
        close
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = app.clone().oneshot(close).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = app
            .oneshot(json_request(
                Method::POST,
                &snapshot_path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &snapshot_request.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), raw_calls);
    }

    #[tokio::test]
    async fn running_profile_snapshot_http_fails_closed_for_duplicate_unknown_empty_and_unsupported()
     {
        let running_id = "00000000-0000-0000-0000-000000000003";
        let mut duplicate = ConnectorTestFactory::connected();
        duplicate.profile_list_rows = vec![
            vec![
                running_id.into(),
                "2026-09-06 00:00:00".into(),
                "1s".into(),
                "Running".into(),
                "SELECT 1".into(),
            ],
            vec![
                running_id.to_ascii_uppercase(),
                "2026-09-06 00:00:01".into(),
                "1s".into(),
                "Running".into(),
                "SELECT 1".into(),
            ],
        ];
        let duplicate = Arc::new(duplicate);
        let duplicate_app = router(connector_state(Arc::clone(&duplicate)));
        let duplicate_connection = connector_connection_id(duplicate_app.clone()).await;
        let list = serde_json::json!({"connection_id": duplicate_connection, "limit": 25});
        let response = duplicate_app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{duplicate_connection}/profiles/running-candidates"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &list.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let list: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(list["returned"], 0);
        assert_eq!(list["rejected"], 2);
        let response = duplicate_app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{duplicate_connection}/profiles/running-snapshot"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({
                    "connection_id": duplicate_connection,
                    "query_id": running_id,
                    "discovery_generation": list["discovery_generation"],
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
        assert_eq!(duplicate.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(duplicate.runtime_calls.load(Ordering::Relaxed), 0);

        let mut locally_clipped = ConnectorTestFactory::connected();
        locally_clipped.profile_list_rows = vec![
            vec![
                "00000000-0000-0000-0000-000000000004".into(),
                "2026-09-06 00:00:00".into(),
                "1s".into(),
                "Running".into(),
                "SELECT 1".into(),
            ],
            vec![
                "00000000-0000-0000-0000-000000000005".into(),
                "2026-09-06 00:00:01".into(),
                "1s".into(),
                "Running".into(),
                "SELECT 2".into(),
            ],
        ];
        let locally_clipped = Arc::new(locally_clipped);
        let locally_clipped_app = router(connector_state(Arc::clone(&locally_clipped)));
        let locally_clipped_connection = connector_connection_id(locally_clipped_app.clone()).await;
        let response = locally_clipped_app
            .oneshot(json_request(
                Method::POST,
                &format!(
                    "{CONNECTIONS_PATH}/{locally_clipped_connection}/profiles/running-candidates"
                ),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({"connection_id": locally_clipped_connection, "limit": 1})
                    .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let locally_clipped_response: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(locally_clipped_response["returned"], 1);
        assert_eq!(locally_clipped_response["at_limit"], false);
        assert_eq!(locally_clipped_response["local_truncated"], true);
        assert_eq!(locally_clipped_response["rejected"], 0);
        assert_eq!(locally_clipped.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(locally_clipped.runtime_calls.load(Ordering::Relaxed), 0);

        for (profile, expected_lifecycle) in [
            (
                "StarRocks Profile 3.5.14\nOperatorTotalTime: 500ms\n",
                Some("unknown"),
            ),
            ("", None),
        ] {
            let mut fixture = ConnectorTestFactory::connected();
            fixture.profile = profile.into();
            fixture.profile_list_rows[0][3] = "Running".into();
            let factory = Arc::new(fixture);
            let app = router(connector_state(Arc::clone(&factory)));
            let connection_id = connector_connection_id(app.clone()).await;
            let list = serde_json::json!({"connection_id": connection_id, "limit": 25});
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/running-candidates"),
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &list.to_string(),
                ))
                .await
                .unwrap();
            let list: serde_json::Value =
                from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
            let response = app
                .oneshot(json_request(
                    Method::POST,
                    &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/running-snapshot"),
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &serde_json::json!({
                        "connection_id": connection_id,
                        "query_id": list["candidates"][0]["query_id"],
                        "discovery_generation": list["discovery_generation"],
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            if let Some(expected_lifecycle) = expected_lifecycle {
                assert_eq!(response.status(), StatusCode::OK);
                let snapshot: serde_json::Value =
                    from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
                assert_eq!(snapshot["query"]["status"], "Running");
                assert_eq!(snapshot["profile_lifecycle"], expected_lifecycle);
            } else {
                assert_eq!(response.status(), StatusCode::CONFLICT);
            }
            assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
            assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        }

        let mut unsupported = ConnectorTestFactory::connected();
        // 4.2 is outside the audited lines (3.1–4.1).
        unsupported.version = "4.2.0".into();
        let unsupported = Arc::new(unsupported);
        let unsupported_app = router(connector_state(Arc::clone(&unsupported)));
        let connection_id = connector_connection_id(unsupported_app.clone()).await;
        let response = unsupported_app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/running-candidates"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({"connection_id": connection_id, "limit": 25}).to_string(),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
        assert_eq!(unsupported.profile_list_calls.load(Ordering::Relaxed), 0);
        assert_eq!(unsupported.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(unsupported.runtime_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn connector_connection_query_fetch_projects_report_without_raw_profile() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;

        let queries = serde_json::json!({
            "connection_id": connection_id,
            "limit": 100,
            "status": "finished",
            "digest": null,
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/queries/search"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &queries,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let queries: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        let discovery_generation = queries["discovery_generation"].as_u64().unwrap();
        assert_eq!(queries["source"], "audit");
        assert_eq!(queries["queries"][0]["status"], "finished");
        assert_eq!(queries["queries"][0]["query_time_ms"], 500);
        assert_eq!(
            queries["queries"][0]["observed_at_utc"],
            "2026-08-30T00:00:00Z"
        );
        assert_eq!(
            queries["queries"][0]["observed_at_local"],
            "2026-08-30 00:00:00"
        );
        assert_eq!(
            queries["queries"][0]["timestamp_state"],
            "reader_session_interpreted"
        );
        assert_eq!(queries["queries"][0]["profile_fetch"]["state"], "available");
        assert_eq!(queries["queries"][0]["plan_check"]["state"], "available");
        assert!(queries["limitations"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item == "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch"
            })
        }));
        assert!(!queries["limitations"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item == "audit_timestamp_time_zone_unverified")
        }));
        assert!(!queries.to_string().contains("secret_canary"));

        let fetch = serde_json::json!({
            "connection_id": connection_id,
            "query_id": "00000000-0000-0000-0000-000000003515",
            "discovery_generation": discovery_generation,
            "kind": "baseline",
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/fetch"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &fetch,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("connector-profile-secret"));
        let fetch: serde_json::Value = serde_json::from_str(&body).unwrap();
        let report = &fetch["outcome"]["report"];
        assert_eq!(fetch["state"], "fetched");
        assert_eq!(fetch["input_origin"], "connected_profile_fetch");
        assert_eq!(fetch["query"]["discovery_source"], "audit");
        assert_eq!(fetch["query"]["discovery_generation"], discovery_generation);
        assert_eq!(fetch["query"]["observed_at_utc"], "2026-08-30T00:00:00Z");
        assert!(
            fetch["query"]["source_refs"]
                .as_array()
                .is_some_and(|refs| {
                    refs.iter()
                        .any(|source_ref| source_ref == "audit:row:1:timestamp")
                })
        );
        assert_eq!(fetch["source_hash"], report["input"]["source_hash"]);
        assert_eq!(fetch["analysis_key"], report["analysis_key"]);
        assert_eq!(fetch["source_kind"], report["input"]["detected_kind"]);
        assert_eq!(fetch["analysis_receipt"], report["analysis_receipt"]);
        let safe_lines = report["source_view"]["plain_text_lines"]
            .as_array()
            .unwrap();
        assert_eq!(safe_lines.len(), 2);
        assert_eq!(safe_lines[1]["text"], "OperatorTotalTime: 500ms");
        assert!(!body.contains("\"profile_text\""));
        assert_eq!(
            fetch["limitations"][0],
            "raw_connector_profile_bytes_not_returned_by_connected_fetch_response"
        );

        let candidate = serde_json::json!({
            "connection_id": connection_id,
            "query_id": "00000000-0000-0000-0000-000000003514",
            "discovery_generation": discovery_generation,
            "kind": "candidate_user_external_rerun",
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/fetch"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &candidate,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let candidate: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(
            candidate["input_origin"],
            "candidate_profile_user_external_rerun"
        );
        assert_eq!(candidate["analysis_key"], fetch["analysis_key"]);

        let mut close = request(
            Method::DELETE,
            &format!("{CONNECTIONS_PATH}/{connection_id}"),
            "127.0.0.1:8000",
            SocketAddr::from(([127, 0, 0, 1], 41000)),
        );
        close
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = app.oneshot(close).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn audit_action_and_endpoint_share_case_insensitive_finished_eligibility() {
        let mut fixture = ConnectorTestFactory::connected();
        fixture.audit_rows.truncate(1);
        fixture.audit_rows[0][3] = "eof".to_owned();
        let factory = Arc::new(fixture);
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let query = &queries["queries"][0];
        assert_eq!(query["status"], "finished");
        assert_eq!(query["profile_fetch"]["state"], "available");
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/fetch"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({
                    "connection_id": connection_id,
                    "query_id": query["query_id"],
                    "discovery_generation": queries["discovery_generation"],
                    "kind": "baseline",
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn audit_row_rejected_by_public_normalization_never_enters_endpoint_membership() {
        let mut fixture = ConnectorTestFactory::connected();
        fixture.audit_rows.truncate(1);
        let query_id = fixture.audit_rows[0][0].clone();
        fixture.audit_rows[0][1] = "bad\u{0001}timestamp".to_owned();
        let factory = Arc::new(fixture);
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        assert_eq!(queries["queries"].as_array().map(Vec::len), Some(0));
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/fetch"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({
                    "connection_id": connection_id,
                    "query_id": query_id,
                    "discovery_generation": queries["discovery_generation"],
                    "kind": "baseline",
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = envelope(response).await;
        assert_eq!(
            error.details.get("limitation").map(String::as_str),
            Some("query_id_not_observed_in_current_session")
        );
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn rejected_shape_before_valid_row_does_not_shift_fetch_provenance_locator() {
        let mut fixture = ConnectorTestFactory::connected();
        fixture.audit_rows.truncate(1);
        fixture.audit_rows.insert(0, vec!["malformed".to_owned()]);
        let factory = Arc::new(fixture);
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let query = &queries["queries"][0];
        assert_eq!(query["source_refs"][0], "audit:row:0:timestamp");

        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/fetch"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({
                    "connection_id": connection_id,
                    "query_id": query["query_id"],
                    "discovery_generation": queries["discovery_generation"],
                    "kind": "baseline",
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let fetched: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(fetched["query"]["source_refs"][0], query["source_refs"][0]);
        assert_eq!(fetched["query"]["source_refs"][1], query["source_refs"][1]);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn audit_epoch_normalization_rejects_ambiguous_zero_and_out_of_range_values() {
        assert_eq!(
            unix_seconds_rfc3339("1788048000").as_deref(),
            Some("2026-08-30T00:00:00Z")
        );
        for value in ["0", "-1", "2147483648", "not-a-number"] {
            assert_eq!(unix_seconds_rfc3339(value), None, "value={value}");
        }
    }

    #[test]
    fn query_actions_fail_closed_for_invalid_duplicate_and_non_terminal_rows() {
        let valid = "00000000-0000-0000-0000-000000003514";
        assert!(matches!(
            profile_fetch_action("not-a-uuid", "finished", false),
            ConnectedProfileFetchAction::Unavailable {
                reason: ProfileFetchUnavailableReason::QueryIdInvalid
            }
        ));
        assert!(matches!(
            profile_fetch_action(valid, "finished", true),
            ConnectedProfileFetchAction::Unavailable {
                reason: ProfileFetchUnavailableReason::QueryIdDuplicate
            }
        ));
        assert!(matches!(
            profile_fetch_action(valid, "running", false),
            ConnectedProfileFetchAction::Unavailable {
                reason: ProfileFetchUnavailableReason::QueryNotFinished
            }
        ));
        assert!(matches!(
            plan_check_action(valid, Some(&"plan-placeholder".to_owned()), true),
            ConnectedPlanCheckAction::Unavailable {
                reason: PlanCheckUnavailableReason::QueryIdDuplicate
            }
        ));
    }

    #[tokio::test]
    async fn connected_query_sql_is_explicit_bounded_and_stale_generation_is_rejected() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(factory));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let query_id = queries["queries"][0]["query_id"].as_str().unwrap();
        let discovery_generation = queries["discovery_generation"].as_u64().unwrap();
        assert!(!queries.to_string().contains("secret_canary"));

        let request_body = serde_json::json!({
            "connection_id": connection_id,
            "query_id": query_id,
            "discovery_generation": discovery_generation,
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/queries/sql"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let sql: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(sql["query_id"], query_id);
        assert_eq!(sql["discovery_generation"], discovery_generation);
        assert_eq!(sql["sql"], "SELECT secret_canary_3514 FROM t");

        let refreshed = observe_connector_queries(app.clone(), &connection_id).await;
        assert_ne!(
            refreshed["discovery_generation"].as_u64(),
            Some(discovery_generation)
        );
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/queries/sql"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &request_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let stale = envelope(response).await;
        assert_eq!(stale.code, "query_discovery_stale");
        assert_eq!(stale.message_id, "query_discovery_stale");

        let unavailable = serde_json::json!({
            "connection_id": connection_id,
            "query_id": "aaaaaaaa-0000-0000-0000-000000000000",
            "discovery_generation": refreshed["discovery_generation"],
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/queries/sql"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &unavailable,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let unavailable = envelope(response).await;
        assert_eq!(unavailable.code, "query_sql_unavailable");
        assert_eq!(unavailable.message_id, "query_sql_unavailable");
        assert_eq!(
            unavailable.details.get("limitation").map(String::as_str),
            Some("query_sql_not_retained")
        );
    }

    #[tokio::test]
    async fn connector_profile_permission_failure_never_fabricates_report() {
        let mut factory = ConnectorTestFactory::connected();
        factory.profile_error = Some(RemoteError::PermissionDenied);
        let app = router(connector_state(Arc::new(factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let discovery_generation = queries["discovery_generation"].as_u64().unwrap();
        let fetch = serde_json::json!({
            "connection_id": connection_id,
            "query_id": "00000000-0000-0000-0000-000000003514",
            "discovery_generation": discovery_generation,
            "kind": "baseline",
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/fetch"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &fetch,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("outcome"));
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&body).unwrap().code,
            "permission_denied"
        );
    }

    #[tokio::test]
    async fn connected_profile_comparison_fetches_two_server_profiles_without_explain() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let discovery_generation = queries["discovery_generation"].as_u64().unwrap();
        let comparison_request = serde_json::json!({
            "connection_id": connection_id,
            "baseline_query_id": "00000000-0000-0000-0000-000000003514",
            "candidate_query_id": "00000000-0000-0000-0000-000000003515",
            "discovery_generation": discovery_generation,
        })
        .to_string();

        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/compare"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &comparison_request,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!body.contains("connector-profile-secret"));
        assert!(!body.contains("\"profile_text\""));
        let response: ConnectedProfileComparisonResponse = from_slice(body.as_bytes()).unwrap();
        assert_eq!(
            response.baseline.input_origin,
            InputOrigin::ConnectedProfileFetch
        );
        assert_eq!(
            response.candidate.input_origin,
            InputOrigin::CandidateProfileUserExternalRerun
        );
        assert_eq!(
            response.baseline.query.query_id,
            "00000000-0000-0000-0000-000000003514"
        );
        assert_eq!(
            response.candidate.query.query_id,
            "00000000-0000-0000-0000-000000003515"
        );
        assert_eq!(
            response.comparison.comparison_key,
            doctor_core::comparison_key(&response.comparison)
        );
        for attempt in [
            &response.comparison.baseline,
            &response.comparison.candidate,
        ] {
            let AnalysisAttempt::Outcome(outcome) = attempt else {
                panic!("connected Profile comparison must contain analyzed outcomes");
            };
            assert_eq!(
                outcome.outcome.report.facts_hash,
                doctor_core::facts_hash(&doctor_core::report_facts(&outcome.outcome.report))
            );
            assert_eq!(
                outcome.outcome.report.analysis_receipt,
                v01_analysis_receipt()
            );
        }
        assert!(
            response.limitations.contains(
                &"candidate_profile_user_external_rerun_is_user_responsibility".to_owned()
            )
        );
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 2);
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn connected_profile_comparison_rejects_non_distinct_invalid_or_unexpected_input_before_fetch()
     {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let peer = SocketAddr::from(([127, 0, 0, 1], 41000));
        let valid = "00000000-0000-0000-0000-000000003514";
        let alternate = "00000000-0000-0000-0000-000000003515";

        for (path_connection_id, body, message_id) in [
            (
                connection_id.as_str(),
                serde_json::json!({
                    "connection_id": "other-connection",
                    "baseline_query_id": valid,
                    "candidate_query_id": alternate,
                    "discovery_generation": 1,
                })
                .to_string(),
                "connection_id_mismatch",
            ),
            (
                connection_id.as_str(),
                serde_json::json!({
                    "connection_id": connection_id,
                    "baseline_query_id": valid,
                    "candidate_query_id": valid,
                    "discovery_generation": 1,
                })
                .to_string(),
                "profile_query_ids_must_differ",
            ),
            (
                connection_id.as_str(),
                serde_json::json!({
                    "connection_id": connection_id,
                    "baseline_query_id": "not-a-uuid",
                    "candidate_query_id": alternate,
                    "discovery_generation": 1,
                })
                .to_string(),
                "profile_query_id_invalid",
            ),
            (
                connection_id.as_str(),
                serde_json::json!({
                    "connection_id": connection_id,
                    "baseline_query_id": "aaaaaaaa-0000-0000-0000-000000000000",
                    "candidate_query_id": "AAAAAAAA-0000-0000-0000-000000000000",
                    "discovery_generation": 1,
                })
                .to_string(),
                "profile_query_ids_must_differ",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    &format!("{CONNECTIONS_PATH}/{path_connection_id}/profiles/compare"),
                    "127.0.0.1:8000",
                    peer,
                    &body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(envelope(response).await.message_id, message_id);
        }
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/compare"),
                "127.0.0.1:8000",
                peer,
                &serde_json::json!({
                    "connection_id": connection_id,
                    "baseline_query_id": valid,
                    "candidate_query_id": alternate,
                    "discovery_generation": 1,
                    "client_report": { "forbidden": true },
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(envelope(response).await.message_id, "json_request_invalid");
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn connected_profile_comparison_applies_combined_profile_budget_before_analysis() {
        let mut factory = ConnectorTestFactory::connected();
        factory.profile.push_str(&"x".repeat(600));
        let factory = Arc::new(factory);
        let remote_factory: Arc<dyn RemoteFactory> = factory.clone();
        let mut config = ApiConfig::default();
        config.budgets.comparison_body_bytes = 300;
        let state = ApiState::with_connector_factory(
            config,
            "fresh-nonce",
            Arc::new(TestClock::default()),
            remote_factory,
        )
        .unwrap();
        let app = router(state);
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let discovery_generation = queries["discovery_generation"].as_u64().unwrap();
        let request = serde_json::json!({
            "connection_id": connection_id,
            "baseline_query_id": "00000000-0000-0000-0000-000000003514",
            "candidate_query_id": "00000000-0000-0000-0000-000000003515",
            "discovery_generation": discovery_generation,
        })
        .to_string();

        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/compare"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &request,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<ErrorEnvelope>(&body).unwrap().code,
            "input_too_large"
        );
        assert!(!body.contains("connector-profile-secret"));
        assert!(!body.contains("outcome"));
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn connected_profile_comparison_propagates_fetch_failure_without_partial_comparison() {
        let mut factory = ConnectorTestFactory::connected();
        factory.profile_error = Some(RemoteError::PermissionDenied);
        let factory = Arc::new(factory);
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let discovery_generation = queries["discovery_generation"].as_u64().unwrap();
        let request = serde_json::json!({
            "connection_id": connection_id,
            "baseline_query_id": "00000000-0000-0000-0000-000000003514",
            "candidate_query_id": "00000000-0000-0000-0000-000000003515",
            "discovery_generation": discovery_generation,
        })
        .to_string();

        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/compare"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &request,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("comparison"));
        let error: ErrorEnvelope = serde_json::from_str(&body).unwrap();
        assert_eq!(error.code, "permission_denied");
        assert_eq!(error.affected_side.as_deref(), Some("baseline"));
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn connected_profile_comparison_attributes_candidate_membership_failure() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/profiles/compare"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &serde_json::json!({
                    "connection_id": connection_id,
                    "baseline_query_id": "00000000-0000-0000-0000-000000003514",
                    "candidate_query_id": "00000000-0000-0000-0000-000000009999",
                    "discovery_generation": queries["discovery_generation"],
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = envelope(response).await;
        assert_eq!(error.affected_side.as_deref(), Some("candidate"));
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn connector_advisor_omits_raw_rows_and_explain_scopes_raw_source_with_local_sql_rejection()
     {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{CONNECTIONS_PATH}/{connection_id}/advisor-guides"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!body.contains("advisor-raw-secret"));
        let advisor: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(advisor["capability"], "available");
        assert_eq!(advisor["recommendations"], serde_json::json!([]));
        assert!(
            advisor["limitations"]
                .as_array()
                .is_some_and(|limits| limits.contains(&serde_json::json!("advisor_row_count:1")))
        );

        let explain = serde_json::json!({
            "input_kind": "user_sql",
            "connection_id": connection_id,
            "candidate_sql": "SELECT 1",
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &explain,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let explain: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            explain["snapshot"]["source_view"]["display_state"],
            "raw_text"
        );
        assert_eq!(
            explain["snapshot"]["source_view"]["lines"][0]["text"],
            factory.explain_rows[0]
        );
        let mut without_source = explain.clone();
        without_source["snapshot"]
            .as_object_mut()
            .unwrap()
            .remove("source_view");
        assert!(!without_source.to_string().contains("explain-raw-secret"));
        assert_eq!(explain["snapshot"]["explain_schema_version"], "0.4");
        assert_eq!(explain["snapshot"]["explain_level"], "costs");
        assert_eq!(
            explain["snapshot"]["graph_view"]["topology_source"],
            "explain_costs"
        );
        assert_ne!(
            explain["snapshot"]["plan_hash"],
            explain["snapshot"]["snapshot_id"]
        );

        let rejected = serde_json::json!({
            "input_kind": "user_sql",
            "connection_id": connection_id,
            "candidate_sql": "SELECT 1; DELETE FROM t",
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &rejected,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(envelope(response).await.code, "statement_not_allowed");
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn explain_modes_bind_both_inputs_without_execution_or_invalid_mode_fallback() {
        use doctor_contracts::ExplainMode;
        for (wire, mode) in [
            ("logical", ExplainMode::Logical),
            ("verbose", ExplainMode::Verbose),
            ("costs", ExplainMode::Costs),
        ] {
            let factory = Arc::new(ConnectorTestFactory {
                explain_expected_mode: Some(mode),
                explain_rows: vec![
                    "- Output => [private_column]".into(),
                    "  <script>literal source, not HTML</script>".into(),
                ],
                ..ConnectorTestFactory::connected()
            });
            let app = router(connector_state(Arc::clone(&factory)));
            let connection_id = connector_connection_id(app.clone()).await;
            let observed = observe_connector_queries(app.clone(), &connection_id).await;
            let query = &observed["queries"][0];
            let path = format!("{CONNECTIONS_PATH}/{connection_id}/explain");
            let mut identities = Vec::new();
            for body in [
                serde_json::json!({"input_kind":"user_sql","connection_id":connection_id,"candidate_sql":"SELECT 1","mode":wire}),
                serde_json::json!({"input_kind":"plan_sql_reference","connection_id":connection_id,"query_id":query["query_id"],"discovery_generation":query["discovery_generation"],"plan_sql_ref":query["plan_check"]["plan_sql_ref"],"mode":wire}),
            ] {
                let response = app
                    .clone()
                    .oneshot(json_request(
                        Method::POST,
                        &path,
                        "127.0.0.1:8000",
                        SocketAddr::from(([127, 0, 0, 1], 41000)),
                        &body.to_string(),
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let result: serde_json::Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                )
                .unwrap();
                let snapshot = &result["snapshot"];
                assert_eq!(snapshot["explain_level"], wire);
                assert_eq!(snapshot["explain_schema_version"], "0.4");
                assert_eq!(
                    snapshot["source_view"]["source_kind"],
                    format!("explain_{wire}")
                );
                assert_eq!(snapshot["runtime_state"], "unavailable");
                assert_eq!(snapshot["source_view"]["display_state"], "raw_text");
                let lines = snapshot["source_view"]["lines"].as_array().unwrap();
                assert_eq!(
                    lines
                        .iter()
                        .map(|line| line["text"].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    factory
                        .explain_rows
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                );
                let mut without_source = result.clone();
                without_source["snapshot"]
                    .as_object_mut()
                    .unwrap()
                    .remove("source_view");
                assert!(!without_source.to_string().contains("private_column"));
                assert!(!without_source.to_string().contains("<script>"));
                if wire != "costs" {
                    assert!(
                        snapshot["graph_view"]["nodes"]
                            .as_array()
                            .unwrap()
                            .is_empty()
                    );
                }
                identities.push(snapshot["snapshot_id"].clone());
            }
            assert_eq!(identities[0], identities[1]);
            for invalid in [
                serde_json::json!("analyze"),
                serde_json::json!(null),
                serde_json::json!("costs; SELECT 1"),
            ] {
                let body = serde_json::json!({"input_kind":"user_sql","connection_id":connection_id,"candidate_sql":"SELECT 1","mode":invalid});
                let response = app
                    .clone()
                    .oneshot(json_request(
                        Method::POST,
                        &path,
                        "127.0.0.1:8000",
                        SocketAddr::from(([127, 0, 0, 1], 41000)),
                        &body.to_string(),
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            }
            assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 2);
            assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
            assert_eq!(factory.official_calls.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn query_plan_reference_is_typed_session_bound_and_revoked_by_refresh() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let queries = observe_connector_queries(app.clone(), &connection_id).await;
        let query = &queries["queries"][0];
        let query_id = query["query_id"].as_str().unwrap();
        let discovery_generation = query["discovery_generation"].as_u64().unwrap();
        let plan_sql_ref = query["plan_check"]["plan_sql_ref"].as_str().unwrap();
        assert_eq!(query["plan_check"]["reason"], "statement_retained");
        let body = serde_json::json!({
            "input_kind": "plan_sql_reference",
            "connection_id": connection_id,
            "query_id": query_id,
            "discovery_generation": discovery_generation,
            "plan_sql_ref": plan_sql_ref,
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response_text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!response_text.contains("secret_canary"));
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 1);

        observe_connector_queries(app.clone(), &connection_id).await;
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = envelope(response).await;
        assert_eq!(
            error.details.get("limitation").map(String::as_str),
            Some("plan_sql_reference_expired_or_unknown")
        );
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn explain_endpoint_assembles_registered_costs_structure() {
        let mut fixture = ConnectorTestFactory::connected();
        fixture.explain_rows = vec![
            "PLAN FRAGMENT 0(F0)".to_owned(),
            "  RESULT SINK".to_owned(),
            "  0:Project".to_owned(),
            "|  confidential_attribute: raw_source_only".to_owned(),
        ];
        let factory = Arc::new(fixture);
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let body = serde_json::json!({
            "input_kind": "user_sql",
            "connection_id": connection_id,
            "candidate_sql": "WITH q AS (SELECT 1) SELECT * FROM q",
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            snapshot["snapshot"]["source_view"]["display_state"],
            "raw_text"
        );
        assert_eq!(
            snapshot["snapshot"]["source_view"]["lines"][3]["text"],
            factory.explain_rows[3]
        );
        let mut without_source = snapshot.clone();
        without_source["snapshot"]
            .as_object_mut()
            .unwrap()
            .remove("source_view");
        assert!(
            !without_source
                .to_string()
                .contains("confidential_attribute")
        );
        assert_eq!(snapshot["snapshot"]["graph_view"]["state"], "complete");
        assert_eq!(
            snapshot["snapshot"]["graph_view"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(snapshot["snapshot"]["runtime_state"], "unavailable");
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn runtime_profile_endpoint_executes_one_select_and_returns_only_analyzed_profile() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let sql_canary = "runtime_sql_must_not_cross_the_api_boundary";
        let body = serde_json::json!({
            "connection_id": connection_id,
            "candidate_sql": format!("SELECT 1 AS {sql_canary}"),
        })
        .to_string();
        let path = format!("{CONNECTIONS_PATH}/{connection_id}/runtime-profile");
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response_text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!response_text.contains(sql_canary));
        let response: serde_json::Value = serde_json::from_str(&response_text).unwrap();
        assert_eq!(response["input_origin"], "connected_sql_runtime");
        assert_eq!(
            response["query"]["query_id"],
            "33333333-3333-3333-3333-333333333333"
        );
        assert_eq!(response["state"], "fetched");
        assert!(
            response["outcome"]["report"]["operator_rows"]
                .as_array()
                .is_some_and(|operators| !operators.is_empty())
        );
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 1);

        let rejected = serde_json::json!({
            "connection_id": connection_id,
            "candidate_sql": "DELETE FROM t",
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &rejected,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(envelope(response).await.code, "statement_not_allowed");
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn runtime_sql_location_binds_only_matching_profile_identity() {
        for matching in [true, false] {
            let mut fixture = ConnectorTestFactory::connected();
            fixture.profile = include_str!(
                "../../../../fixtures/runtime/3.5.14/docker_runtime_corpus_complex.profile.txt"
            )
            .to_owned();
            if matching {
                fixture.profile = fixture.profile.replace(
                    "a5fff049-a91f-11f1-a261-f642d1535403",
                    "33333333-3333-3333-3333-333333333333",
                );
            }
            let factory = Arc::new(fixture);
            let app = router(connector_state(Arc::clone(&factory)));
            let connection_id = connector_connection_id(app.clone()).await;
            let body = serde_json::json!({"connection_id": connection_id,"candidate_sql":"SELECT * FROM runtime_corpus.sales"}).to_string();
            let response = app
                .oneshot(json_request(
                    Method::POST,
                    &format!("{CONNECTIONS_PATH}/{connection_id}/runtime-profile"),
                    "127.0.0.1:8000",
                    SocketAddr::from(([127, 0, 0, 1], 41000)),
                    &body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let value: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            let nodes = value["outcome"]["report"]["graph_view"]["nodes"]
                .as_array()
                .unwrap();
            assert_eq!(
                nodes
                    .iter()
                    .any(|n| n["sql_location"]["state"] == "located"),
                matching
            );
            if matching {
                assert!(
                    nodes
                        .iter()
                        .any(|n| n["sql_location"]["highlight"] == "runtime_corpus.sales")
                );
            }
            assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 1);
        }
    }
    #[tokio::test]
    async fn standard_execution_eligibility_reads_official_profile_once_without_rerunning() {
        let attempt = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let query_id = "33333333-3333-3333-3333-333333333333";
        let mut fixture = ConnectorTestFactory::connected();
        fixture.official_result = Some(Ok(vec!["official standard text".into()]));
        fixture.official_expected = Some((query_id.into(), vec![]));
        let factory = Arc::new(fixture);
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let runtime_path = format!("{CONNECTIONS_PATH}/{connection_id}/runtime-profile");
        let runtime_body = serde_json::json!({
            "connection_id": connection_id,
            "execution_attempt_id": attempt,
            "candidate_sql": "SELECT 1",
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &runtime_path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &runtime_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let runtime: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(runtime["query"]["query_id"], query_id);
        assert_eq!(
            runtime["official_profile_eligibility"],
            serde_json::json!({"execution_attempt_id": attempt, "method": "standard_profile"})
        );

        let identity = serde_json::json!({
            "kind": "verified_standard_attempt",
            "execution_attempt_id": attempt,
        });
        let official_path =
            format!("{CONNECTIONS_PATH}/{connection_id}/profiles/official-analysis");
        let official_body = serde_json::json!({
            "connection_id": connection_id,
            "query_id": query_id,
            "identity": identity,
            "node_ids": [],
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &official_path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &official_body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let official: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(official["identity"], identity);
        assert_eq!(official["query"]["query_id"], query_id);
        assert!(official["query"]["discovery_generation"].is_null());
        assert!(official["query"]["discovery_source"].is_null());
        assert!(
            official["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "query_status_from_verified_standard_execution")
        );
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);

        let unknown = serde_json::json!({
            "connection_id": connection_id,
            "query_id": query_id,
            "identity": {
                "kind": "verified_standard_attempt",
                "execution_attempt_id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            },
            "node_ids": [],
        })
        .to_string();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &official_path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &unknown,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.official_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn explain_endpoint_rejects_busy_analysis_slots_before_remote_call_and_releases() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let state = connector_state(Arc::clone(&factory));
        let app = router(state.clone());
        let connection_id = connector_connection_id(app.clone()).await;
        let first = state.budgets.begin(WorkKind::Analysis).unwrap();
        let second = state.budgets.begin(WorkKind::Analysis).unwrap();
        let body = serde_json::json!({
            "input_kind": "user_sql",
            "connection_id": connection_id,
            "candidate_sql": "SELECT 1",
        })
        .to_string();

        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(envelope(response).await.code, "analysis_busy");
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);

        drop(first);
        drop(second);
        assert!(state.budgets.begin(WorkKind::Analysis).is_ok());
    }

    #[tokio::test]
    async fn explain_endpoint_timeout_never_returns_success_and_releases_lease() {
        let clock = Arc::new(TestClock::default());
        let advance_clock = Arc::clone(&clock);
        let mut fixture = ConnectorTestFactory::connected();
        fixture.explain_hook = Some(Arc::new(move || advance_clock.set_ms(11)));
        let factory = Arc::new(fixture);
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = Duration::from_millis(10);
        let state = ApiState::with_connector_factory(config, "fresh-nonce", clock, factory.clone())
            .unwrap();
        let app = router(state.clone());
        let connection_id = connector_connection_id(app.clone()).await;
        let body = serde_json::json!({
            "input_kind": "user_sql",
            "connection_id": connection_id,
            "candidate_sql": "SELECT 1",
        })
        .to_string();

        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/explain"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(envelope(response).await.code, "analysis_timeout");
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 1);
        assert!(state.budgets.begin(WorkKind::Analysis).is_ok());
    }

    #[tokio::test]
    async fn connector_expiry_timer_seam_closes_idle_session() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let connector_clock = Arc::new(ConnectorTestClock::default());
        let remote_factory: Arc<dyn RemoteFactory> = factory.clone();
        let session_clock: Arc<dyn doctor_connector::Clock> = connector_clock.clone();
        let sessions = Arc::new(SessionManager::with_seams(
            remote_factory,
            session_clock,
            Duration::from_secs(1),
        ));
        let state = ApiState::with_session_manager(
            ApiConfig::default(),
            "fresh-nonce",
            Arc::new(TestClock::default()),
            Arc::clone(&sessions),
        )
        .unwrap();
        let app = router(state.clone());
        let _ = connector_connection_id(app).await;
        connector_clock.set_seconds(2);
        expire_idle_once(Arc::clone(&state.sessions)).await;
        assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn audit_user_filter_capability_projects_available_only_after_schema_probe() {
        let factory = Arc::new(
            ConnectorTestFactory::connected()
                .with_audit_user_column()
                .with_audit_statement_filter_column(),
        );
        let app = router(connector_state(factory));
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                CONNECTIONS_PATH,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                connector_request_body(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let connected: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(connected["capabilities"]["audit_user_filter"], "available");
        assert_eq!(
            connected["capabilities"]["audit_statement_filter"],
            "available"
        );
        let connection_id = connected["connection_id"].as_str().unwrap();
        let body = serde_json::json!({
            "connection_id": connection_id,
            "request": {
                "scope": {
                    "window": { "start_seconds": 1788047990, "end_seconds": 1788051600 },
                    "state": null,
                    "user": "reader",
                    "native_digest": null,
                    "catalog": null,
                    "database": null,
                    "resource_group": null, "statement_filter": "exclude_doctor_and_non_query"
                },
                "sort": "query_time_desc",
                "limit": 50
            }
        });
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/audit/occurrences"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let workload: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(workload["scope"]["user"], "reader");
        assert_eq!(
            workload["scope"]["statement_filter"],
            "exclude_doctor_and_non_query"
        );
        assert_eq!(
            workload["coverage"]["statement_filter"],
            serde_json::json!({
                "state": "applied",
                "hidden_non_query_row_count": "7",
                "hidden_doctor_row_count": "3",
            })
        );
    }
    #[tokio::test]
    async fn audit_user_filter_capability_is_projected_and_missing_column_rejects_scope() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(factory));
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                CONNECTIONS_PATH,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                connector_request_body(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let connected: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(
            connected["capabilities"]["audit_user_filter"],
            "unavailable"
        );
        assert_eq!(
            connected["capabilities"]["audit_statement_filter"],
            "unavailable"
        );
        let connection_id = connected["connection_id"].as_str().unwrap();
        let filtered = serde_json::json!({
            "connection_id": connection_id,
            "request": {
                "scope": {
                    "window": { "start_seconds": 1788047990, "end_seconds": 1788051600 },
                    "state": null, "user": null, "native_digest": null, "catalog": null,
                    "database": null, "resource_group": null,
                    "statement_filter": "exclude_doctor_and_non_query"
                },
                "sort": "query_time_desc",
                "limit": 1
            }
        });
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/audit/occurrences"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &filtered.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = envelope(response).await;
        assert_eq!(
            error.details.get("limitation").map(String::as_str),
            Some("audit_statement_filter_unavailable")
        );
        let body = serde_json::json!({
            "connection_id": connection_id,
            "request": {
                "scope": {
                    "window": { "start_seconds": 1788047990, "end_seconds": 1788051600 },
                    "state": null,
                    "user": "reader",
                    "native_digest": null,
                    "catalog": null,
                    "database": null,
                    "resource_group": null, "statement_filter": "all"
                },
                "sort": "query_time_desc",
                "limit": 1
            }
        });
        let response = app
            .oneshot(json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/audit/occurrences"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let error = envelope(response).await;
        assert_eq!(error.code, "capability_unavailable");
        assert_eq!(
            error.details.get("limitation").map(String::as_str),
            Some("audit_user_filter_unavailable")
        );
    }
    #[tokio::test]
    async fn audit_error_proportion_sort_has_typed_http_guard_and_preserves_failed_generation() {
        for (population_max, expected) in [
            (Some("3000000000000"), StatusCode::OK),
            (Some("3000000000001"), StatusCode::CONFLICT),
            (Some("184467440737095516160"), StatusCode::CONFLICT),
            (None, StatusCode::BAD_GATEWAY),
            (Some("not-a-count"), StatusCode::BAD_GATEWAY),
        ] {
            let mut factory = ConnectorTestFactory::connected();
            factory.audit_family_population_max = population_max.map(str::to_owned);
            let factory = Arc::new(factory);
            let app = router(connector_state(Arc::clone(&factory)));
            let scope = serde_json::json!({
                "window": { "start_seconds": 1788047990, "end_seconds": 1788051600 },
                "state": null, "user": null, "native_digest": null, "catalog": null,
                "database": null, "resource_group": null, "statement_filter": "all"
            });
            for suffix in ["audit/families", "audit/overview"] {
                let connection_id = connector_connection_id(app.clone()).await;
                let call = |suffix: &str, body: &serde_json::Value| {
                    json_request(
                        Method::POST,
                        &format!("{CONNECTIONS_PATH}/{connection_id}/{suffix}"),
                        "127.0.0.1:8000",
                        SocketAddr::from(([127, 0, 0, 1], 41000)),
                        &body.to_string(),
                    )
                };
                let occurrence = serde_json::json!({ "connection_id": connection_id,
                    "request": { "scope": scope, "sort": "query_time_desc", "limit": 50 } });
                let prior = app
                    .clone()
                    .oneshot(call("audit/occurrences", &occurrence))
                    .await
                    .unwrap();
                assert_eq!(prior.status(), StatusCode::OK);
                let prior: serde_json::Value =
                    from_slice(&to_bytes(prior.into_body(), usize::MAX).await.unwrap()).unwrap();
                let request = if suffix == "audit/families" {
                    serde_json::json!({ "scope": scope, "sort": "error_proportion_desc", "limit": 20 })
                } else {
                    serde_json::json!({ "scope": scope, "primary": { "state": "families", "sort": "error_proportion_desc", "limit": 20 }, "family_top_sort": "execution_count_desc" })
                };
                let result = app
                    .clone()
                    .oneshot(call(
                        suffix,
                        &serde_json::json!({ "connection_id": connection_id, "request": request }),
                    ))
                    .await
                    .unwrap();
                assert_eq!(result.status(), expected, "{suffix} / {population_max:?}");
                if expected == StatusCode::OK {
                    let body = to_bytes(result.into_body(), usize::MAX).await.unwrap();
                    let result: serde_json::Value = from_slice(&body).unwrap();
                    let rows = if suffix == "audit/families" {
                        &result["rows"]
                    } else {
                        &result["primary"]["rows"]
                    };
                    assert_eq!(
                        rows[0]["error_proportion"],
                        serde_json::json!({ "state": "exact", "numerator": "1", "denominator": "600" })
                    );
                    assert!(!String::from_utf8_lossy(&body).contains("population_max"));
                } else {
                    let failure = envelope(result).await;
                    if expected == StatusCode::CONFLICT {
                        assert_eq!(failure.code, "capability_unavailable");
                        assert_eq!(
                            failure.details.get("limitation").map(String::as_str),
                            Some("audit_family_error_proportion_population_limit_exceeded")
                        );
                    }
                    let lookup = serde_json::json!({ "connection_id": connection_id, "scope": scope,
                        "query_id": prior["rows"][0]["query"]["query_id"], "discovery_generation": prior["discovery_generation"] });
                    let sql = app
                        .clone()
                        .oneshot(call("audit/sql", &lookup))
                        .await
                        .unwrap();
                    assert_eq!(
                        sql.status(),
                        if expected == StatusCode::CONFLICT {
                            StatusCode::OK
                        } else {
                            StatusCode::GONE
                        },
                        "cap rejection preserves generation; malformed remote response discards the connection"
                    );
                }
                assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
                assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
            }
        }
    }

    fn family_http_request(id: &str, suffix: &str, body: &serde_json::Value) -> Request {
        json_request(
            Method::POST,
            &format!("{CONNECTIONS_PATH}/{id}/audit/{suffix}"),
            "127.0.0.1:8000",
            SocketAddr::from(([127, 0, 0, 1], 41000)),
            &body.to_string(),
        )
    }

    async fn discover_http_family(app: Router, id: &str) -> serde_json::Value {
        let scope = serde_json::json!({"window":{"start_seconds":1788047990,"end_seconds":1788051600},
            "state":null,"user":null,"native_digest":null,"catalog":null,"database":null,"resource_group":null,"statement_filter":"all","statement_filter":"all"});
        let body = serde_json::json!({"connection_id":id,"request":{"scope":scope,"sort":"cumulative_query_time_desc","limit":20}});
        let response = app
            .oneshot(family_http_request(id, "families", &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let family: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        serde_json::json!({"connection_id":id,"scope":scope,"discovery_generation":family["discovery_generation"],
            "family_key":family["rows"][0]["membership"]["family_key"]})
    }

    #[tokio::test]
    async fn family_detail_http_publishes_one_generation_and_reads_sql_only_on_request() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(factory.clone()));
        let id = connector_connection_id(app.clone()).await;
        let request = discover_http_family(app.clone(), &id).await;
        let list_before = factory.profile_list_calls.load(Ordering::Relaxed);
        let response = app
            .clone()
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret_canary"));
        let detail: serde_json::Value = from_slice(&bytes).unwrap();
        assert_eq!(
            detail["discovery_generation"].as_u64().unwrap(),
            request["discovery_generation"].as_u64().unwrap() + 1
        );
        assert_eq!(detail["scope"]["native_digest"], "a0b1");
        assert_eq!(detail["distribution"]["population_count"], "1");
        for role in ["recent", "slowest", "profile_known"] {
            let query = &detail["representatives"][role]["occurrence"]["query"];
            assert_eq!(
                query["discovery_generation"],
                detail["discovery_generation"]
            );
            assert_eq!(query["source_refs"][0], "audit:row:0:timestamp");
        }
        assert_eq!(
            detail["representatives"]["profile_known"]["profile_evidence"],
            "profile_list"
        );
        assert_eq!(
            factory.profile_list_calls.load(Ordering::Relaxed),
            list_before + 1
        );
        assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.audit_sql_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        assert_eq!(factory.explain_calls.load(Ordering::Relaxed), 0);
        let stale = app
            .clone()
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            factory.profile_list_calls.load(Ordering::Relaxed),
            list_before + 1
        );
        let lookup = serde_json::json!({"connection_id":id,"scope":detail["scope"],
            "discovery_generation":detail["discovery_generation"],"query_id":detail["representatives"]["recent"]["occurrence"]["query"]["query_id"]});
        let response = app
            .oneshot(family_http_request(&id, "sql", &lookup))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let sql: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(sql["sql"], "SELECT secret_canary FROM t");
        assert_eq!(factory.audit_sql_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn family_detail_http_lease_failure_evicts_and_does_not_retry_sql() {
        let clock = Arc::new(TestClock::default());
        let advance = clock.clone();
        let mut fixture = ConnectorTestFactory::connected();
        fixture.family_detail_hook = Some(Arc::new(move || {
            advance.set_ms(PAST_CLOCK_DRIVEN_ANALYSIS_WALL_MS)
        }));
        let factory = Arc::new(fixture);
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = CLOCK_DRIVEN_ANALYSIS_WALL;
        let state = ApiState::with_connector_factory(config, "fresh-nonce", clock, factory.clone())
            .unwrap();
        let app = router(state.clone());
        let id = connector_connection_id(app.clone()).await;
        let request = discover_http_family(app.clone(), &id).await;
        let response = app
            .clone()
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(envelope(response).await.code, "remote_timeout");
        assert!(state.budgets.begin(WorkKind::Analysis).is_ok());
        let response = app
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(envelope(response).await.code, "connection_expired");
        assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn family_detail_local_projection_failures_leave_original_binding_usable() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let state = connector_state(factory.clone());
        let app = router(state.clone());
        let id = connector_connection_id(app.clone()).await;
        let request = discover_http_family(app.clone(), &id).await;
        let typed: doctor_contracts::AuditFamilyDetailReadRequest =
            serde_json::from_value(request.clone()).unwrap();
        let prepared = state
            .sessions
            .prepare_audit_family_detail(&id, typed)
            .unwrap();
        for defect in 0..4 {
            let mut workload = prepared.workload().clone();
            match defect {
                0 => workload.session_identity.audit_timestamp_time_zone = None,
                1 => workload.representative_indices.clear(),
                2 => workload.selected_profile_evidence = None,
                _ => workload.source.recent = None,
            }
            assert!(audit::family_detail_response(&state, &workload).is_err());
        }
        // No failed projection is committed; a fresh HTTP request still accepts the old binding.
        let response = app
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let detail: serde_json::Value =
            from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(
            detail["discovery_generation"].as_u64().unwrap(),
            request["discovery_generation"].as_u64().unwrap() + 1
        );
        assert_eq!(
            state
                .sessions
                .commit_audit_family_detail(prepared)
                .unwrap_err()
                .code,
            ConnectorErrorCode::QueryDiscoveryStale
        );
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn family_detail_http_empty_cohort_has_no_targets_and_repeated_detail_is_valid() {
        let mut fixture = ConnectorTestFactory::connected();
        let mut row = vec![Some("summary".into())];
        row.extend(vec![Some("0".into()); 13]);
        row.extend(vec![None; 18]);
        fixture.family_detail_rows = Some(vec![row]);
        let app = router(connector_state(Arc::new(fixture)));
        let id = connector_connection_id(app.clone()).await;
        let mut request = discover_http_family(app.clone(), &id).await;
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(family_http_request(&id, "family-detail", &request))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let detail: serde_json::Value =
                from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
            assert_eq!(detail["distribution"]["population_count"], "0");
            for role in ["recent", "slowest", "profile_known"] {
                assert_eq!(detail["representatives"][role]["state"], "unavailable");
                assert_eq!(detail["representatives"][role]["reason"], "empty_cohort");
            }
            request["scope"] = detail["scope"].clone();
            request["discovery_generation"] = detail["discovery_generation"].clone();
        }
    }

    #[tokio::test]
    async fn family_detail_http_recoverable_remote_error_does_not_revoke_discovery() {
        for (error, status) in [
            (RemoteError::PermissionDenied, StatusCode::FORBIDDEN),
            (RemoteError::Unsupported, StatusCode::CONFLICT),
        ] {
            let mut fixture = ConnectorTestFactory::connected();
            fixture.family_detail_error = Some(error);
            let factory = Arc::new(fixture);
            let state = connector_state(factory.clone());
            let app = router(state.clone());
            let id = connector_connection_id(app.clone()).await;
            let body = serde_json::json!({"connection_id":id,"request":{"scope":{
                "window":{"start_seconds":1788047990,"end_seconds":1788051600},
                "state":null,"user":null,"native_digest":null,"catalog":null,"database":null,"resource_group":null,"statement_filter":"all","statement_filter":"all"},
                "sort":"query_time_desc","limit":50}});
            let response = app
                .clone()
                .oneshot(family_http_request(&id, "occurrences", &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let discovery: serde_json::Value =
                from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
            let request = serde_json::json!({"connection_id":id,"scope":body["request"]["scope"],
                "discovery_generation":discovery["discovery_generation"],"family_key":discovery["rows"][0]["family"]["family_key"]});
            let response = app
                .clone()
                .oneshot(family_http_request(&id, "family-detail", &request))
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(factory.closes.load(Ordering::Relaxed), 0);
            let lookup = serde_json::json!({"connection_id":id,"scope":request["scope"],
                "discovery_generation":request["discovery_generation"],"query_id":discovery["rows"][0]["query"]["query_id"]});
            let response = app
                .oneshot(family_http_request(&id, "sql", &lookup))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            state
                .sessions
                .fetch_profile(
                    &id,
                    discovery["rows"][0]["query"]["query_id"].as_str().unwrap(),
                    discovery["discovery_generation"].as_u64().unwrap() as u32,
                )
                .unwrap();
        }
    }

    #[tokio::test]
    async fn family_detail_http_fatal_error_requires_restart_then_returns_gone() {
        for (error, status) in [
            (RemoteError::Protocol, StatusCode::BAD_GATEWAY),
            (RemoteError::Timeout, StatusCode::GATEWAY_TIMEOUT),
        ] {
            let mut fixture = ConnectorTestFactory::connected();
            fixture.family_detail_error = Some(error);
            let factory = Arc::new(fixture);
            let app = router(connector_state(factory.clone()));
            let id = connector_connection_id(app.clone()).await;
            let request = discover_http_family(app.clone(), &id).await;
            let response = app
                .clone()
                .oneshot(family_http_request(&id, "family-detail", &request))
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(envelope(response).await.recoverability, "restart");
            assert_eq!(factory.closes.load(Ordering::Relaxed), 1);
            let response = app
                .oneshot(family_http_request(&id, "family-detail", &request))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::GONE);
            assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 1);
        }
    }

    #[tokio::test]
    async fn audit_v2_keeps_full_window_facts_and_fetches_sql_and_profile_on_demand() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(connector_state(Arc::clone(&factory)));
        let connection_id = connector_connection_id(app.clone()).await;
        let scope = serde_json::json!({
            "window": { "start_seconds": 1788047990, "end_seconds": 1788051600 },
            "state": null, "user": null, "native_digest": null, "catalog": null, "database": null, "resource_group": null, "statement_filter": "all",
        });
        let read = serde_json::json!({ "connection_id": connection_id,
            "request": { "scope": scope, "sort": "query_time_desc", "limit": 50 }
        });
        let call = |suffix: &str, body: &serde_json::Value| {
            json_request(
                Method::POST,
                &format!("{CONNECTIONS_PATH}/{connection_id}/{suffix}"),
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body.to_string(),
            )
        };
        let response = app
            .clone()
            .oneshot(call("audit/occurrences", &read))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("secret_canary"));
        assert!(!text.contains("\"sql\""));
        let result: serde_json::Value = from_slice(&bytes).unwrap();
        assert_eq!(result["schema_version"], "audit-workload.v1");
        assert_eq!(result["rows"].as_array().unwrap().len(), 2);
        assert_eq!(result["top"]["full_window_count"], "600");
        assert_eq!(result["top"]["returned_count"], 2);
        assert_eq!(result["top"]["truncated"], true);
        // Each row carries only its bounded prefix; full SQL stays on demand.
        for row in result["rows"].as_array().unwrap() {
            assert_eq!(
                row["statement_sample"],
                serde_json::json!({ "state": "text", "value": "SELECT occurrence_prefix", "truncated": true })
            );
        }
        assert_eq!(
            result["coverage"]["metric_inputs"][0]["aggregate"]["metric"]["canonical_number"]["value"],
            "300000000000"
        );
        assert_eq!(result["rows"][0]["metrics"][1]["state"], "missing");
        assert_eq!(
            result["rows"][1]["metrics"][1]["canonical_number"]["value"],
            "0"
        );
        assert_eq!(result["rows"][0]["metrics"][2]["state"], "precision_unsafe");
        assert_eq!(
            result["rows"][0]["metrics"][3]["state"],
            "negative_sentinel"
        );
        assert_eq!(result["rows"][0]["metrics"][4]["state"], "overflow");
        assert_eq!(result["rows"][0]["metrics"][6]["metric_id"], "pending_time");
        assert_eq!(
            result["rows"][0]["metrics"][6]["state"],
            "negative_sentinel"
        );
        assert_eq!(
            result["rows"][1]["metrics"][6]["canonical_number"]["value"],
            "0"
        );
        assert_eq!(result["rows"][0]["error_code"]["state"], "text");
        assert_eq!(result["rows"][0]["error_code"]["value"], "");
        assert_eq!(result["cluster_identity"]["state"], "session_scoped");
        assert_eq!(
            result["audit_timestamp_time_zone"]["effective_time_zone"],
            "Etc/UTC"
        );
        let query = &result["rows"][0]["query"];
        assert_eq!(query["plan_check"]["state"], "unavailable");
        assert_eq!(query["profile_fetch"]["state"], "available");
        assert_eq!(query["query_time_ms"], serde_json::Value::Null);
        let generation = result["discovery_generation"].clone();
        let lookup = serde_json::json!({ "connection_id": connection_id,
            "scope": scope, "query_id": query["query_id"], "discovery_generation": generation });
        let sql = app
            .clone()
            .oneshot(call("audit/sql", &lookup))
            .await
            .unwrap();
        assert_eq!(sql.status(), StatusCode::OK);
        let sql: serde_json::Value =
            from_slice(&to_bytes(sql.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(sql["sql"], "SELECT secret_canary FROM t");
        let fetch = serde_json::json!({ "connection_id": connection_id, "query_id": query["query_id"],
            "discovery_generation": generation, "kind": "baseline" });
        let profile = app
            .clone()
            .oneshot(call("profiles/fetch", &fetch))
            .await
            .unwrap();
        assert_eq!(profile.status(), StatusCode::OK);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
        assert_eq!(factory.runtime_calls.load(Ordering::Relaxed), 0);
        let family_read = serde_json::json!({ "connection_id": connection_id,
            "request": { "scope": scope, "sort": "cumulative_query_time_desc", "limit": 20 } });
        let family = app
            .clone()
            .oneshot(call("audit/families", &family_read))
            .await
            .unwrap();
        assert_eq!(family.status(), StatusCode::OK);
        let bytes = to_bytes(family.into_body(), usize::MAX).await.unwrap();
        let family: serde_json::Value = from_slice(&bytes).unwrap();
        // v0.7 design board 01: the family page carries one bounded `stmt`
        // sample of its most recent member, typed and marked when truncated.
        assert_eq!(
            family["rows"][0]["statement_sample"],
            serde_json::json!({ "state": "text", "value": "SELECT secret_canary FROM t", "truncated": false })
        );
        assert_eq!(family["rows"][0]["execution_count"], "600");
        assert_eq!(
            family["rows"][0]["membership"]["state"],
            "exact_native_digest"
        );
        assert_eq!(family["top"]["truncated"], false);
        assert_eq!(
            family["rows"][0]["aggregates"][2]["metric"]["state"],
            "missing"
        );
        // A1: digest-missing executions get product fingerprints in their own
        // list. Literals never reach the wire; truncated text is not grouped.
        let fingerprints = app
            .clone()
            .oneshot(call("audit/fingerprint-families", &family_read))
            .await
            .unwrap();
        assert_eq!(fingerprints.status(), StatusCode::OK);
        let bytes = to_bytes(fingerprints.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret_literal"));
        let fingerprints: serde_json::Value = from_slice(&bytes).unwrap();
        assert_eq!(fingerprints["authority"], "product_unofficial");
        assert_eq!(
            fingerprints["fingerprint_version"],
            "sr-doctor-fingerprint.v3"
        );
        let row = &fingerprints["rows"][0];
        assert_eq!(row["template"], "SELECT id FROM t WHERE id = ?");
        assert!(
            row["family_key"]
                .as_str()
                .unwrap()
                .starts_with("audit-fingerprint-v1:")
        );
        assert_eq!(
            (row["execution_count"].as_str(), row["error_count"].as_str()),
            (Some("2"), Some("1"))
        );
        assert_eq!(row["aggregates"][0]["metric"]["input"]["value"], "20");
        assert_eq!(row["aggregates"][0]["source_nonnegative_count"], "2");
        let coverage = &fingerprints["coverage"];
        assert_eq!(
            coverage["source"],
            serde_json::json!({ "state": "partial", "reason": "read_limit" })
        );
        assert_eq!(
            (
                coverage["eligible_event_count"].as_str(),
                coverage["read_event_count"].as_str()
            ),
            (Some("4"), Some("3"))
        );
        assert_eq!(
            (
                coverage["fingerprinted_event_count"].as_str(),
                coverage["partial_event_count"].as_str()
            ),
            (Some("2"), Some("1"))
        );
        assert_eq!(fingerprints["rows"].as_array().unwrap().len(), 1);
        // A2: the official family and the fingerprint group share a table but
        // not predicate columns; with that dimension off they form one cluster.
        let similar_read = |predicates: bool, detached: Vec<String>| {
            serde_json::json!({ "connection_id": connection_id, "request": {
                "scope": scope, "sort": "cumulative_query_time_desc", "limit": 20,
                "dimensions": { "tables": true, "join": true, "predicates": predicates },
                "detached_member_keys": detached, "refined_cluster_keys": [] } })
        };
        let similar = app
            .clone()
            .oneshot(call("audit/similar-sql", &similar_read(true, vec![])))
            .await
            .unwrap();
        assert_eq!(similar.status(), StatusCode::OK);
        let similar: serde_json::Value =
            from_slice(&to_bytes(similar.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(similar["clusters"].as_array().unwrap().len(), 0);
        assert_eq!(similar["independent"].as_array().unwrap().len(), 2);
        let similar = app
            .clone()
            .oneshot(call("audit/similar-sql", &similar_read(false, vec![])))
            .await
            .unwrap();
        let bytes = to_bytes(similar.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret_literal"));
        let similar: serde_json::Value = from_slice(&bytes).unwrap();
        let cluster = &similar["clusters"][0];
        assert_eq!(cluster["basis"]["tables"], serde_json::json!(["t"]));
        assert_eq!(cluster["execution_count"], "602");
        let sources: Vec<&str> = cluster["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|member| member["source"]["kind"].as_str().unwrap())
            .collect();
        assert_eq!(sources, ["official_digest", "product_fingerprint"]);
        assert_eq!(
            similar["coverage"]["fingerprint"]["partial_event_count"],
            "1"
        );
        let detached = cluster["members"][1]["member_key"]
            .as_str()
            .unwrap()
            .to_owned();
        let split = app
            .clone()
            .oneshot(call(
                "audit/similar-sql",
                &similar_read(false, vec![detached]),
            ))
            .await
            .unwrap();
        let split: serde_json::Value =
            from_slice(&to_bytes(split.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(split["clusters"].as_array().unwrap().len(), 0);
        assert!(
            split["independent"]
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["detached"] == true)
        );
        let mut invalid = similar_read(false, vec![]);
        invalid["request"]["dimensions"] =
            serde_json::json!({ "tables": false, "join": false, "predicates": false });
        let invalid = app
            .clone()
            .oneshot(call("audit/similar-sql", &invalid))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let stale = app
            .clone()
            .oneshot(call("audit/sql", &lookup))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let stale = app.oneshot(call("profiles/fetch", &fetch)).await.unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(factory.profile_calls.load(Ordering::Relaxed), 1);
    }

    fn history_state(factory: Arc<ConnectorTestFactory>, path: &std::path::Path) -> ApiState {
        ApiState::with_connector_factory(
            ApiConfig {
                history: HistoryConfig {
                    path: Some(path.to_owned()),
                    period_seconds: 3600,
                    slice_seconds: 300,
                    lag_seconds: 60,
                    initial_backfill_seconds: 900,
                    ..HistoryConfig::default()
                },
                ..ApiConfig::default()
            },
            "fresh-nonce",
            Arc::new(TestClock::default()),
            factory,
        )
        .unwrap()
    }

    async fn history_post(
        app: Router,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(json_request(
                Method::POST,
                path,
                "127.0.0.1:8000",
                SocketAddr::from(([127, 0, 0, 1], 41000)),
                &body.to_string(),
            ))
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            if bytes.is_empty() {
                serde_json::Value::Null
            } else {
                from_slice(&bytes).unwrap()
            },
        )
    }

    fn task21_history_dir(case: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "doctor-api-task21-{case}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn task21_history_duplicate_legacy_items_expose_manual_resolution_without_details() {
        let dir = task21_history_dir("duplicates");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("PRIVATE_PATH_SENTINEL.sqlite");
        {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                INSERT INTO meta VALUES ('schema_version','3');
                CREATE TABLE governance_items (target TEXT, template_key TEXT, state TEXT, title TEXT);
                INSERT INTO governance_items VALUES ('t','k','new','PRIVATE_SQL_SENTINEL'),('t','k','triaged','PRIVATE_SQL_SENTINEL');").unwrap();
        }
        let app = router(history_state(
            Arc::new(ConnectorTestFactory::connected()),
            &path,
        ));
        let (status, body) = history_post(
            app,
            "/api/v1/history/status",
            serde_json::json!({"connection_id":null}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["store"],
            serde_json::json!({
                "state":"unavailable", "reason":"migration_requires_manual_resolution",
                "code":"history_store_migration_requires_manual_resolution"
            })
        );
        let wire = body.to_string();
        assert!(!wire.contains("PRIVATE_"));
        assert!(!wire.contains("duplicate_active_governance_items"));
        assert_eq!(body["collection"]["state"], "stopped");
        let connection = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM governance_items WHERE state IN ('new','triaged')",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "3"
        );
        drop(connection);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn task21_history_invalid_path_exposes_io_category_without_details() {
        let dir = task21_history_dir("io");
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("PRIVATE_PATH_SENTINEL");
        std::fs::write(&blocker, b"PRIVATE_SQL_SENTINEL").unwrap();
        // A file as parent and a directory as database are portable I/O failures,
        // including on Windows where readonly directory permissions are unreliable.
        for path in [blocker.join("history.sqlite"), dir.clone()] {
            let app = router(history_state(
                Arc::new(ConnectorTestFactory::connected()),
                &path,
            ));
            let (status, body) = history_post(
                app,
                "/api/v1/history/status",
                serde_json::json!({"connection_id":null}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body["store"],
                serde_json::json!({
                    "state":"unavailable", "reason":"io_or_permission", "code":"history_store_io_or_permission"
                })
            );
            assert!(!body.to_string().contains("PRIVATE_"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn task21_history_invalid_database_exposes_other_category_without_details() {
        let dir = task21_history_dir("invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("PRIVATE_PATH_SENTINEL.sqlite");
        std::fs::write(
            &path,
            b"PRIVATE_SQL_SENTINEL: this is not a SQLite database",
        )
        .unwrap();
        let app = router(history_state(
            Arc::new(ConnectorTestFactory::connected()),
            &path,
        ));
        let (status, body) = history_post(
            app,
            "/api/v1/history/status",
            serde_json::json!({"connection_id":null}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["store"],
            serde_json::json!({
                "state":"unavailable", "reason":"other", "code":"history_store_open_failed"
            })
        );
        assert!(!body.to_string().contains("PRIVATE_"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn audit_v1_12_status_coverage_reads_all_1380_minute_slices() {
        let dir = std::env::temp_dir().join(format!("doctor-api-coverage-{}", std::process::id()));
        let path = dir.join("history.sqlite");
        let mut state = history_state(Arc::new(ConnectorTestFactory::connected()), &path);
        Arc::get_mut(&mut state.history)
            .unwrap()
            .config
            .slice_seconds = 60;
        let now = history::unix_now();
        let current = now.div_euclid(3600) * 3600;
        {
            let store = state.history.store().unwrap();
            for start in (current - 23 * 3600..current).step_by(60) {
                let run = doctor_history::RunRecord {
                    started_at: start + 61,
                    slice_start: start,
                    slice_end: start + 60,
                    state: doctor_history::RunState::Complete,
                    reason: None,
                    rows_read: 0,
                    attributed: 0,
                    partial_statements: 0,
                    unavailable_statements: 0,
                    duration_ms: 1,
                };
                store.record_run("t", &run, Some(start + 60)).unwrap();
            }
        }
        let app = router(state);
        let (code, response) = history_post(
            app.clone(),
            "/api/v1/history/status",
            serde_json::json!({"connection_id":null}),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(response["budget"]["slice_seconds"], 60);
        let coverage = response["coverage"].as_array().unwrap();
        assert_eq!(
            coverage.iter().filter(|h| h["state"] == "complete").count(),
            23,
            "{response}"
        );
        assert_eq!(coverage.last().unwrap()["state"], "not_collected");
        assert_eq!(response["runs"].as_array().unwrap().len(), 20);
        drop(app);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn audit_v1_09_archived_and_mixed_rules_keep_governance_pending_over_http() {
        let dir =
            std::env::temp_dir().join(format!("doctor-api-rule-boundary-{}", std::process::id()));
        let state = history_state(
            Arc::new(ConnectorTestFactory::connected()),
            &dir.join("history.sqlite"),
        );
        let now = history::unix_now();
        let current = now.div_euclid(3600) * 3600;
        let current_rule = doctor_core::history_rule_version();
        let mut cases = Vec::new();
        {
            let store = state.history.store().unwrap();
            for case in ["legacy", "mixed_engine", "mixed_rule", "compatible"] {
                let before_rule = if case == "legacy" {
                    "sr-doctor-fingerprint.v2"
                } else {
                    &current_rule
                };
                let mut key = String::new();
                for ago in (0..8).rev() {
                    let start = current - ago * 3600;
                    let rows: Vec<_> = (0..20)
                        .map(|n| {
                            seeded_row(
                                "SELECT id FROM t WHERE id=1",
                                format!("boundary-{ago}-{n}"),
                                start + 60,
                                if ago < 2 { 150 } else { 250 },
                                3000,
                            )
                        })
                        .collect();
                    let (templates, buckets, _) = doctor_core::aggregate_collection(case, &rows);
                    key = templates[0].template_key.clone();
                    let engine = if case == "mixed_engine" && ago == 0 {
                        "3.5.15"
                    } else {
                        "3.5.14"
                    };
                    let rule = if case == "mixed_rule" && ago == 0 {
                        "legacy-rule"
                    } else {
                        before_rule
                    };
                    let run = doctor_history::RunRecord {
                        started_at: start + 3600,
                        slice_start: start,
                        slice_end: start + 3600,
                        state: doctor_history::RunState::Complete,
                        reason: None,
                        rows_read: 20,
                        attributed: 20,
                        partial_statements: 0,
                        unavailable_statements: 0,
                        duration_ms: 1,
                    };
                    store
                        .record_slice(case, engine, rule, &run, start + 3600, &templates, &buckets)
                        .unwrap();
                }
                let item = store
                    .create_item(case, &key, "Controlled fixture", "manual", None, now)
                    .unwrap()
                    .unwrap();
                let snapshot = serde_json::json!({
                    "view": {"pinned_seconds":now, "template":"SELECT id FROM t WHERE id = ?", "window_start_seconds":current-3*3600, "window_end_seconds":current-3600,
                        "comparability":{"engine_version":"3.5.14","rule_version":before_rule}, "executions":"40","valid_samples":"40",
                        "query_time_median":null,"scan_bytes_median":null,"cpu_cost_median":null,"exemplar_query_id":null,"verdict":null},
                    "executions":40,"valid":40,"query_time_median":250,"scan_bytes_median":3000,"cpu_cost_median":null
                });
                store
                    .update_item(
                        item,
                        &doctor_history::ItemUpdate {
                            from: "new".into(),
                            to: "pending_verification".into(),
                            kind: "change_recorded".into(),
                            actor: "local_user".into(),
                            change_at: Some(current - 3600),
                            snapshot_json: Some(snapshot.to_string()),
                            ..doctor_history::ItemUpdate::default()
                        },
                        now,
                    )
                    .unwrap();
                cases.push((case, item));
                if case == "legacy" {
                    let windows = doctor_core::TrendWindows::ending_at(now, 2, 24);
                    let evaluated =
                        history_trend::evaluate(store, case, windows, windows.current_start)
                            .unwrap();
                    assert!(matches!(
                        evaluated.trends[0].1.verdict,
                        doctor_contracts::TrendVerdict::NotComparable {
                            reason: doctor_contracts::TrendNotComparableReason::RuleVersionChanged
                        }
                    ));
                }
            }
        }
        let app = router(state);
        for (case, item) in cases {
            let (code, response) = history_post(
                app.clone(),
                "/api/v1/governance/verify",
                serde_json::json!({"item_id":item.to_string()}),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "{response}");
            if case == "compatible" {
                assert_eq!(response["state"], "closed");
                assert_eq!(response["verification"]["outcome"]["state"], "improved");
            } else {
                assert_eq!(
                    response["state"], "pending_verification",
                    "{case}: {response}"
                );
                assert_eq!(
                    response["verification"]["outcome"]["state"],
                    "not_comparable"
                );
                let reason = if case == "mixed_engine" {
                    "engine_version_changed"
                } else {
                    "rule_version_changed"
                };
                assert_eq!(response["verification"]["outcome"]["reason"], reason);
            }
        }
        drop(app);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn audit_v1_10_reopening_conflict_is_an_explanatory_http_409() {
        let dir =
            std::env::temp_dir().join(format!("doctor-api-item-conflict-{}", std::process::id()));
        let state = history_state(
            Arc::new(ConnectorTestFactory::connected()),
            &dir.join("history.sqlite"),
        );
        let first;
        {
            let store = state.history.store().unwrap();
            first = store
                .create_item("t", "k", "first", "manual", None, 1)
                .unwrap()
                .unwrap();
            store
                .update_item(
                    first,
                    &doctor_history::ItemUpdate {
                        from: "new".into(),
                        to: "wont_fix".into(),
                        ..doctor_history::ItemUpdate::default()
                    },
                    2,
                )
                .unwrap();
            store
                .create_item("t", "k", "second", "manual", None, 3)
                .unwrap()
                .unwrap();
        }
        let app = router(state);
        let (code, response) = history_post(
            app.clone(),
            "/api/v1/governance/transition",
            serde_json::json!({"item_id":first.to_string(),"to":"triaged","note":null}),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(response["code"], "governance_item_already_open");
        drop(app);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn stage_b_collects_literal_free_history_and_keeps_no_credentials_after_restart() {
        let dir = std::env::temp_dir().join(format!("doctor-api-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.sqlite");
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(history_state(Arc::clone(&factory), &path));
        let connection_id = connector_connection_id(app.clone()).await;
        // The first cycle runs right after connecting: backfill of 3 slices.
        let mut status = serde_json::Value::Null;
        for _ in 0..100 {
            let (_, body) = history_post(
                app.clone(),
                "/api/v1/history/status",
                serde_json::json!({ "connection_id": connection_id }),
            )
            .await;
            status = body;
            if status["runs"]
                .as_array()
                .is_some_and(|runs| runs.len() >= 3)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(status["collection"]["state"], "running");
        assert_eq!(status["target"], "127.0.0.1:9030");
        assert_eq!(status["runs"].as_array().unwrap().len(), 3);
        assert!(
            status["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|run| run["state"] == "complete")
        );
        assert_eq!(status["store"]["templates"], "1");
        assert_eq!(status["budget"]["production_budget_measured"], false);
        let watermark = status["watermark_seconds"].as_u64().unwrap();
        // One collector per process: a second session to the same target waits.
        let second = connector_connection_id(app.clone()).await;
        let (_, waiting) = history_post(
            app.clone(),
            "/api/v1/history/status",
            serde_json::json!({ "connection_id": second }),
        )
        .await;
        assert_eq!(waiting["collection"]["state"], "stopped");
        assert_eq!(
            waiting["collection"]["reason"],
            "another_session_collecting"
        );
        // Pause is persisted with the target and survives a restart.
        let (code, _) = history_post(
            app.clone(),
            "/api/v1/history/collection",
            serde_json::json!({ "connection_id": connection_id, "action": "pause" }),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        let (_, paused) = history_post(
            app.clone(),
            "/api/v1/history/status",
            serde_json::json!({ "connection_id": connection_id }),
        )
        .await;
        assert_eq!(paused["collection"]["state"], "paused");
        // Closing the collecting session hands collection to the waiting one,
        // which keeps the persisted pause.
        let mut close = request(
            Method::DELETE,
            &format!("{CONNECTIONS_PATH}/{connection_id}"),
            "127.0.0.1:8000",
            SocketAddr::from(([127, 0, 0, 1], 41000)),
        );
        close
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert_eq!(
            app.clone().oneshot(close).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let mut handed = serde_json::Value::Null;
        for _ in 0..100 {
            (_, handed) = history_post(
                app.clone(),
                "/api/v1/history/status",
                serde_json::json!({ "connection_id": second }),
            )
            .await;
            if handed["collection"]["state"] == "paused" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(handed["collection"]["state"], "paused");
        assert_eq!(handed["watermark_seconds"].as_u64(), Some(watermark));
        drop(app);
        // RB-03 / RB-05: no literal and no credential anywhere on disk.
        let mut bytes = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            bytes.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        for needle in [&b"history_secret_literal"[..], b"write-only"] {
            assert!(!bytes.windows(needle.len()).any(|window| window == needle));
        }
        // A restarted process has no session: collection waits for a reconnect
        // and the watermark is kept.
        let restarted = router(history_state(
            Arc::new(ConnectorTestFactory::connected()),
            &path,
        ));
        let (_, after) = history_post(
            restarted.clone(),
            "/api/v1/history/status",
            serde_json::json!({ "connection_id": null }),
        )
        .await;
        assert_eq!(
            after["collection"],
            serde_json::json!({ "state": "stopped", "reason": "not_connected" })
        );
        assert_eq!(after["store"]["templates"], "1");
        let (code, _) = history_post(
            restarted.clone(),
            "/api/v1/history/clear",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        assert!(watermark > 0);
        drop(restarted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stage_b_never_collects_a_remote_cluster_before_g1() {
        let dir =
            std::env::temp_dir().join(format!("doctor-api-history-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = history_state(
            Arc::new(ConnectorTestFactory::connected()),
            &dir.join("history.sqlite"),
        );
        history::start_collector(
            &state,
            "remote-connection",
            "starrocks.example.internal",
            9030,
            true,
        );
        let app = router(state);
        let (_, status) = history_post(
            app,
            "/api/v1/history/status",
            serde_json::json!({ "connection_id": "remote-connection" }),
        )
        .await;
        assert_eq!(
            status["collection"],
            serde_json::json!({ "state": "stopped", "reason": "production_gate" })
        );
        assert_eq!(status["remote_collection_enabled"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seeds one hour slice per hour: `rows(hours_ago)` gives that hour's
    /// executions; `engine` is the version recorded with the slice.
    fn seed_history(
        path: &std::path::Path,
        now: i64,
        hours: std::ops::RangeInclusive<i64>,
        engine: impl Fn(i64) -> &'static str,
        rows: impl Fn(i64, i64) -> Vec<doctor_core::CollectedExecution>,
    ) {
        let store = doctor_history::HistoryStore::open(path).unwrap();
        let current = now.div_euclid(3600) * 3600;
        for ago in hours.rev() {
            let start = current - ago * 3600;
            let (templates, buckets, _) =
                doctor_core::aggregate_collection("127.0.0.1:9030", &rows(ago, start));
            let run = doctor_history::RunRecord {
                started_at: start + 3600,
                slice_start: start,
                slice_end: start + 3600,
                state: doctor_history::RunState::Complete,
                reason: None,
                rows_read: 0,
                attributed: 0,
                partial_statements: 0,
                unavailable_statements: 0,
                duration_ms: 1,
            };
            assert!(
                store
                    .record_slice(
                        "127.0.0.1:9030",
                        engine(ago),
                        &doctor_core::history_rule_version(),
                        &run,
                        start + 3600,
                        &templates,
                        &buckets
                    )
                    .unwrap()
            );
        }
    }

    fn seeded_row(
        sql: &str,
        id: String,
        epoch: i64,
        millis: u64,
        scan: u64,
    ) -> doctor_core::CollectedExecution {
        doctor_core::CollectedExecution {
            query_id: Some(id),
            catalog: Some("default_catalog".into()),
            database: Some("demo".into()),
            native_digest: None,
            statement: doctor_core::AuditFingerprintStatement::Text {
                value: sql.into(),
                truncated: false,
            },
            error: false,
            event_epoch_seconds: Some(epoch),
            metrics: vec![
                (
                    doctor_contracts::AuditMetricId::QueryTime,
                    doctor_contracts::AuditMetricInput::from_sql_cell(Some(millis.to_string())),
                ),
                (
                    doctor_contracts::AuditMetricId::ScanBytes,
                    doctor_contracts::AuditMetricInput::from_sql_cell(Some(scan.to_string())),
                ),
            ],
        }
    }

    #[tokio::test]
    async fn stage_c_trends_score_only_comparable_baselines_and_never_without_one() {
        let dir = std::env::temp_dir().join(format!("doctor-api-trends-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.sqlite");
        let now = history::unix_now();
        // Hours 26..=49 ago were recorded under 3.5.13, the rest under 3.5.14.
        seed_history(
            &path,
            now,
            0..=49,
            |ago| if ago >= 26 { "3.5.13" } else { "3.5.14" },
            |ago, start| {
                let mut rows = Vec::new();
                for n in 0..20u64 {
                    let id = |t: &str| format!("{t}-{ago}-{n}");
                    if ago <= 1 {
                        rows.push(seeded_row(
                            "SELECT a FROM t WHERE id = 1",
                            id("a"),
                            start + 60,
                            250 + n,
                            3_000,
                        ));
                        rows.push(seeded_row(
                            "SELECT b FROM t WHERE id = 1",
                            id("b"),
                            start + 60,
                            100,
                            1_000,
                        ));
                        rows.push(seeded_row(
                            "SELECT c FROM t WHERE id = 1",
                            id("c"),
                            start + 60,
                            100,
                            1_000,
                        ));
                    } else if n < 5 && ago <= 25 {
                        rows.push(seeded_row(
                            "SELECT a FROM t WHERE id = 1",
                            id("a"),
                            start + 60,
                            96 + n,
                            1_000,
                        ));
                    } else if n < 5 {
                        rows.push(seeded_row(
                            "SELECT b FROM t WHERE id = 1",
                            id("b"),
                            start + 60,
                            100,
                            1_000,
                        ));
                    }
                }
                rows
            },
        );
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(history_state(factory, &path));
        let (code, trends) = history_post(
            app.clone(),
            "/api/v1/history/trends",
            serde_json::json!({ "target": null, "current_hours": 2, "baseline_hours": 48, "limit": 50 }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{trends}");
        assert_eq!(trends["target"], "127.0.0.1:9030");
        assert_eq!(trends["rule"]["calibrated"], false);
        let verdicts: BTreeMap<String, serde_json::Value> = trends["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["template"].as_str().unwrap().to_owned(),
                    row["verdict"].clone(),
                )
            })
            .collect();
        assert_eq!(
            verdicts["SELECT a FROM t WHERE id = ?"],
            serde_json::json!({ "state": "regression", "severity": "high", "workload": "increased" })
        );
        assert_eq!(
            verdicts["SELECT b FROM t WHERE id = ?"],
            serde_json::json!({ "state": "not_comparable", "reason": "engine_version_changed" })
        );
        assert_eq!(
            verdicts["SELECT c FROM t WHERE id = ?"],
            serde_json::json!({ "state": "unscored", "reason": "insufficient_baseline" })
        );
        assert_eq!(
            trends["rows"][0]["template"],
            "SELECT a FROM t WHERE id = ?"
        );
        let regression = &trends["rows"][0];
        assert_eq!(regression["baseline"]["median"]["canonical_unit"], "ns");
        assert_eq!(regression["evidence"], "complete");
        assert!(
            regression["change"]["relative_percent"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
                > 100.0
        );
        let unscored = trends["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["verdict"]["state"] == "unscored")
            .unwrap();
        assert_eq!(
            (unscored["confidence"].clone(), unscored["change"].clone()),
            (serde_json::Value::Null, serde_json::Value::Null)
        );

        let key = regression["template_key"].as_str().unwrap();
        let (code, series) = history_post(
            app.clone(),
            "/api/v1/history/series",
            serde_json::json!({ "target": "127.0.0.1:9030", "template_key": key, "current_hours": 2, "baseline_hours": 48, "points": 48 }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{series}");
        let points = series["points"].as_array().unwrap();
        assert_eq!(points.len(), 48);
        assert!(points.iter().all(|point| point["coverage"] == "complete"));
        // The slowest kept executions of the current window, by query ID only.
        let exemplars = series["exemplars"].as_array().unwrap();
        assert_eq!(exemplars.len(), 5);
        assert!(exemplars[0]["query_id"].as_str().unwrap().starts_with("a-"));
        assert_eq!(
            exemplars[0]["query_time"]["canonical_number"]["value"],
            "269000000"
        );

        // Not connected: status shows the most recently collected target.
        let (_, status) = history_post(
            app.clone(),
            "/api/v1/history/status",
            serde_json::json!({ "connection_id": null }),
        )
        .await;
        assert_eq!(status["target"], "127.0.0.1:9030");
        assert_eq!(status["collection"]["reason"], "not_connected");
        assert!(status["watermark_seconds"].as_u64().is_some());

        for bad in [
            serde_json::json!({ "target": null, "current_hours": 0, "baseline_hours": 48, "limit": 50 }),
            serde_json::json!({ "target": null, "current_hours": 2, "baseline_hours": 12, "limit": 50 }),
            serde_json::json!({ "target": "a b", "current_hours": 2, "baseline_hours": 48, "limit": 50 }),
        ] {
            assert_eq!(
                history_post(app.clone(), "/api/v1/history/trends", bad)
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
        let (code, _) = history_post(
            app.clone(),
            "/api/v1/history/series",
            serde_json::json!({ "target": "127.0.0.1:9030", "template_key": "history-template-v1:none", "current_hours": 2, "baseline_hours": 48, "points": 48 }),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn audit_v2_routes_retain_trust_shape_connection_and_work_budget_gates() {
        let factory = Arc::new(ConnectorTestFactory::connected());
        let state = connector_state(factory);
        let app = router(state.clone());
        let connection_id = connector_connection_id(app.clone()).await;
        let body = serde_json::json!({ "connection_id": connection_id,
            "request": { "scope": { "window": { "start_seconds": 1, "end_seconds": 2 },
                "state": null, "native_digest": null, "catalog": null, "database": null, "resource_group": null, "statement_filter": "all" },
                "sort": "query_time_desc", "limit": 1 } });
        let path = format!("{CONNECTIONS_PATH}/{connection_id}/audit/occurrences");
        let peer = SocketAddr::from(([127, 0, 0, 1], 41000));
        for suffix in [
            "occurrences",
            "families",
            "overview",
            "active-at",
            "family-detail",
            "fingerprint-families",
            "similar-sql",
            "sql",
        ] {
            let route = format!("{CONNECTIONS_PATH}/{connection_id}/audit/{suffix}");
            let mut req = json_request(Method::POST, &route, "127.0.0.1:8000", peer, "{}");
            req.headers_mut()
                .insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:8000"));
            let response = app.clone().oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(envelope(response).await.code, "csrf_invalid");
        }
        let mut bad = body.clone();
        bad["connection_id"] = "other".into();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                peer,
                &bad.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            envelope(response).await.message_id,
            "connection_id_mismatch"
        );
        let mut bad = body.clone();
        bad["request"]["order_by"] = "stmt".into();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                peer,
                &bad.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let mut bad = body.clone();
        bad["request"]["limit"] = 501.into();
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                peer,
                &bad.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let _one = state.budgets.begin(WorkKind::Analysis).unwrap();
        let _two = state.budgets.begin(WorkKind::Analysis).unwrap();
        let response = app
            .oneshot(json_request(
                Method::POST,
                &path,
                "127.0.0.1:8000",
                peer,
                &body.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn connection_expired_failure_is_http_410_restart_envelope() {
        let state = state_with(ApiConfig::default());
        let failure = connector_failure(
            &state,
            ConnectorError {
                code: ConnectorErrorCode::ConnectionExpired,
                stage: "session",
                limitation: None,
                affected_side: None,
            },
        );
        assert_eq!(failure.status, StatusCode::GONE);
        assert_eq!(failure.envelope.code, "connection_expired");
        assert_eq!(failure.envelope.message_id, "connection_expired");
        assert_eq!(failure.envelope.recoverability, "restart");
    }
    #[tokio::test]
    async fn audit_operation_drop_keeps_lease_until_blocking_worker_exits() {
        let state = state_with(ApiConfig::default());
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            audit_connector_call(
                &task_state,
                lease,
                "missing-test-session".into(),
                move |_, control, _| {
                    started_tx.send(Arc::downgrade(control)).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    assert!(control.check().is_err());
                    Ok(())
                },
            )
            .await
        });
        let weak = started_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(state.budgets.active.load(Ordering::Acquire), 1);
        assert!(weak.upgrade().unwrap().check().is_err());
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while state.budgets.active.load(Ordering::Acquire) != 0 || weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn audit_operation_timer_stops_worker_and_exits_without_handler_drop() {
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = Duration::from_millis(30);
        let state = state_with(config);
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let (weak_tx, weak_rx) = tokio::sync::oneshot::channel();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            audit_connector_call(
                &state,
                lease,
                "missing-test-session".into(),
                move |_, control, _| {
                    weak_tx.send(Arc::downgrade(control)).unwrap();
                    // Observe the independent timer, not check()'s synchronous deadline fallback.
                    while !control.is_stopped() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(())
                },
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(result.envelope.code, "analysis_timeout");
        assert_eq!(result.envelope.recoverability, "reduce_input");
        assert_eq!(state.budgets.active.load(Ordering::Acquire), 0);
        let weak = weak_rx.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn audit_operation_panicking_worker_releases_lease_and_timer() {
        let state = state_with(ApiConfig::default());
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        let (weak_tx, weak_rx) = tokio::sync::oneshot::channel();
        let failure = audit_connector_call::<()>(
            &state,
            lease,
            "missing-test-session".into(),
            move |_, control, _| {
                weak_tx.send(Arc::downgrade(control)).unwrap();
                panic!("synthetic worker panic");
            },
        )
        .await
        .unwrap_err();
        assert_eq!(failure.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(state.budgets.active.load(Ordering::Acquire), 0);
        let weak = weak_rx.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn audit_family_panicking_owned_worker_evicts_before_next_request() {
        let mut fixture = ConnectorTestFactory::connected();
        fixture.family_detail_hook = Some(Arc::new(|| panic!("synthetic owned worker panic")));
        let factory = Arc::new(fixture);
        let state = connector_state(factory.clone());
        let app = router(state.clone());
        let id = connector_connection_id(app.clone()).await;
        let request = discover_http_family(app.clone(), &id).await;
        let response = app
            .clone()
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(state.budgets.active.load(Ordering::Acquire), 0);
        let response = app
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn audit_expired_queued_request_returns_408_and_preserves_session() {
        let clock = Arc::new(TestClock::default());
        let factory = Arc::new(ConnectorTestFactory::connected());
        let mut config = ApiConfig::default();
        config.budgets.analysis_wall = CLOCK_DRIVEN_ANALYSIS_WALL;
        let state =
            ApiState::with_connector_factory(config, "fresh-nonce", clock.clone(), factory.clone())
                .unwrap();
        let app = router(state.clone());
        let id = connector_connection_id(app.clone()).await;
        let request = discover_http_family(app.clone(), &id).await;
        let typed: doctor_contracts::AuditFamilyDetailReadRequest =
            serde_json::from_value(request.clone()).unwrap();
        let lease = state.budgets.begin(WorkKind::Analysis).unwrap();
        clock.set_ms(PAST_CLOCK_DRIVEN_ANALYSIS_WALL_MS);
        let worker_id = id.clone();
        let result = audit_connector_call(
            &state,
            lease,
            id.clone(),
            move |sessions, control, state| {
                sessions
                    .prepare_audit_family_detail_controlled(&worker_id, typed, Some(control))
                    .map(|_| ())
                    .map_err(|error| connector_failure(state, error))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(result.status, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(result.envelope.code, "analysis_timeout");
        assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 0);
        let response = app
            .oneshot(family_http_request(&id, "family-detail", &request))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(factory.family_detail_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn stage_d_alerts_dedupe_and_the_comparator_alone_closes_an_item() {
        let dir =
            std::env::temp_dir().join(format!("doctor-api-governance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.sqlite");
        let now = history::unix_now();
        let current = now.div_euclid(3600) * 3600;
        // Template a: a regression in the last two hours (RD-01).
        // Template g: slow for a day, then fast in the last three hours.
        seed_history(
            &path,
            now,
            3..=49,
            |_| "3.5.14",
            |ago, start| {
                let mut rows = Vec::new();
                for n in 0..20u64 {
                    if n < 5 {
                        rows.push(seeded_row(
                            "SELECT a FROM t WHERE id = 1",
                            format!("a-{ago}-{n}"),
                            start + 60,
                            96 + n,
                            1_000,
                        ));
                    }
                    if ago <= 26 {
                        rows.push(seeded_row(
                            "SELECT g FROM t WHERE name = 'governance_secret_literal'",
                            format!("g-{ago}-{n}"),
                            start + 60,
                            250 + n % 3,
                            3_000,
                        ));
                    }
                }
                rows
            },
        );
        let regressing = |ago: i64, start: i64| -> Vec<doctor_core::CollectedExecution> {
            (0..20u64)
                .map(|n| {
                    seeded_row(
                        "SELECT a FROM t WHERE id = 1",
                        format!("a-{ago}-{n}"),
                        start + 60,
                        250 + n,
                        3_000,
                    )
                })
                .collect()
        };
        let factory = Arc::new(ConnectorTestFactory::connected());
        let app = router(history_state(factory, &path));
        let post =
            |path: &'static str, body: serde_json::Value| history_post(app.clone(), path, body);
        // Hours 2..0 are not seeded yet: pin g's slow window first.
        let (_, trends) = post("/api/v1/history/trends", serde_json::json!({ "target": null, "current_hours": 24, "baseline_hours": 168, "limit": 50 })).await;
        let key_of = |template: &str| -> String {
            trends["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["template"] == template)
                .map(|row| row["template_key"].as_str().unwrap().to_owned())
                .unwrap_or_default()
        };
        let g = key_of("SELECT g FROM t WHERE name = ?");
        assert!(!g.is_empty(), "{trends}");
        let (code, item) = post("/api/v1/governance/create", serde_json::json!({ "target": "127.0.0.1:9030", "template_key": g, "origin": "trends", "alert_id": null, "title": null })).await;
        assert_eq!(code, StatusCode::OK, "{item}");
        let id = item["item_id"].as_str().unwrap().to_owned();
        assert_eq!(item["state"], "new");
        assert_eq!(
            item["allowed_transitions"],
            serde_json::json!(["triaged", "wont_fix"])
        );
        let (code, _) = post("/api/v1/governance/create", serde_json::json!({ "target": "127.0.0.1:9030", "template_key": g, "origin": "manual", "alert_id": null, "title": "again" })).await;
        assert_eq!(code, StatusCode::CONFLICT);
        // RD-02: closed and pending are never reachable by hand.
        for to in ["closed", "pending_verification", "in_progress"] {
            let (code, body) = post(
                "/api/v1/governance/transition",
                serde_json::json!({ "item_id": id, "to": to, "note": null }),
            )
            .await;
            assert_eq!(
                (code, body["code"].clone()),
                (
                    StatusCode::CONFLICT,
                    serde_json::json!("illegal_governance_transition")
                ),
                "{to}"
            );
        }
        let (code, _) = post(
            "/api/v1/governance/change",
            serde_json::json!({ "item_id": id, "note": "too early", "change_seconds": null }),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT);
        for to in ["triaged", "in_progress"] {
            let (code, body) = post(
                "/api/v1/governance/transition",
                serde_json::json!({ "item_id": id, "to": to, "note": "ok" }),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "{body}");
        }
        // A change needs a pinned "before" first.
        let (code, body) = post("/api/v1/governance/change", serde_json::json!({ "item_id": id, "note": "Added a partition", "change_seconds": null })).await;
        assert_eq!(
            (code, body["code"].clone()),
            (StatusCode::CONFLICT, serde_json::json!("snapshot_required"))
        );
        // The pin takes the alert window (last 3 hours): one slow hour of g.
        seed_history(
            &path,
            now,
            2..=2,
            |_| "3.5.14",
            |ago, start| {
                let mut rows = regressing(ago, start);
                rows.extend((0..20u64).map(|n| {
                    seeded_row(
                        "SELECT g FROM t WHERE name = 'y'",
                        format!("g-{ago}-{n}"),
                        start + 60,
                        250 + n % 3,
                        3_000,
                    )
                }));
                rows
            },
        );
        let (code, pinned) = post(
            "/api/v1/governance/pin",
            serde_json::json!({ "item_id": id }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{pinned}");
        let snapshot = &pinned["snapshot"];
        assert_eq!(snapshot["template"], "SELECT g FROM t WHERE name = ?");
        assert_eq!(snapshot["query_time_median"]["canonical_unit"], "ns");
        assert_eq!(snapshot["executions"], "20");
        // Record the change through the store with a back-dated time (the API
        // only accepts times after the pin), then add the faster hours.
        {
            let store = doctor_history::HistoryStore::open(&path).unwrap();
            let update = doctor_history::ItemUpdate {
                from: "in_progress".into(),
                to: "pending_verification".into(),
                kind: "change_recorded".into(),
                actor: "local_user".into(),
                note: Some("Added a partition".into()),
                change_at: Some(current - 2 * 3600 + 1),
                ..doctor_history::ItemUpdate::default()
            };
            assert_eq!(
                store
                    .update_item(id.parse().unwrap(), &update, now)
                    .unwrap(),
                doctor_history::StoreChange::Applied
            );
        }
        seed_history(
            &path,
            now,
            0..=1,
            |_| "3.5.14",
            |ago, start| {
                let mut rows = regressing(ago, start);
                rows.extend((0..20u64).map(|n| {
                    seeded_row(
                        "SELECT g FROM t WHERE name = 'x'",
                        format!("g-{ago}-{n}"),
                        start + 60,
                        150 + n % 3,
                        3_000,
                    )
                }));
                rows
            },
        );
        // RD-03: the user cannot close; the comparator does.
        let (code, _) = post(
            "/api/v1/governance/transition",
            serde_json::json!({ "item_id": id, "to": "closed", "note": null }),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT);
        let (code, verified) = post(
            "/api/v1/governance/verify",
            serde_json::json!({ "item_id": id }),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{verified}");
        assert_eq!(verified["state"], "closed");
        assert_eq!(
            verified["verification"]["outcome"],
            serde_json::json!({ "state": "improved" })
        );
        assert_eq!(verified["verification"]["executions_after"], "40");
        let events: Vec<(String, String)> = verified["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["kind"].as_str().unwrap().to_owned(),
                    e["actor"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            events.last().unwrap(),
            &("verified".to_owned(), "comparator".to_owned())
        );
        assert_eq!(events.len(), 6);

        // RD-01: the regression of a raises exactly one alert, however often evaluated.
        let _ = post(
            "/api/v1/history/alerts",
            serde_json::json!({ "target": null }),
        )
        .await;
        let (_, alerts) = post(
            "/api/v1/history/alerts",
            serde_json::json!({ "target": null }),
        )
        .await;
        let list = alerts["alerts"].as_array().unwrap();
        assert_eq!(list.len(), 1, "{alerts}");
        assert_eq!(list[0]["state"], "open");
        assert_eq!(list[0]["severity"], "high");
        assert_eq!(list[0]["template"], "SELECT a FROM t WHERE id = ?");
        assert!(
            list[0]["exemplar_query_id"]
                .as_str()
                .unwrap()
                .starts_with("a-")
        );
        assert_eq!(
            alerts["channels"][1],
            serde_json::json!({ "name": "webhook", "available": false })
        );
        assert_eq!(alerts["counts"]["open"], "1");
        let alert = list[0]["alert_id"].as_str().unwrap().to_owned();
        let (_, status) = post(
            "/api/v1/history/status",
            serde_json::json!({ "connection_id": null }),
        )
        .await;
        assert_eq!(status["open_alerts"], "1");
        let (code, _) = post(
            "/api/v1/history/alerts/action",
            serde_json::json!({ "alert_id": alert, "action": "reopen" }),
        )
        .await;
        assert_eq!(code, StatusCode::CONFLICT);
        let (code, _) = post(
            "/api/v1/history/alerts/action",
            serde_json::json!({ "alert_id": alert, "action": "mute" }),
        )
        .await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        let (_, muted) = post(
            "/api/v1/history/alerts",
            serde_json::json!({ "target": null }),
        )
        .await;
        assert_eq!(
            (
                muted["alerts"].as_array().unwrap().len(),
                muted["counts"]["muted"].clone()
            ),
            (1, serde_json::json!("1"))
        );
        assert!(
            muted["alerts"][0]["muted_until_seconds"].as_u64().unwrap() >= now as u64 + 23 * 3600
        );
        // An item made from the alert is linked back to it.
        let a = list[0]["template_key"].as_str().unwrap();
        let (code, linked) = post("/api/v1/governance/create", serde_json::json!({ "target": "127.0.0.1:9030", "template_key": a, "origin": "alert", "alert_id": alert, "title": null })).await;
        assert_eq!(
            (code, linked["alert_id"].clone()),
            (StatusCode::OK, serde_json::json!(alert))
        );
        let (_, listed) = post(
            "/api/v1/governance/list",
            serde_json::json!({ "target": null }),
        )
        .await;
        assert_eq!(listed["items"][0]["state"], "new");
        assert_eq!(listed["items"][1]["state"], "closed");
        drop(app);
        // RB-03: neither the snapshot nor any record keeps the literal.
        let mut bytes = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            bytes.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        assert!(!bytes.windows(25).any(|w| w == b"governance_secret_literal"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
