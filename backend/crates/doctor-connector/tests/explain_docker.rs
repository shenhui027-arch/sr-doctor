//! Opt-in, read-only transport check against the dedicated EX-01 synthetic fixture.
//! Fixture setup is external; this test issues no DDL, writes, or query execution.

use std::sync::Arc;

use doctor_connector::{CreateConnectionRequest, MysqlTextRemoteFactory, SessionManager, TlsMode};

#[test]
#[ignore = "requires the fixed-digest EX-01 fixture and STARROCKS_EXPLAIN_SMOKE=1"]
fn loopback_explain_preserves_fresh_reference_rows_and_connection_context() {
    assert_eq!(std::env::var("STARROCKS_EXPLAIN_SMOKE").as_deref(), Ok("1"));
    let port = std::env::var("STARROCKS_PORT")
        .expect("explicit fixture port is required")
        .parse::<u16>()
        .expect("fixture port must be a u16");
    // COSTS includes live table metadata. A historical parser golden is not a
    // transport oracle after that metadata changes. Require a fresh CLI capture
    // from this same fixture; never silently regenerate the checked-in golden.
    let expected_path = std::env::var("STARROCKS_EXPLAIN_EXPECTED_TEXT")
        .expect("explicit fresh CLI reference path is required");
    assert!(std::path::Path::new(&expected_path).is_absolute());
    let expected_text = std::fs::read_to_string(expected_path)
        .expect("fresh reference must be UTF-8 without a terminal output newline");
    let expected: Vec<&str> = expected_text.split('\n').collect();
    let manager = SessionManager::new(Arc::new(MysqlTextRemoteFactory));
    let session = manager
        .connect(CreateConnectionRequest {
            host: "127.0.0.1".into(),
            port,
            username: std::env::var("STARROCKS_USER").unwrap_or_else(|_| "root".into()),
            password: std::env::var("STARROCKS_PASSWORD").unwrap_or_default(),
            default_catalog: None,
            default_database: Some("explain_fixture".into()),
            audit_table: None,
            tls_mode: TlsMode::DisabledLoopbackDocker,
        })
        .expect("the owned loopback fixture must accept the read-only session");
    // The checked-in CLI script has a statement terminator. The product accepts
    // a single SELECT without a delimiter; do not widen its SQL allowlist.
    let sql = include_str!("../../../../fixtures/explain/starrocks-3.5.14/select-multijoin.sql")
        .trim_end()
        .strip_suffix(';')
        .expect("this CLI fixture must have one terminal delimiter");
    let result = manager.explain_costs(&session.connection_id, sql);
    manager.close(&session.connection_id);
    let result = result.expect("ordinary COSTS must return the fixture plan");
    assert_eq!(result.detected_version, "3.5.14-23a56ec");
    assert_eq!(result.default_database.as_deref(), Some("explain_fixture"));
    assert_eq!(
        result.rows.len(),
        expected.len(),
        "protocol row count changed"
    );
    for (index, (actual, expected)) in result.rows.iter().zip(expected.iter()).enumerate() {
        assert!(
            actual == expected,
            "fresh reference mismatch at row {index}"
        );
    }
    println!(
        "EXPLAIN_TRANSPORT rows={} bytes={} blank_rows={} multiline_rows={} version={}",
        result.rows.len(),
        result.rows.iter().map(String::len).sum::<usize>(),
        result.rows.iter().filter(|row| row.is_empty()).count(),
        result.rows.iter().filter(|row| row.contains('\n')).count(),
        result.detected_version,
    );
}
