//! Explicit opt-in smoke test for the isolated 3.5.14 Docker fixture.
//!
//! It uses the real text-protocol factory rather than an injectable fake. It
//! is ignored by default because it requires an already-provisioned local
//! StarRocks container and reads credentials only from process environment.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use doctor_connector::{
    CreateConnectionRequest, MysqlTextRemoteFactory, QuerySearch, QuerySource, SessionManager,
    TlsMode,
};

#[test]
#[ignore = "requires the isolated StarRocks 3.5.14 Docker fixture and SR_DOCTOR_SMOKE=1"]
fn loopback_docker_reads_version_and_audit_without_general_sql() {
    assert_eq!(
        std::env::var("SR_DOCTOR_SMOKE").as_deref(),
        Ok("1"),
        "set SR_DOCTOR_SMOKE=1 to make the explicit network call"
    );
    let host = std::env::var("STARROCKS_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    assert!(
        matches!(host.as_str(), "127.0.0.1" | "::1"),
        "this smoke seam permits plaintext only to a literal loopback Docker endpoint"
    );
    let port = std::env::var("STARROCKS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(39030);
    let password = std::env::var("STARROCKS_PASSWORD").unwrap_or_default();
    let username = std::env::var("STARROCKS_USER").unwrap_or_else(|_| "root".into());
    let manager = SessionManager::new(Arc::new(MysqlTextRemoteFactory));
    let session = manager
        .connect(CreateConnectionRequest {
            host,
            port,
            username,
            password,
            default_catalog: None,
            default_database: None,
            audit_table: None,
            tls_mode: TlsMode::DisabledLoopbackDocker,
        })
        .expect("loopback fixture should accept the closed read-only handshake and probes");
    assert_eq!(session.detected_version.split('-').next(), Some("3.5.14"));

    let end_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    let audit = manager
        .search(
            &session.connection_id,
            QuerySearch {
                start_seconds: end_seconds.saturating_sub(30 * 24 * 60 * 60),
                end_seconds,
                state: None,
                digest: None,
                limit: 25,
                source: QuerySource::AuditLoader,
                statement_filter: doctor_contracts::AuditStatementFilter::All,
            },
        )
        .expect("closed audit template should read the fixture rows");
    assert!(
        !audit.rows.is_empty(),
        "fixture AuditLoader rows were not visible"
    );

    if let Ok(query_id) = std::env::var("STARROCKS_SMOKE_QUERY_ID") {
        let profile = manager
            .fetch_profile(
                &session.connection_id,
                &query_id,
                audit.discovery_generation,
            )
            .expect("configured fixture query profile should be retrievable");
        assert!(!profile.into_adapter_input().content.is_empty());
    }
    manager.close(&session.connection_id);
}
