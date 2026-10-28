//! Narrow MySQL text-protocol transport for the v0.1 connector.
//!
//! This deliberately owns the small protocol surface we need instead of using
//! a general driver whose handshake advertises `LOCAL_FILES` or
//! `MULTI_STATEMENTS`.  It is synchronous by design: the API must call the
//! `SessionManager` through `spawn_blocking`.

use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use sha1::{Digest, Sha1};

use crate::{
    Remote, RemoteConnectOptions, RemoteError, RemoteFactory, RemoteOperation, RemoteResponse,
    SessionTimeZone, TlsMode, analyze_profile_statement, audit_active_at_sql_lookup_statement,
    audit_active_at_statement, audit_collection_slice_statement, audit_concurrency_statement,
    audit_coverage_statement, audit_family_detail_sql_lookup_statement,
    audit_family_detail_statement, audit_family_sample_statement, audit_family_statement,
    audit_fingerprint_event_statement, audit_fingerprint_sample_statement,
    audit_occurrence_sample_statement, audit_occurrence_statement,
    audit_search_hidden_counts_statement, audit_sql_lookup_statement, audit_statement,
    doctor_statement, explain_statement, is_uuid, quote_sql_literal, valid_identifier_path,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const PROFILE_READY_ATTEMPTS: usize = 10;
const PROFILE_READY_DELAY: Duration = Duration::from_millis(100);
pub const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_RESULT_ROWS: usize = 2_048;
pub const MAX_RESULT_COLUMNS: usize = 32;
pub const MAX_CELL_BYTES: usize = 16 * 1024 * 1024;

const CLIENT_LONG_PASSWORD: u32 = 0x0000_0001;
const CLIENT_LONG_FLAG: u32 = 0x0000_0004;
const CLIENT_CONNECT_WITH_DB: u32 = 0x0000_0008;
const CLIENT_PROTOCOL_41: u32 = 0x0000_0200;
const CLIENT_SSL: u32 = 0x0000_0800;
const CLIENT_TRANSACTIONS: u32 = 0x0000_2000;
const CLIENT_SECURE_CONNECTION: u32 = 0x0000_8000;
const CLIENT_PLUGIN_AUTH: u32 = 0x0008_0000;
const CLIENT_LOCAL_FILES: u32 = 0x0000_0080;
const CLIENT_MULTI_STATEMENTS: u32 = 0x0001_0000;
const SERVER_MORE_RESULTS_EXISTS: u16 = 0x0008;
const REQUIRED_SERVER_CAPABILITIES: u32 =
    CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH;

/// Capability allowlist for the handshake response.  Neither local-file
/// transfer nor multiple statements/results are ever advertised.
pub const CLIENT_CAPABILITY_ALLOWLIST: u32 = CLIENT_LONG_PASSWORD
    | CLIENT_LONG_FLAG
    | CLIENT_PROTOCOL_41
    | CLIENT_SSL
    | CLIENT_TRANSACTIONS
    | CLIENT_SECURE_CONNECTION
    | CLIENT_PLUGIN_AUTH;

#[derive(Default)]
pub struct MysqlTextRemoteFactory;

impl RemoteFactory for MysqlTextRemoteFactory {
    fn open(&self, options: RemoteConnectOptions) -> Result<Box<dyn Remote>, RemoteError> {
        MysqlTextRemote::connect(options).map(|remote| Box::new(remote) as Box<dyn Remote>)
    }
}

pub struct MysqlTextRemote {
    stream: WireStream,
    audit_control: Option<Arc<crate::OperationControl>>,
}

enum WireStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for WireStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for WireStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

struct ServerGreeting {
    capabilities: u32,
    auth_plugin: String,
    nonce: Vec<u8>,
}

struct ResultBudget {
    bytes: usize,
    max_bytes: usize,
    exceeded_error: RemoteError,
}

impl Default for ResultBudget {
    fn default() -> Self {
        Self {
            bytes: 0,
            max_bytes: MAX_RESULT_BYTES,
            exceeded_error: RemoteError::Protocol,
        }
    }
}

#[derive(Clone, Copy)]
enum TextResultPolicy {
    Generic,
    ExplainCosts,
    AnalyzeProfile,
    DiscardRows,
}

impl TextResultPolicy {
    fn row_limit_error(self) -> RemoteError {
        if matches!(self, Self::AnalyzeProfile) {
            RemoteError::OutputBudgetExceeded
        } else {
            RemoteError::Protocol
        }
    }
}

impl MysqlTextRemote {
    fn connect(options: RemoteConnectOptions) -> Result<Self, RemoteError> {
        let mut tcp = connect_tcp(&options.host, options.port)?;
        let mut greeting_budget = ResultBudget::default();
        let mut greeting_sequence = 0;
        let greeting = parse_server_greeting(&read_packet(
            &mut tcp,
            &mut greeting_budget,
            &mut greeting_sequence,
        )?)?;
        if greeting.auth_plugin != "mysql_native_password" {
            return Err(RemoteError::Authentication);
        }

        let mut capabilities = negotiate_client_capabilities(
            greeting.capabilities,
            options.tls_mode,
            options.default_database.is_some(),
        )?;

        let packet_sequence = match options.tls_mode {
            TlsMode::VerifyFull => {
                write_packet(&mut tcp, 1, &ssl_request(capabilities))?;
                tcp.flush().map_err(map_io_error)?;
                2
            }
            TlsMode::DisabledLoopbackDocker | TlsMode::Plaintext => {
                capabilities &= !CLIENT_SSL;
                1
            }
        };

        let mut stream = match options.tls_mode {
            TlsMode::VerifyFull => WireStream::Tls(Box::new(tls_stream(tcp, &options.host)?)),
            TlsMode::DisabledLoopbackDocker | TlsMode::Plaintext => WireStream::Plain(tcp),
        };
        let auth = mysql_native_password(options.password.as_bytes(), &greeting.nonce);
        let response = handshake_response(
            capabilities,
            &options.username,
            &auth,
            options.default_database.as_deref(),
        )?;
        write_packet(&mut stream, packet_sequence, &response)?;
        stream.flush().map_err(map_io_error)?;
        let mut budget = ResultBudget::default();
        let mut server_sequence = packet_sequence.wrapping_add(1);
        let server_reply = read_packet(&mut stream, &mut budget, &mut server_sequence)?;
        match server_reply.first().copied() {
            Some(0x00) => Ok(Self {
                stream,
                audit_control: None,
            }),
            Some(0xff) => Err(server_error(&server_reply)),
            Some(_) | None => Err(RemoteError::Protocol),
        }
    }

    fn query(&mut self, statement: &str) -> Result<Vec<Vec<String>>, RemoteError> {
        self.query_nullable_with_policy(statement, TextResultPolicy::Generic)
            .map(collapse_nullable_rows)
    }

    fn query_nullable(&mut self, statement: &str) -> Result<Vec<Vec<Option<String>>>, RemoteError> {
        self.query_nullable_with_policy(statement, TextResultPolicy::Generic)
    }

    /// Every fixed statement this tool owns carries the product statement
    /// marker so the explicit Audit statement-class filter can recognise it.
    fn query_nullable_with_policy(
        &mut self,
        statement: &str,
        policy: TextResultPolicy,
    ) -> Result<Vec<Vec<Option<String>>>, RemoteError> {
        self.send_query(statement, policy, StatementOrigin::Doctor)
    }

    /// The single user-supplied Runtime Top SELECT/WITH is sent verbatim: it is
    /// the user's workload, never hidden as this tool's own statement.
    fn query_user_statement_with_policy(
        &mut self,
        statement: &str,
        policy: TextResultPolicy,
    ) -> Result<Vec<Vec<String>>, RemoteError> {
        self.send_query(statement, policy, StatementOrigin::User)
            .map(collapse_nullable_rows)
    }

    fn send_query(
        &mut self,
        statement: &str,
        policy: TextResultPolicy,
        origin: StatementOrigin,
    ) -> Result<Vec<Vec<Option<String>>>, RemoteError> {
        if statement.is_empty() || statement.len() > crate::MAX_PLAN_SQL_BYTES {
            return Err(RemoteError::Protocol);
        }
        if let Some(control) = &self.audit_control {
            control.check().map_err(|_| RemoteError::Timeout)?;
        }
        let statement = match origin {
            StatementOrigin::Doctor => doctor_statement(statement),
            StatementOrigin::User => statement.to_owned(),
        };
        let mut payload = Vec::with_capacity(statement.len() + 1);
        payload.push(0x03); // COM_QUERY
        payload.extend_from_slice(statement.as_bytes());
        write_packet(&mut self.stream, 0, &payload)?;
        self.stream.flush().map_err(map_io_error)?;
        read_nullable_text_result_with_policy(&mut self.stream, policy)
    }

    fn audit_query(&mut self, statement: &str) -> Result<Vec<Vec<Option<String>>>, RemoteError> {
        self.query(&format!(
            "SET query_timeout = {}",
            crate::AUDIT_QUERY_TIMEOUT_SECONDS
        ))?;
        self.query_nullable(statement)
    }

    fn query_profile_when_ready(&mut self, query_id: &str) -> Result<String, RemoteError> {
        let statement = format!("SELECT get_query_profile({})", quote_sql_literal(query_id));
        for attempt in 0..PROFILE_READY_ATTEMPTS {
            let profile = parse_query_profile(self.query(&statement)?)?;
            if !profile.is_empty() || attempt + 1 == PROFILE_READY_ATTEMPTS {
                return Ok(profile);
            }
            // StarRocks publishes a finished query's Profile asynchronously.
            // Retry only the fixed profile lookup; never re-run user SQL.
            std::thread::sleep(PROFILE_READY_DELAY);
        }
        Ok(String::new())
    }
}

#[derive(Clone, Copy)]
enum StatementOrigin {
    Doctor,
    User,
}

fn negotiate_client_capabilities(
    server_capabilities: u32,
    tls_mode: TlsMode,
    connect_with_database: bool,
) -> Result<u32, RemoteError> {
    if server_capabilities & REQUIRED_SERVER_CAPABILITIES != REQUIRED_SERVER_CAPABILITIES {
        return Err(RemoteError::Protocol);
    }
    if tls_mode == TlsMode::VerifyFull && server_capabilities & CLIENT_SSL == 0 {
        return Err(RemoteError::Tls);
    }
    if connect_with_database && server_capabilities & CLIENT_CONNECT_WITH_DB == 0 {
        return Err(RemoteError::Protocol);
    }
    let mut capabilities = CLIENT_CAPABILITY_ALLOWLIST & server_capabilities;
    if connect_with_database {
        capabilities |= CLIENT_CONNECT_WITH_DB;
    }
    if tls_mode.is_plaintext() {
        capabilities &= !CLIENT_SSL;
    }
    if capabilities & (CLIENT_LOCAL_FILES | CLIENT_MULTI_STATEMENTS) != 0 {
        return Err(RemoteError::Protocol);
    }
    Ok(capabilities)
}

impl Remote for MysqlTextRemote {
    fn set_audit_control(
        &mut self,
        control: Option<Arc<crate::OperationControl>>,
    ) -> Result<(), RemoteError> {
        if let Some(control) = &control {
            let socket = match &self.stream {
                WireStream::Plain(stream) => stream.try_clone(),
                WireStream::Tls(stream) => stream.sock.try_clone(),
            }
            .map_err(map_io_error)?;
            control.bind(socket).map_err(|_| RemoteError::Timeout)?;
        }
        self.audit_control = control;
        Ok(())
    }

    fn run(&mut self, operation: RemoteOperation) -> Result<RemoteResponse, RemoteError> {
        match operation {
            RemoteOperation::CurrentVersion => {
                let rows = self.query("SELECT current_version()")?;
                let version = parse_current_version(rows)?;
                Ok(RemoteResponse::Version(version))
            }
            RemoteOperation::SessionTimeZone => {
                // The current session variable is the authority for DATETIME
                // interpretation.  The offset is evidence for operators and
                // diagnostics only; historical Audit rows use UNIX_TIMESTAMP
                // in the same session so DST is resolved by StarRocks itself.
                self.query(
                    "SELECT @@time_zone, @@system_time_zone, TIME_TO_SEC(TIMEDIFF(NOW(), UTC_TIMESTAMP()))",
                )
                .and_then(parse_session_time_zone)
                .map(RemoteResponse::TimeZone)
            }
            RemoteOperation::AuditProbe { table } => {
                if !valid_identifier_path(&table) {
                    return Err(RemoteError::Protocol);
                }
                let statement = format!(
                    "SELECT 1 FROM {} LIMIT 1",
                    crate::quote_identifier_path(&table)
                );
                self.query(&statement).map(RemoteResponse::Rows)
            }
            RemoteOperation::AuditSearch { table, parameters } => {
                let statement =
                    audit_statement(&table, &parameters).map_err(|_| RemoteError::Protocol)?;
                self.query(&statement).map(RemoteResponse::Rows)
            }
            RemoteOperation::AuditSearchHiddenCounts { table, parameters } => {
                let statement = audit_search_hidden_counts_statement(&table, &parameters)
                    .map_err(|_| RemoteError::Protocol)?;
                self.query(&statement).map(RemoteResponse::Rows)
            }
            RemoteOperation::AuditSchemaProbe { table } => self
                .audit_query(&format!("SHOW COLUMNS FROM {}", table.quoted()))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditOccurrences {
                table,
                request,
                schema,
            } => audit_occurrence_statement(&table, &request, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditFamilies {
                table,
                request,
                schema,
            } => audit_family_statement(&table, &request, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditFamilyDetail {
                table,
                parent,
                target,
                candidates,
                schema,
            } => audit_family_detail_statement(&table, &parent, &target, &candidates, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditFamilySamples {
                table,
                scope,
                targets,
                schema,
                chars,
            } => audit_family_sample_statement(&table, &scope, &targets, &schema, chars)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditOccurrenceSamples {
                table,
                scope,
                window,
                query_ids,
                schema,
            } => audit_occurrence_sample_statement(&table, &scope, window, &query_ids, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditCollectionSlice {
                table,
                scope,
                schema,
            } => audit_collection_slice_statement(&table, &scope, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditFingerprintEvents {
                table,
                scope,
                schema,
            } => audit_fingerprint_event_statement(&table, &scope, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditFingerprintSamples {
                table,
                scope,
                query_ids,
                schema,
            } => audit_fingerprint_sample_statement(&table, &scope, &query_ids, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditFamilyDetailSqlLookup {
                table,
                parent,
                target,
                query_id,
                schema,
            } => audit_family_detail_sql_lookup_statement(
                &table, &parent, &target, &query_id, &schema,
            )
            .map_err(|_| RemoteError::Protocol)
            .and_then(|statement| self.audit_query(&statement))
            .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditCoverage {
                table,
                scope,
                schema,
            } => audit_coverage_statement(&table, &scope, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditConcurrency {
                table,
                scope,
                schema,
            } => audit_concurrency_statement(&table, &scope, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditActiveAt {
                table,
                scope,
                at_milliseconds,
                schema,
            } => audit_active_at_statement(&table, &scope, at_milliseconds, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditOverviewFamilyTop {
                table,
                request,
                schema,
            } => audit_family_statement(&table, &request, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditSqlLookup {
                table,
                request,
                schema,
            } => audit_sql_lookup_statement(&table, &request, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::AuditActiveAtSqlLookup {
                table,
                request,
                at_milliseconds,
                schema,
            } => audit_active_at_sql_lookup_statement(&table, &request, at_milliseconds, &schema)
                .map_err(|_| RemoteError::Protocol)
                .and_then(|statement| self.audit_query(&statement))
                .map(RemoteResponse::NullableRows),
            RemoteOperation::ShowProfileList { limit } => self
                .query(&format!("SHOW PROFILELIST LIMIT {limit}"))
                .map(RemoteResponse::Rows),
            RemoteOperation::GetQueryProfile { query_id } => {
                if !is_uuid(&query_id) {
                    return Err(RemoteError::Protocol);
                }
                let rows = self.query(&format!(
                    "SELECT get_query_profile({})",
                    quote_sql_literal(&query_id)
                ))?;
                let profile = parse_query_profile(rows)?;
                Ok(RemoteResponse::Profile(profile))
            }
            RemoteOperation::AnalyzeProfile { query_id, node_ids } => {
                let statement = analyze_profile_statement(&query_id, &node_ids)
                    .map_err(|_| RemoteError::Protocol)?;
                self.query(&format!(
                    "SET query_timeout = {}",
                    crate::ANALYZE_PROFILE_TIMEOUT_SECONDS
                ))?;
                let rows =
                    self.query_nullable_with_policy(&statement, TextResultPolicy::AnalyzeProfile)?;
                Ok(RemoteResponse::AnalyzeProfile(
                    rows.into_iter()
                        .map(|row| {
                            let [Some(value)] = row.as_slice() else {
                                return Err(RemoteError::Protocol);
                            };
                            Ok(value.clone())
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            }
            RemoteOperation::ShowPlanAdvisor => {
                self.query("SHOW PLAN ADVISOR").map(RemoteResponse::Rows)
            }
            RemoteOperation::Explain {
                candidate_sql,
                mode,
            } => {
                let statement =
                    explain_statement(&candidate_sql, &mode).map_err(|_| RemoteError::Protocol)?;
                self.query(&format!(
                    "SET query_timeout = {}",
                    crate::ANALYZE_PROFILE_TIMEOUT_SECONDS
                ))?;
                let rows =
                    self.query_nullable_with_policy(&statement, TextResultPolicy::ExplainCosts)?;
                Ok(RemoteResponse::Explain(
                    rows.into_iter()
                        .map(|row| {
                            let [Some(value)] = row.as_slice() else {
                                return Err(RemoteError::Protocol);
                            };
                            Ok(value.clone())
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            }
            RemoteOperation::ProfiledSelect { candidate_sql } => {
                // These are server-owned fixed statements. The user statement
                // remains one validated SELECT/WITH and multi-statements are
                // never advertised by the handshake.
                self.query("SET enable_profile = true")?;
                self.query("SET pipeline_profile_level = 1")?;
                self.query(&format!(
                    "SET query_timeout = {}",
                    crate::AUDIT_QUERY_TIMEOUT_SECONDS
                ))?;
                self.query_user_statement_with_policy(
                    &candidate_sql,
                    TextResultPolicy::DiscardRows,
                )?;
                let query_id = exactly_one_scalar(self.query("SELECT last_query_id()")?)?;
                if !is_uuid(&query_id) {
                    return Err(RemoteError::Protocol);
                }
                let profile = self.query_profile_when_ready(&query_id)?;
                Ok(RemoteResponse::ProfiledQuery { query_id, profile })
            }
        }
    }

    fn close(&mut self) {
        let _ = self.stream.flush();
    }
}

fn exactly_one_scalar(rows: Vec<Vec<String>>) -> Result<String, RemoteError> {
    let [row] = rows.as_slice() else {
        return Err(RemoteError::Protocol);
    };
    let [value] = row.as_slice() else {
        return Err(RemoteError::Protocol);
    };
    Ok(value.clone())
}

fn parse_current_version(rows: Vec<Vec<String>>) -> Result<String, RemoteError> {
    let version = exactly_one_scalar(rows)?;
    if version.is_empty() {
        return Err(RemoteError::Protocol);
    }
    Ok(version)
}

fn parse_query_profile(rows: Vec<Vec<String>>) -> Result<String, RemoteError> {
    exactly_one_scalar(rows)
}

fn parse_session_time_zone(rows: Vec<Vec<String>>) -> Result<SessionTimeZone, RemoteError> {
    let mut rows = rows.into_iter();
    let row = rows.next().ok_or(RemoteError::Protocol)?;
    if rows.next().is_some() {
        return Err(RemoteError::Protocol);
    }
    let [session_time_zone, system_time_zone, offset] = row.as_slice() else {
        return Err(RemoteError::Protocol);
    };
    let offset = offset
        .parse::<i32>()
        .ok()
        .filter(|value| (-14 * 60 * 60..=14 * 60 * 60).contains(value))
        .ok_or(RemoteError::Protocol)?;
    let valid_zone = |value: &str| {
        !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
    };
    if !valid_zone(session_time_zone) || !valid_zone(system_time_zone) {
        return Err(RemoteError::Protocol);
    }
    let effective_time_zone = if session_time_zone.eq_ignore_ascii_case("SYSTEM") {
        system_time_zone.clone()
    } else {
        session_time_zone.clone()
    };
    Ok(SessionTimeZone {
        session_time_zone: session_time_zone.clone(),
        system_time_zone: system_time_zone.clone(),
        effective_time_zone,
        current_utc_offset_seconds: offset,
    })
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, RemoteError> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| RemoteError::Connection)?;
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream.set_nodelay(true).map_err(map_io_error)?;
                stream
                    .set_read_timeout(Some(READ_WRITE_TIMEOUT))
                    .map_err(map_io_error)?;
                stream
                    .set_write_timeout(Some(READ_WRITE_TIMEOUT))
                    .map_err(map_io_error)?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.map_or(RemoteError::Connection, map_io_error))
}

fn tls_stream(
    tcp: TcpStream,
    host: &str,
) -> Result<StreamOwned<ClientConnection, TcpStream>, RemoteError> {
    let loaded = rustls_native_certs::load_native_certs();
    if !loaded.errors.is_empty() || loaded.certs.is_empty() {
        return Err(RemoteError::Tls);
    }
    let mut roots = RootCertStore::empty();
    for certificate in loaded.certs {
        roots.add(certificate).map_err(|_| RemoteError::Tls)?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| RemoteError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = ServerName::try_from(host.to_owned()).map_err(|_| RemoteError::Tls)?;
    let connection =
        ClientConnection::new(Arc::new(config), server_name).map_err(|_| RemoteError::Tls)?;
    let mut stream = StreamOwned::new(connection, tcp);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|_| RemoteError::Tls)?;
    if stream.conn.is_handshaking() {
        return Err(RemoteError::Tls);
    }
    Ok(stream)
}

fn ssl_request(capabilities: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32);
    payload.extend_from_slice(&capabilities.to_le_bytes());
    payload.extend_from_slice(&(MAX_PACKET_BYTES as u32).to_le_bytes());
    payload.push(45); // utf8mb4_general_ci
    payload.extend_from_slice(&[0; 23]);
    payload
}

fn handshake_response(
    capabilities: u32,
    username: &str,
    auth: &[u8],
    database: Option<&str>,
) -> Result<Vec<u8>, RemoteError> {
    if username.is_empty() || username.contains('\0') || auth.len() > 255 {
        return Err(RemoteError::Protocol);
    }
    let mut payload = ssl_request(capabilities);
    payload.extend_from_slice(username.as_bytes());
    payload.push(0);
    payload.push(auth.len() as u8);
    payload.extend_from_slice(auth);
    if capabilities & CLIENT_CONNECT_WITH_DB != 0 {
        let Some(database) = database else {
            return Err(RemoteError::Protocol);
        };
        payload.extend_from_slice(database.as_bytes());
        payload.push(0);
    }
    payload.extend_from_slice(b"mysql_native_password\0");
    Ok(payload)
}

fn mysql_native_password(password: &[u8], nonce: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let first = Sha1::digest(password);
    let second = Sha1::digest(first);
    let mut seeded = Vec::with_capacity(nonce.len() + second.len());
    seeded.extend_from_slice(nonce);
    seeded.extend_from_slice(&second);
    let third = Sha1::digest(seeded);
    first
        .iter()
        .zip(third.iter())
        .map(|(left, right)| left ^ right)
        .collect()
}

fn parse_server_greeting(payload: &[u8]) -> Result<ServerGreeting, RemoteError> {
    if payload.first() != Some(&0x0a) {
        return Err(RemoteError::Protocol);
    }
    let mut index = 1;
    let _server_version = read_nul_terminated(payload, &mut index)?;
    take(payload, &mut index, 4)?;
    let nonce_first = take(payload, &mut index, 8)?;
    take(payload, &mut index, 1)?;
    let capabilities_low = read_u16(payload, &mut index)? as u32;
    if index == payload.len() {
        return Err(RemoteError::Protocol);
    }
    take(payload, &mut index, 1)?; // collation
    take(payload, &mut index, 2)?; // server status
    let capabilities = capabilities_low | ((read_u16(payload, &mut index)? as u32) << 16);
    let auth_plugin_len = take(payload, &mut index, 1)?[0] as usize;
    take(payload, &mut index, 10)?;
    let remainder = payload.get(index..).ok_or(RemoteError::Protocol)?;
    let expected_auth_tail = auth_plugin_len.saturating_sub(8).max(13);
    let auth_tail_len = remainder.len().min(expected_auth_tail);
    let mut nonce = nonce_first.to_vec();
    nonce.extend_from_slice(&remainder[..auth_tail_len]);
    while nonce.last() == Some(&0) {
        nonce.pop();
    }
    index = index.saturating_add(auth_tail_len);
    let auth_plugin = if index < payload.len() {
        let mut plugin_index = index;
        String::from_utf8(read_nul_terminated(payload, &mut plugin_index)?.to_vec())
            .map_err(|_| RemoteError::Protocol)?
    } else {
        return Err(RemoteError::Protocol);
    };
    if nonce.len() < 20 {
        return Err(RemoteError::Protocol);
    }
    Ok(ServerGreeting {
        capabilities,
        auth_plugin,
        nonce,
    })
}

#[cfg(test)]
fn read_text_result(stream: &mut impl Read) -> Result<Vec<Vec<String>>, RemoteError> {
    read_text_result_with_policy(stream, TextResultPolicy::Generic)
}

#[cfg(test)]
fn read_text_result_with_policy(
    stream: &mut impl Read,
    policy: TextResultPolicy,
) -> Result<Vec<Vec<String>>, RemoteError> {
    read_nullable_text_result_with_policy(stream, policy).map(collapse_nullable_rows)
}

fn read_nullable_text_result_with_policy(
    stream: &mut impl Read,
    policy: TextResultPolicy,
) -> Result<Vec<Vec<Option<String>>>, RemoteError> {
    let mut budget = if matches!(policy, TextResultPolicy::AnalyzeProfile) {
        ResultBudget {
            // Bound allocation before packet payload reads, including protocol overhead.
            max_bytes: crate::ANALYZE_PROFILE_MAX_OUTPUT_BYTES + 64 * 1024,
            exceeded_error: RemoteError::OutputBudgetExceeded,
            ..ResultBudget::default()
        }
    } else {
        ResultBudget::default()
    };
    let mut sequence = 1;
    let first = read_packet(stream, &mut budget, &mut sequence)?;
    match first.first().copied() {
        Some(0xff) => return Err(server_error(&first)),
        Some(0xfb) => return Err(RemoteError::Protocol),
        Some(0x00) => {
            if matches!(policy, TextResultPolicy::AnalyzeProfile) {
                return Err(RemoteError::Protocol);
            }
            require_single_result(ok_status(&first)?)?;
            return Ok(Vec::new());
        }
        Some(_) => {}
        None => return Err(RemoteError::Protocol),
    }
    let mut first_index = 0;
    let columns = read_lenenc(&first, &mut first_index)?.ok_or(RemoteError::Protocol)?;
    if first_index != first.len() || !(1..=MAX_RESULT_COLUMNS).contains(&columns) {
        return Err(RemoteError::Protocol);
    }
    if matches!(
        policy,
        TextResultPolicy::ExplainCosts | TextResultPolicy::AnalyzeProfile
    ) && columns != 1
    {
        return Err(RemoteError::Protocol);
    }
    for _ in 0..columns {
        let field = read_packet(stream, &mut budget, &mut sequence)?;
        if matches!(field.first(), Some(0xff | 0xfb)) {
            return Err(RemoteError::Protocol);
        }
    }
    require_single_result(terminator_status(&read_packet(
        stream,
        &mut budget,
        &mut sequence,
    )?)?)?;

    let mut rows = Vec::new();
    let mut official_text_bytes = 0;
    loop {
        let packet = read_packet(stream, &mut budget, &mut sequence)?;
        match packet.first().copied() {
            Some(0xff) => return Err(server_error(&packet)),
            // LOCAL_INFILE is only valid as the first response packet.  In a
            // row stream, 0xfb is a legal NULL length-encoded first cell.
            Some(0xfb) => {
                if !matches!(policy, TextResultPolicy::DiscardRows) && rows.len() >= MAX_RESULT_ROWS
                {
                    return Err(policy.row_limit_error());
                }
                let row = parse_nullable_text_row(&packet, columns, policy)?;
                check_official_text_budget(policy, &row, rows.len(), &mut official_text_bytes)?;
                if !matches!(policy, TextResultPolicy::DiscardRows) {
                    rows.push(row);
                }
            }
            Some(0xfe) if packet.len() < 9 => {
                require_single_result(terminator_status(&packet)?)?;
                return Ok(rows);
            }
            // We intentionally do not advertise CLIENT_DEPRECATE_EOF, so an
            // EOF packet terminates a text result.  A row can legitimately
            // start with 0x00 when its first column is the empty string; do
            // not misclassify that row as an OK packet.
            Some(0x00) => {
                if !matches!(policy, TextResultPolicy::DiscardRows) && rows.len() >= MAX_RESULT_ROWS
                {
                    return Err(policy.row_limit_error());
                }
                let row = parse_nullable_text_row(&packet, columns, policy)?;
                check_official_text_budget(policy, &row, rows.len(), &mut official_text_bytes)?;
                if !matches!(policy, TextResultPolicy::DiscardRows) {
                    rows.push(row);
                }
            }
            Some(_) => {
                if !matches!(policy, TextResultPolicy::DiscardRows) && rows.len() >= MAX_RESULT_ROWS
                {
                    return Err(policy.row_limit_error());
                }
                let row = parse_nullable_text_row(&packet, columns, policy)?;
                check_official_text_budget(policy, &row, rows.len(), &mut official_text_bytes)?;
                if !matches!(policy, TextResultPolicy::DiscardRows) {
                    rows.push(row);
                }
            }
            None => return Err(RemoteError::Protocol),
        }
    }
}

fn check_official_text_budget(
    policy: TextResultPolicy,
    row: &[Option<String>],
    previous_rows: usize,
    bytes: &mut usize,
) -> Result<(), RemoteError> {
    if matches!(policy, TextResultPolicy::AnalyzeProfile) {
        let [Some(value)] = row else {
            return Err(RemoteError::Protocol);
        };
        *bytes = bytes
            .checked_add(value.len())
            .and_then(|size| size.checked_add(usize::from(previous_rows > 0)))
            .filter(|size| *size <= crate::ANALYZE_PROFILE_MAX_OUTPUT_BYTES)
            .ok_or(RemoteError::OutputBudgetExceeded)?;
    }
    Ok(())
}

fn parse_nullable_text_row(
    packet: &[u8],
    columns: usize,
    policy: TextResultPolicy,
) -> Result<Vec<Option<String>>, RemoteError> {
    let mut index = 0;
    let mut row = Vec::with_capacity(columns);
    for _ in 0..columns {
        let value = read_lenenc(packet, &mut index)?;
        let Some(value) = value else {
            if matches!(
                policy,
                TextResultPolicy::ExplainCosts | TextResultPolicy::AnalyzeProfile
            ) {
                return Err(RemoteError::Protocol);
            }
            row.push(None);
            continue;
        };
        if value > MAX_CELL_BYTES
            || index
                .checked_add(value)
                .is_none_or(|end| end > packet.len())
        {
            return Err(RemoteError::Protocol);
        }
        let end = index + value;
        let text = std::str::from_utf8(&packet[index..end])
            .map_err(|_| RemoteError::Protocol)?
            .to_owned();
        row.push(Some(text));
        index = end;
    }
    if index != packet.len() {
        return Err(RemoteError::Protocol);
    }
    Ok(row)
}

fn collapse_nullable_rows(rows: Vec<Vec<Option<String>>>) -> Vec<Vec<String>> {
    rows.into_iter()
        .map(|row| row.into_iter().map(Option::unwrap_or_default).collect())
        .collect()
}

fn require_single_result(status: u16) -> Result<(), RemoteError> {
    if status & SERVER_MORE_RESULTS_EXISTS != 0 {
        Err(RemoteError::Protocol)
    } else {
        Ok(())
    }
}

fn terminator_status(packet: &[u8]) -> Result<u16, RemoteError> {
    match packet.first() {
        Some(0xfe) if packet.len() >= 5 && packet.len() < 9 => {
            Ok(u16::from_le_bytes([packet[3], packet[4]]))
        }
        Some(0x00) => ok_status(packet),
        _ => Err(RemoteError::Protocol),
    }
}

fn ok_status(packet: &[u8]) -> Result<u16, RemoteError> {
    if packet.first() != Some(&0x00) {
        return Err(RemoteError::Protocol);
    }
    let mut index = 1;
    let _affected = read_lenenc(packet, &mut index)?;
    let _last_id = read_lenenc(packet, &mut index)?;
    read_u16(packet, &mut index)
}

fn server_error(packet: &[u8]) -> RemoteError {
    if packet.len() < 3 || packet[0] != 0xff {
        return RemoteError::Protocol;
    }
    let code = u16::from_le_bytes([packet[1], packet[2]]);
    let sql_state = packet.get(3..9);
    match code {
        1045 => RemoteError::Authentication,
        1044 | 1142 | 1227 => RemoteError::PermissionDenied,
        1049 | 1146 => RemoteError::Unsupported,
        5501 if sql_state == Some(&b"#3F000"[..]) => RemoteError::Unsupported,
        5502 if sql_state == Some(&b"#42602"[..]) || sql_state == Some(&b"#42S02"[..]) => {
            RemoteError::Unsupported
        }
        // StarRocks 3.5.14 `ANALYZE PROFILE` on a query whose Profile is no
        // longer retained: `Preconditions.checkNotNull(profileElement, "query
        // not exists")` (StmtExecutor.java:1771) reaches `catch (Throwable)`
        // without an ErrorCode, so MysqlErrPacket sends its default 1064 /
        // HY000 with exactly that message. A well-formed ERR packet: the
        // session is healthy and must not be discarded. 1064 alone is also a
        // syntax error, hence the exact state and message.
        1064 if sql_state == Some(&b"#HY000"[..])
            && packet.get(9..) == Some(&b"query not exists"[..]) =>
        {
            RemoteError::QueryProfileNotFound
        }
        _ => RemoteError::Connection,
    }
}

fn read_packet(
    stream: &mut impl Read,
    budget: &mut ResultBudget,
    expected_sequence: &mut u8,
) -> Result<Vec<u8>, RemoteError> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).map_err(map_io_error)?;
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    if header[3] != *expected_sequence
        || length == 0
        || length == 0x00ff_ffff
        || length >= MAX_PACKET_BYTES
    {
        return Err(RemoteError::Protocol);
    }
    if budget
        .bytes
        .checked_add(length + 4)
        .is_none_or(|next| next > budget.max_bytes)
    {
        return Err(budget.exceeded_error);
    }
    *expected_sequence = expected_sequence.wrapping_add(1);
    budget.bytes += length + 4;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).map_err(map_io_error)?;
    Ok(payload)
}

fn write_packet(stream: &mut impl Write, sequence: u8, payload: &[u8]) -> Result<(), RemoteError> {
    if payload.is_empty() || payload.len() >= MAX_PACKET_BYTES {
        return Err(RemoteError::Protocol);
    }
    let length = payload.len();
    let header = [
        length as u8,
        (length >> 8) as u8,
        (length >> 16) as u8,
        sequence,
    ];
    stream.write_all(&header).map_err(map_io_error)?;
    stream.write_all(payload).map_err(map_io_error)
}

fn read_lenenc(payload: &[u8], index: &mut usize) -> Result<Option<usize>, RemoteError> {
    let first = *payload.get(*index).ok_or(RemoteError::Protocol)?;
    *index += 1;
    match first {
        0xfb => Ok(None),
        0xfc => Ok(Some(read_le(payload, index, 2)?)),
        0xfd => Ok(Some(read_le(payload, index, 3)?)),
        0xfe => Ok(Some(read_le(payload, index, 8)?)),
        0xff => Err(RemoteError::Protocol),
        value => Ok(Some(value as usize)),
    }
}

fn read_le(payload: &[u8], index: &mut usize, width: usize) -> Result<usize, RemoteError> {
    let bytes = take(payload, index, width)?;
    let mut value = 0_usize;
    for (offset, byte) in bytes.iter().enumerate() {
        value |= usize::from(*byte) << (offset * 8);
    }
    Ok(value)
}

fn read_u16(payload: &[u8], index: &mut usize) -> Result<u16, RemoteError> {
    let bytes = take(payload, index, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_nul_terminated<'a>(payload: &'a [u8], index: &mut usize) -> Result<&'a [u8], RemoteError> {
    let rest = payload.get(*index..).ok_or(RemoteError::Protocol)?;
    let length = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(RemoteError::Protocol)?;
    let value = &rest[..length];
    *index += length + 1;
    Ok(value)
}

fn take<'a>(payload: &'a [u8], index: &mut usize, length: usize) -> Result<&'a [u8], RemoteError> {
    let end = index.checked_add(length).ok_or(RemoteError::Protocol)?;
    let value = payload.get(*index..end).ok_or(RemoteError::Protocol)?;
    *index = end;
    Ok(value)
}

fn map_io_error(error: io::Error) -> RemoteError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => RemoteError::Timeout,
        _ => RemoteError::Connection,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Cursor, Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    use doctor_contracts::ExplainMode;

    use super::*;

    fn packet(sequence: u8, payload: &[u8]) -> Vec<u8> {
        let length = payload.len();
        let mut bytes = vec![
            length as u8,
            (length >> 8) as u8,
            (length >> 16) as u8,
            sequence,
        ];
        bytes.extend_from_slice(payload);
        bytes
    }

    fn text_result(columns: u8, row_payloads: Vec<Vec<u8>>) -> Vec<u8> {
        let mut bytes = packet(1, &[columns]);
        let mut sequence: u8 = 2;
        for _ in 0..columns {
            bytes.extend(packet(sequence, b"field"));
            sequence = sequence.wrapping_add(1);
        }
        let eof = [0xfe, 0, 0, 0, 0];
        bytes.extend(packet(sequence, &eof));
        sequence = sequence.wrapping_add(1);
        for row_payload in row_payloads {
            bytes.extend(packet(sequence, &row_payload));
            sequence = sequence.wrapping_add(1);
        }
        bytes.extend(packet(sequence, &eof));
        bytes
    }

    fn loopback_remote<F>(script: F) -> (MysqlTextRemote, thread::JoinHandle<Vec<String>>)
    where
        F: FnOnce(TcpStream) -> Vec<String> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            script(stream)
        });
        let client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        (
            MysqlTextRemote {
                stream: WireStream::Plain(client),
                audit_control: None,
            },
            server,
        )
    }

    fn read_com_query(stream: &mut TcpStream) -> String {
        let mut header = [0; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header[3], 0);
        let length =
            usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).unwrap();
        assert_eq!(payload.first(), Some(&0x03));
        String::from_utf8(payload[1..].to_vec()).unwrap()
    }

    fn write_ok(stream: &mut TcpStream) {
        stream
            .write_all(&packet(1, &[0x00, 0, 0, 0, 0, 0, 0]))
            .unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn explain_modes_set_fixed_timeout_before_their_single_explain_command() {
        for (mode, expected_explain) in [
            (ExplainMode::Logical, "EXPLAIN LOGICAL SELECT 1"),
            (ExplainMode::Verbose, "EXPLAIN VERBOSE SELECT 1"),
            (ExplainMode::Costs, "EXPLAIN COSTS SELECT 1"),
        ] {
            let (mut remote, server) = loopback_remote(|mut stream| {
                let timeout = read_com_query(&mut stream);
                write_ok(&mut stream);
                let explain = read_com_query(&mut stream);
                stream
                    .write_all(&text_result(1, vec![vec![4, b'P', b'L', b'A', b'N']]))
                    .unwrap();
                stream.flush().unwrap();
                vec![timeout, explain]
            });

            let RemoteResponse::Explain(rows) = remote
                .run(RemoteOperation::Explain {
                    candidate_sql: "SELECT 1".into(),
                    mode,
                })
                .unwrap()
            else {
                panic!("expected EXPLAIN response");
            };
            assert_eq!(rows, vec!["PLAN"]);
            drop(remote);
            assert_eq!(
                server.join().unwrap(),
                vec![
                    doctor_statement("SET query_timeout = 10"),
                    doctor_statement(expected_explain)
                ]
            );
        }
    }

    #[test]
    fn explain_modes_do_not_send_explain_when_timeout_setup_connection_fails() {
        for mode in [
            ExplainMode::Logical,
            ExplainMode::Verbose,
            ExplainMode::Costs,
        ] {
            let (mut remote, server) =
                loopback_remote(|mut stream| vec![read_com_query(&mut stream)]);

            assert!(matches!(
                remote.run(RemoteOperation::Explain {
                    candidate_sql: "SELECT 1".into(),
                    mode,
                }),
                Err(RemoteError::Connection)
            ));
            drop(remote);
            assert_eq!(
                server.join().unwrap(),
                vec![doctor_statement("SET query_timeout = 10")]
            );
        }
    }

    #[test]
    fn explain_modes_stop_after_timeout_permission_denial_on_live_connection() {
        for mode in [
            ExplainMode::Logical,
            ExplainMode::Verbose,
            ExplainMode::Costs,
        ] {
            let (mut remote, server) = loopback_remote(|mut stream| {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let timeout = read_com_query(&mut stream);
                let mut denied = vec![0xff];
                denied.extend_from_slice(&1227_u16.to_le_bytes());
                denied.extend_from_slice(b"#42000denied");
                stream.write_all(&packet(1, &denied)).unwrap();
                stream.flush().unwrap();
                // Keep the server alive: any fallback command would be observable.
                let mut next = [0];
                assert_eq!(stream.read(&mut next).unwrap(), 0);
                vec![timeout]
            });
            assert!(matches!(
                remote.run(RemoteOperation::Explain {
                    candidate_sql: "SELECT 1".into(),
                    mode,
                }),
                Err(RemoteError::PermissionDenied)
            ));
            drop(remote);
            assert_eq!(
                server.join().unwrap(),
                vec![doctor_statement("SET query_timeout = 10")]
            );
        }
    }

    #[test]
    fn profiled_select_marks_fixed_statements_and_sends_user_sql_verbatim() {
        let query_id = "01234567-89ab-4abc-8def-0123456789ab";
        let (mut remote, server) = loopback_remote(move |mut stream| {
            let mut statements = Vec::new();
            for _ in 0..3 {
                statements.push(read_com_query(&mut stream));
                write_ok(&mut stream);
            }
            statements.push(read_com_query(&mut stream));
            stream
                .write_all(&text_result(1, vec![vec![1, b'1']]))
                .unwrap();
            statements.push(read_com_query(&mut stream));
            let mut id = vec![36];
            id.extend_from_slice(query_id.as_bytes());
            stream.write_all(&text_result(1, vec![id])).unwrap();
            statements.push(read_com_query(&mut stream));
            stream
                .write_all(&text_result(
                    1,
                    vec![vec![7, b'P', b'r', b'o', b'f', b'i', b'l', b'e']],
                ))
                .unwrap();
            stream.flush().unwrap();
            statements
        });
        let RemoteResponse::ProfiledQuery { profile, .. } = remote
            .run(RemoteOperation::ProfiledSelect {
                candidate_sql: "SELECT 1".into(),
            })
            .unwrap()
        else {
            panic!("expected profiled query response");
        };
        assert_eq!(profile, "Profile");
        drop(remote);
        assert_eq!(
            server.join().unwrap(),
            vec![
                doctor_statement("SET enable_profile = true"),
                doctor_statement("SET pipeline_profile_level = 1"),
                doctor_statement("SET query_timeout = 10"),
                "SELECT 1".to_owned(),
                doctor_statement("SELECT last_query_id()"),
                doctor_statement(&format!("SELECT get_query_profile('{query_id}')")),
            ]
        );
        assert!(doctor_statement("SET enable_profile = true").starts_with("/* sr-doctor */ SET"));
    }

    #[test]
    fn handshake_allowlist_never_advertises_local_files_or_multi_statements() {
        assert_eq!(
            CLIENT_CAPABILITY_ALLOWLIST & (CLIENT_LOCAL_FILES | CLIENT_MULTI_STATEMENTS),
            0
        );
        let response =
            handshake_response(CLIENT_CAPABILITY_ALLOWLIST, "reader", &[1; 20], None).unwrap();
        let capabilities = u32::from_le_bytes(response[..4].try_into().unwrap());
        assert_eq!(
            capabilities & (CLIENT_LOCAL_FILES | CLIENT_MULTI_STATEMENTS),
            0
        );
    }

    #[test]
    fn local_infile_and_multiple_result_packets_fail_closed() {
        let mut local = Cursor::new(packet(1, &[0xfb]));
        assert_eq!(read_text_result(&mut local), Err(RemoteError::Protocol));
        let mut more_results = Cursor::new(packet(1, &[0x00, 0, 0, 0x08, 0x00, 0, 0]));
        assert_eq!(
            read_text_result(&mut more_results),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn oversized_packet_is_rejected_before_allocation() {
        let mut bytes = vec![0xff, 0xff, 0xff, 1];
        bytes.extend_from_slice(&[0; 1]);
        let mut cursor = Cursor::new(bytes);
        let mut sequence = 1;
        assert_eq!(
            read_packet(&mut cursor, &mut ResultBudget::default(), &mut sequence),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn packet_sequence_and_continuation_packets_fail_closed_before_allocation() {
        let mut wrong_sequence = Cursor::new(packet(2, &[0x00, 0, 0, 0, 0]));
        assert_eq!(
            read_text_result(&mut wrong_sequence),
            Err(RemoteError::Protocol)
        );

        let mut bad_greeting = Cursor::new(packet(1, &[0x0a]));
        let mut greeting_sequence = 0;
        assert_eq!(
            read_packet(
                &mut bad_greeting,
                &mut ResultBudget::default(),
                &mut greeting_sequence
            ),
            Err(RemoteError::Protocol)
        );

        let mut continuation = Cursor::new(vec![0xff, 0xff, 0xff, 1]);
        let mut expected_sequence = 1;
        assert_eq!(
            read_packet(
                &mut continuation,
                &mut ResultBudget::default(),
                &mut expected_sequence
            ),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn missing_required_server_capability_fails_closed() {
        let supported = REQUIRED_SERVER_CAPABILITIES | CLIENT_SSL | CLIENT_CONNECT_WITH_DB;
        assert!(negotiate_client_capabilities(supported, TlsMode::VerifyFull, true).is_ok());
        assert_eq!(
            negotiate_client_capabilities(
                supported & !CLIENT_PLUGIN_AUTH,
                TlsMode::VerifyFull,
                false
            ),
            Err(RemoteError::Protocol)
        );
        assert_eq!(
            negotiate_client_capabilities(REQUIRED_SERVER_CAPABILITIES, TlsMode::VerifyFull, false),
            Err(RemoteError::Tls)
        );
    }

    #[test]
    fn native_password_response_is_stable_and_never_contains_password_bytes() {
        let response = mysql_native_password(b"secret", b"12345678901234567890");
        assert_eq!(response.len(), 20);
        assert_ne!(response, b"secret");
        assert!(mysql_native_password(b"", b"12345678901234567890").is_empty());
    }

    #[test]
    fn result_rows_allow_empty_and_null_cells_without_treating_them_as_ok() {
        let mut bytes = packet(1, &[2]); // two columns
        bytes.extend(packet(2, b"first"));
        bytes.extend(packet(3, b"second"));
        bytes.extend(packet(4, &[0xfe, 0, 0, 0, 0])); // field EOF
        bytes.extend(packet(5, &[0xfb, 0x00])); // SQL NULL, then empty string
        bytes.extend(packet(6, &[0xfe, 0, 0, 0, 0])); // row EOF
        assert_eq!(
            read_text_result(&mut Cursor::new(bytes)).unwrap(),
            vec![vec![String::new(), String::new()]]
        );
    }

    #[test]
    fn nullable_decoder_keeps_sql_null_distinct_from_empty_and_zero() {
        let mut bytes = packet(1, &[3]);
        bytes.extend(packet(2, b"first"));
        bytes.extend(packet(3, b"second"));
        bytes.extend(packet(4, b"third"));
        bytes.extend(packet(5, &[0xfe, 0, 0, 0, 0]));
        bytes.extend(packet(6, &[0xfb, 0x00, 1, b'0']));
        bytes.extend(packet(7, &[0xfe, 0, 0, 0, 0]));
        assert_eq!(
            read_nullable_text_result_with_policy(
                &mut Cursor::new(bytes),
                TextResultPolicy::Generic,
            )
            .unwrap(),
            vec![vec![None, Some(String::new()), Some("0".into())]]
        );
    }

    #[test]
    fn scalar_results_require_exactly_one_row_and_one_column() {
        for parse in [
            parse_current_version as fn(Vec<Vec<String>>) -> Result<String, RemoteError>,
            parse_query_profile,
        ] {
            assert_eq!(
                parse(vec![vec![String::from("value")]]),
                Ok(String::from("value"))
            );
            assert_eq!(parse(Vec::new()), Err(RemoteError::Protocol));
            assert_eq!(parse(vec![Vec::new()]), Err(RemoteError::Protocol));
            assert_eq!(
                parse(vec![
                    vec![String::from("first")],
                    vec![String::from("second")]
                ]),
                Err(RemoteError::Protocol)
            );
            assert_eq!(
                parse(vec![vec![String::from("first"), String::from("second")]]),
                Err(RemoteError::Protocol)
            );
        }
        assert_eq!(
            parse_current_version(vec![vec![String::new()]]),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn unique_empty_profile_scalar_remains_available_to_the_upper_layer() {
        assert_eq!(
            parse_query_profile(vec![vec![String::new()]]),
            Ok(String::new())
        );
    }

    #[test]
    fn explain_policy_preserves_exact_rows_and_rejects_ambiguous_cells() {
        let mut multiline = vec![6];
        multiline.extend_from_slice(b"A\n\nB\r\n");
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, vec![multiline, vec![0]])),
                TextResultPolicy::ExplainCosts,
            )
            .unwrap(),
            vec![vec![String::from("A\n\nB\r\n")], vec![String::new()],]
        );
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, Vec::new())),
                TextResultPolicy::ExplainCosts,
            )
            .unwrap(),
            Vec::<Vec<String>>::new()
        );

        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(2, vec![vec![0, 0]])),
                TextResultPolicy::ExplainCosts,
            ),
            Err(RemoteError::Protocol)
        );
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, vec![vec![0xfb]])),
                TextResultPolicy::ExplainCosts,
            ),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn official_profile_policy_preserves_text_and_rejects_non_single_column_shapes() {
        let value = "Summary\n  name: <script>\r\n  time: ?\n";
        let mut cell = vec![value.len() as u8];
        cell.extend_from_slice(value.as_bytes());
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, vec![cell, vec![0]])),
                TextResultPolicy::AnalyzeProfile,
            )
            .unwrap(),
            vec![vec![value.to_owned()], vec![String::new()]]
        );
        assert!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, vec![])),
                TextResultPolicy::AnalyzeProfile,
            )
            .unwrap()
            .is_empty()
        );
        for bytes in [
            text_result(2, vec![vec![0, 0]]),
            text_result(1, vec![vec![0xfb]]),
            text_result(1, vec![vec![1, 0xff]]),
            packet(1, &[0x00, 0, 0, 0, 0, 0, 0]),
            packet(1, &[0xfb]),
        ] {
            assert_eq!(
                read_text_result_with_policy(
                    &mut Cursor::new(bytes),
                    TextResultPolicy::AnalyzeProfile
                ),
                Err(RemoteError::Protocol)
            );
        }
    }

    #[test]
    fn official_profile_policy_has_independent_text_and_row_budgets() {
        let row = |length: usize| {
            let mut value = vec![
                0xfd,
                length as u8,
                (length >> 8) as u8,
                (length >> 16) as u8,
            ];
            value.resize(length + 4, b'x');
            value
        };
        let limit = crate::ANALYZE_PROFILE_MAX_OUTPUT_BYTES;
        let exact = read_text_result_with_policy(
            &mut Cursor::new(text_result(1, vec![row(limit)])),
            TextResultPolicy::AnalyzeProfile,
        )
        .unwrap();
        assert_eq!(exact[0][0].len(), limit);
        for rows in [
            vec![row(limit + 1)],
            vec![row(limit / 2), row(limit / 2)],
            vec![vec![0]; MAX_RESULT_ROWS + 1],
        ] {
            assert_eq!(
                read_text_result_with_policy(
                    &mut Cursor::new(text_result(1, rows)),
                    TextResultPolicy::AnalyzeProfile,
                ),
                Err(RemoteError::OutputBudgetExceeded)
            );
        }
        // The stricter policy must not silently change generic Profile budgets.
        assert!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, vec![row(limit + 1)])),
                TextResultPolicy::Generic,
            )
            .is_ok()
        );
    }

    #[test]
    fn official_profile_wire_budget_rejects_before_payload_allocation_or_read() {
        let length = crate::ANALYZE_PROFILE_MAX_OUTPUT_BYTES + 64 * 1024 + 1;
        let mut input = Cursor::new(vec![
            length as u8,
            (length >> 8) as u8,
            (length >> 16) as u8,
            1,
        ]);
        assert_eq!(
            read_text_result_with_policy(&mut input, TextResultPolicy::AnalyzeProfile),
            Err(RemoteError::OutputBudgetExceeded)
        );
        assert_eq!(input.position(), 4);
    }

    #[test]
    fn official_profile_permission_is_typed_and_multiple_results_are_fatal() {
        let mut denied = vec![0xff];
        denied.extend_from_slice(&1142_u16.to_le_bytes());
        denied.extend_from_slice(b"#42000secret_error_not_retained");
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(packet(1, &denied)),
                TextResultPolicy::AnalyzeProfile,
            ),
            Err(RemoteError::PermissionDenied)
        );
        let mut multiple = text_result(1, vec![vec![1, b'x']]);
        let len = multiple.len();
        multiple[len - 2] = SERVER_MORE_RESULTS_EXISTS as u8;
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(multiple),
                TextResultPolicy::AnalyzeProfile,
            ),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn discard_policy_consumes_rows_without_retaining_query_results() {
        let rows = (0..(MAX_RESULT_ROWS + 1)).map(|_| vec![1, b'x']).collect();
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, rows)),
                TextResultPolicy::DiscardRows,
            )
            .unwrap(),
            Vec::<Vec<String>>::new()
        );
    }

    #[test]
    fn invalid_utf8_in_any_text_row_is_rejected() {
        let invalid = vec![3, b'A', 0xff, b'B'];
        assert_eq!(
            read_text_result(&mut Cursor::new(text_result(1, vec![invalid.clone()]))),
            Err(RemoteError::Protocol)
        );
        assert_eq!(
            read_text_result_with_policy(
                &mut Cursor::new(text_result(1, vec![invalid])),
                TextResultPolicy::ExplainCosts,
            ),
            Err(RemoteError::Protocol)
        );
    }

    #[test]
    fn session_time_zone_mapping_preserves_explicit_or_system_zone() {
        let explicit = parse_session_time_zone(vec![vec![
            "Asia/Shanghai".into(),
            "Etc/UTC".into(),
            "28800".into(),
        ]])
        .unwrap();
        assert_eq!(
            explicit,
            SessionTimeZone {
                session_time_zone: "Asia/Shanghai".into(),
                system_time_zone: "Etc/UTC".into(),
                effective_time_zone: "Asia/Shanghai".into(),
                current_utc_offset_seconds: 28_800,
            }
        );

        let system = parse_session_time_zone(vec![vec![
            "SYSTEM".into(),
            "America/New_York".into(),
            "-14400".into(),
        ]])
        .unwrap();
        assert_eq!(system.session_time_zone, "SYSTEM");
        assert_eq!(system.system_time_zone, "America/New_York");
        assert_eq!(system.effective_time_zone, "America/New_York");
        assert_eq!(system.current_utc_offset_seconds, -14_400);
    }

    #[test]
    fn session_time_zone_mapping_accepts_offset_boundaries() {
        for offset in [-50_400, 50_400] {
            let evidence = parse_session_time_zone(vec![vec![
                "UTC".into(),
                "Etc/UTC".into(),
                offset.to_string(),
            ]])
            .unwrap();
            assert_eq!(evidence.current_utc_offset_seconds, offset);
        }
    }

    #[test]
    fn session_time_zone_mapping_rejects_missing_or_non_unique_rows() {
        for rows in [
            vec![],
            vec![vec!["UTC".into(), "Etc/UTC".into()]],
            vec![vec![
                "UTC".into(),
                "Etc/UTC".into(),
                "0".into(),
                "extra".into(),
            ]],
            vec![
                vec!["UTC".into(), "Etc/UTC".into(), "0".into()],
                vec!["UTC".into(), "Etc/UTC".into(), "0".into()],
            ],
        ] {
            assert_eq!(parse_session_time_zone(rows), Err(RemoteError::Protocol));
        }
    }

    #[test]
    fn session_time_zone_mapping_rejects_empty_null_or_control_names() {
        // The text-protocol decoder represents a SQL NULL cell as an empty string.
        for rows in [
            vec![vec!["".into(), "Etc/UTC".into(), "0".into()]],
            vec![vec!["UTC".into(), "".into(), "0".into()]],
            vec![vec!["UTC\u{001f}".into(), "Etc/UTC".into(), "0".into()]],
            vec![vec!["UTC".into(), "Etc/UTC\n".into(), "0".into()]],
        ] {
            assert_eq!(parse_session_time_zone(rows), Err(RemoteError::Protocol));
        }
    }

    #[test]
    fn session_time_zone_mapping_rejects_invalid_or_out_of_range_offset() {
        for offset in ["-50401", "50401", "not-a-number"] {
            assert_eq!(
                parse_session_time_zone(vec![vec!["UTC".into(), "Etc/UTC".into(), offset.into(),]]),
                Err(RemoteError::Protocol)
            );
        }
    }

    #[test]
    fn missing_database_or_table_remains_a_typed_nonfatal_server_error() {
        let error_packet = |code: u16, state: &[u8; 5]| {
            let mut payload = vec![0xff];
            payload.extend_from_slice(&code.to_le_bytes());
            payload.push(b'#');
            payload.extend_from_slice(state);
            payload
        };

        let missing_database = error_packet(5501, b"3F000");
        assert_eq!(server_error(&missing_database), RemoteError::Unsupported);
        assert_eq!(
            read_text_result(&mut Cursor::new(packet(1, &missing_database))),
            Err(RemoteError::Unsupported)
        );
        assert_eq!(
            server_error(&error_packet(5502, b"42602")),
            RemoteError::Unsupported
        );
        assert_eq!(
            server_error(&error_packet(1146, b"42S02")),
            RemoteError::Unsupported
        );
        assert_eq!(
            server_error(&error_packet(1049, b"42000")),
            RemoteError::Unsupported
        );
        assert_eq!(
            server_error(&error_packet(1045, b"28000")),
            RemoteError::Authentication
        );
        assert_eq!(
            server_error(&error_packet(5501, b"HY000")),
            RemoteError::Connection
        );
        assert_eq!(server_error(&[0xff, 0x7d]), RemoteError::Protocol);
    }

    #[test]
    fn analyze_profile_on_an_evicted_profile_is_a_typed_nonfatal_server_error() {
        let packet = |code: u16, state: &[u8; 5], message: &[u8]| {
            let mut payload = vec![0xff];
            payload.extend_from_slice(&code.to_le_bytes());
            payload.push(b'#');
            payload.extend_from_slice(state);
            payload.extend_from_slice(message);
            payload
        };
        // The exact official 3.5.14 reply for a Profile that is no longer retained.
        assert_eq!(
            server_error(&packet(1064, b"HY000", b"query not exists")),
            RemoteError::QueryProfileNotFound
        );
        // Any other 1064 (e.g. a syntax error) keeps the fail-closed default.
        assert_eq!(
            server_error(&packet(1064, b"HY000", b"Getting syntax error at line 1")),
            RemoteError::Connection
        );
        assert_eq!(
            server_error(&packet(1064, b"42000", b"query not exists")),
            RemoteError::Connection
        );
        assert_eq!(
            server_error(&packet(1064, b"HY000", b"query not exists!")),
            RemoteError::Connection
        );
    }
    #[test]
    fn audit_control_interrupts_blocked_read_and_prevents_next_com_query() {
        let (sent, received) = std::sync::mpsc::channel();
        let (mut remote, server) = loopback_remote(move |mut stream| {
            let statement = read_com_query(&mut stream);
            sent.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
            vec![statement]
        });
        let control = crate::OperationControl::new(Duration::from_secs(5));
        remote
            .set_audit_control(Some(Arc::clone(&control)))
            .unwrap();
        let worker = std::thread::spawn(move || {
            assert!(remote.query("SELECT bounded_fixture").is_err());
            assert!(matches!(
                remote.query("SELECT must_not_send"),
                Err(RemoteError::Timeout)
            ));
        });
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        control.stop();
        worker.join().unwrap();
        assert_eq!(
            server.join().unwrap(),
            vec![doctor_statement("SELECT bounded_fixture")]
        );
    }

    #[test]
    fn audit_control_interrupts_tls_stream_handshake_io() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        // Far above the wait below, so the socket timeout can never end the
        // handshake read in place of `stop()`.
        client
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let (mut server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let config = ClientConfig::builder()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
        let tls =
            ClientConnection::new(Arc::new(config), ServerName::try_from("localhost").unwrap())
                .unwrap();
        let mut remote = MysqlTextRemote {
            stream: WireStream::Tls(Box::new(StreamOwned::new(tls, client))),
            audit_control: None,
        };
        let control = crate::OperationControl::new(Duration::from_secs(5));
        remote
            .set_audit_control(Some(Arc::clone(&control)))
            .unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            assert!(remote.query("SELECT never_completed").is_err());
            assert!(matches!(
                remote.query("SELECT must_not_send"),
                Err(RemoteError::Timeout)
            ));
            done_tx.send(()).unwrap();
        });
        // Real TLS ClientHello bytes establish that worker is inside rustls socket IO.
        let mut bytes = [0; 4096];
        assert!(server.read(&mut bytes).unwrap() > 0);
        control.stop();
        // Well under the 5 s operation deadline and the 30 s socket timeout, and
        // the silent server never closes, so only `stop()` can end it.
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
