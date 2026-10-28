//! Test infrastructure only (stage D E2E): writes an earlier baseline for one
//! official-digest template into an empty history file, so a regression of
//! the real fixture workload can raise an alert without days of collection.
//! The rows are synthetic; nothing reads StarRocks.
//!
//! Usage: seed_baseline <history.sqlite> <host:port> <digest> <catalog> <database>
//!        <engine_version> <first_hours_ago> <last_hours_ago> <query_ms> <scan_bytes>
use doctor_contracts::{AuditMetricId, AuditMetricInput};
use doctor_core::{
    AuditFingerprintStatement, CollectedExecution, aggregate_collection, history_rule_version,
};
use doctor_history::{HistoryStore, RunRecord, RunState};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [
        path,
        target,
        digest,
        catalog,
        database,
        engine,
        first,
        last,
        query_ms,
        scan,
    ] = args.as_slice()
    else {
        eprintln!(
            "usage: seed_baseline <path> <target> <digest> <catalog> <database> <engine> <first_hours_ago> <last_hours_ago> <query_ms> <scan_bytes>"
        );
        std::process::exit(2);
    };
    let (first, last): (i64, i64) = (first.parse().expect("first"), last.parse().expect("last"));
    let store = HistoryStore::open(std::path::Path::new(path)).expect("open history");
    assert!(
        store.targets().expect("targets").is_empty(),
        "the history file must be empty"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let current = now.div_euclid(3600) * 3600;
    // The same rule version the collector writes: buckets under any other
    // version are, correctly, never compared with the collected ones.
    let rule_version = history_rule_version();
    let mut written = 0;
    for ago in (last..=first).rev() {
        let start = current - ago * 3600;
        let rows: Vec<CollectedExecution> = (0..5)
            .map(|n| CollectedExecution {
                query_id: Some(format!("seed-{ago}-{n}")),
                catalog: Some(catalog.clone()),
                database: Some(database.clone()),
                native_digest: Some(digest.clone()),
                statement: AuditFingerprintStatement::NotRead,
                error: false,
                event_epoch_seconds: Some(start + 60 + n),
                metrics: vec![
                    (
                        AuditMetricId::QueryTime,
                        AuditMetricInput::from_sql_cell(Some(query_ms.clone())),
                    ),
                    (
                        AuditMetricId::ScanBytes,
                        AuditMetricInput::from_sql_cell(Some(scan.clone())),
                    ),
                ],
            })
            .collect();
        let (templates, buckets, _) = aggregate_collection(target, &rows);
        let run = RunRecord {
            started_at: start + 3600,
            slice_start: start,
            slice_end: start + 3600,
            state: RunState::Complete,
            reason: Some("seeded_for_e2e".into()),
            rows_read: rows.len() as u64,
            attributed: rows.len() as u64,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 0,
        };
        assert!(
            store
                .record_slice(
                    target,
                    engine,
                    &rule_version,
                    &run,
                    start + 3600,
                    &templates,
                    &buckets
                )
                .expect("record")
        );
        written += 1;
    }
    println!("{{\"seeded_hours\":{written}}}");
}
