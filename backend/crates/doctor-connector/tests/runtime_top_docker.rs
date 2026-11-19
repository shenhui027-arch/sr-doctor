//! Task 76 (T76-5): opt-in check of the controlled-execution session guard
//! against the fixed-digest StarRocks 3.5.14 allin1 fixture. Driven by
//! `scripts/e2e/task76-runtime-top-guard.ps1`, which sets the global
//! `sql_dialect` / `enable_dialect_downgrade` between phases. The statements
//! run here are single read-only SELECTs; nothing is written.

use std::sync::Arc;

use doctor_connector::{
    ConnectorError, ConnectorErrorCode, CreateConnectionRequest, MysqlTextRemoteFactory,
    SessionManager, TlsMode,
};

fn connect(manager: &SessionManager) -> Result<String, ConnectorError> {
    let port = std::env::var("STARROCKS_PORT")
        .expect("explicit fixture port is required")
        .parse::<u16>()
        .expect("fixture port must be a u16");
    let session = manager.connect(CreateConnectionRequest {
        host: "127.0.0.1".into(),
        port,
        username: "root".into(),
        password: String::new(),
        default_catalog: None,
        default_database: None,
        audit_table: None,
        tls_mode: TlsMode::DisabledLoopbackDocker,
    })?;
    assert_eq!(session.detected_version.split('-').next(), Some("3.5.14"));
    Ok(session.connection_id)
}

#[test]
#[ignore = "requires the fixed-digest 3.5.14 fixture and SR_DOCTOR_T76 set by the task 76 driver"]
fn controlled_execution_pins_the_starrocks_dialect_and_restores_the_session() {
    let phase = std::env::var("SR_DOCTOR_T76").expect("SR_DOCTOR_T76 selects the phase");
    let manager = SessionManager::new(Arc::new(MysqlTextRemoteFactory));
    // Under StarRocks rules this is one statement with one string literal.
    // Under Trino rules the literal ends at `\'` and a second SELECT follows.
    // It scans the driver's fixture table so BE produces a runtime Profile.
    let split_by_trino = "SELECT k, 'a\\'; SELECT 2; --' AS v FROM t76.t";
    let explain = |error: ConnectorError| {
        format!(
            "code={:?} stage={} limitation={:?}",
            error.code, error.stage, error.limitation
        )
    };
    match phase.as_str() {
        // Default dialect, and Trino with dialect downgrade on: the snapshot is
        // readable, the run uses StarRocks rules, and the restored connection
        // is reused for every following call.
        "starrocks" | "trino_downgrade_on" => {
            let connection_id = connect(&manager)
                .unwrap_or_else(|error| panic!("read-only session: {}", explain(error)));
            for run in 0..2 {
                let executed = manager
                    .execute_profiled_select(&connection_id, split_by_trino)
                    .unwrap_or_else(|error| panic!("run {run}: {}", explain(error)));
                assert!(!executed.query_id.is_empty());
            }
            manager
                .explain_costs(&connection_id, split_by_trino)
                .unwrap_or_else(|error| panic!("plan check: {}", explain(error)));
            manager
                .execute_profiled_select(&connection_id, "SELECT k FROM t76.t")
                .unwrap_or_else(|error| panic!("after plan check: {}", explain(error)));
            for bypass in [
                "SELECT 1 INTO OUTFILE 'file:///tmp/t76'",
                "SELECT /*+ SET_VAR(query_timeout=86400) */ 1",
                "SELECT 1 -- note\r; SELECT 2",
                "SELECT `a\\` FROM t; SELECT 2 -- `",
            ] {
                let error = match manager.execute_profiled_select(&connection_id, bypass) {
                    Ok(_) => panic!("bypass executed: {bypass:?}"),
                    Err(error) => error,
                };
                assert_eq!(error.code, ConnectorErrorCode::StatementNotAllowed);
            }
            println!("T76-5 {phase}: PASS");
        }
        // Trino with downgrade off: either the connection probes already fail
        // (no session at all), or FE cannot parse the snapshot query and the
        // run fails closed before any SET or user SQL.
        "trino_downgrade_off" => match connect(&manager) {
            Err(error) => println!("T76-5 {phase}: PASS (connect refused: {:?})", error.code),
            Ok(connection_id) => {
                let error = match manager.execute_profiled_select(&connection_id, "SELECT 1") {
                    Ok(_) => panic!("an unreadable snapshot must not run user SQL"),
                    Err(error) => error,
                };
                println!("T76-5 {phase}: PASS (run refused: {:?})", error.code);
            }
        },
        other => panic!("unknown phase {other}"),
    }
}
