use doctor_contracts::{
    AuditActiveAtReadRequest, AuditBoundedSourceCoverage, AuditBoundedSourcePartialReason,
    AuditOverviewFamilyTopSort, AuditOverviewPrimary, AuditOverviewReadRequest,
    AuditOverviewRequest, AuditScope, AuditStatementFilter, AuditWindow,
};
use serde_json::json;
use ts_rs::TS;

#[test]
fn overview_contract_has_closed_primary_and_server_concurrency_projection() {
    let request = AuditOverviewReadRequest {
        connection_id: "session-a".into(),
        request: AuditOverviewRequest {
            scope: AuditScope {
                window: AuditWindow {
                    start_seconds: 0,
                    end_seconds: 3600,
                },
                state: None,
                user: None,
                native_digest: None,
                catalog: None,
                database: None,
                resource_group: None,
                statement_filter: AuditStatementFilter::ExcludeDoctorAndNonQuery,
            },
            primary: AuditOverviewPrimary::Occurrences {
                sort: doctor_contracts::AuditOccurrenceSort::QueryTimeDesc,
                limit: 50,
            },
            family_top_sort: AuditOverviewFamilyTopSort::ExecutionCountDesc,
        },
    };
    let value = serde_json::to_value(request).unwrap();
    assert!(value["request"].get("bin_width_seconds").is_none());
    assert_eq!(value["request"]["primary"]["state"], json!("occurrences"));
    assert_eq!(
        value["request"]["scope"]["statement_filter"],
        json!("exclude_doctor_and_non_query")
    );
    let declaration = doctor_contracts::AuditOverviewResponse::decl();
    assert!(declaration.contains("concurrency: AuditConcurrencyPresentation"));
    assert!(!declaration.contains("timeline: AuditTimelinePresentation"));
    assert!(declaration.contains("family_top: AuditOverviewFamilyTopPresentation"));
    assert!(
        doctor_contracts::AuditConcurrencyPresentation::decl()
            .contains("source_coverage: AuditBoundedSourceCoverage")
    );
}

#[test]
fn active_at_contract_binds_scope_generation_and_exact_milliseconds() {
    let request = AuditActiveAtReadRequest {
        connection_id: "session-a".into(),
        discovery_generation: 7,
        scope: AuditScope {
            window: AuditWindow {
                start_seconds: 1_700_000_000,
                end_seconds: 1_700_003_600,
            },
            state: None,
            user: None,
            native_digest: None,
            catalog: None,
            database: None,
            resource_group: None,
            statement_filter: AuditStatementFilter::All,
        },
        at_milliseconds: "1700000123456".into(),
    };
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "connection_id": "session-a",
            "discovery_generation": 7,
            "scope": {
                "window": { "start_seconds": 1_700_000_000, "end_seconds": 1_700_003_600 },
                "state": null,
                "user": null,
                "native_digest": null,
                "catalog": null,
                "database": null,
                "resource_group": null,
                "statement_filter": "all",
            },
            "at_milliseconds": "1700000123456",
        })
    );
    let declaration = doctor_contracts::AuditActiveAtResponse::decl();
    assert!(declaration.contains("active_count"));
    assert!(declaration.contains("participants: AuditActiveAtParticipants"));
    assert!(declaration.contains("source_coverage: AuditBoundedSourceCoverage"));
}

#[test]
fn bounded_source_coverage_is_explicitly_partial_even_when_the_lower_bound_saturates() {
    let coverage = AuditBoundedSourceCoverage::Partial {
        reason: AuditBoundedSourcePartialReason::BoundedLookback,
        source_start_seconds: 0,
        source_end_seconds: 3_600,
        lookback_seconds: 3_600,
    };
    assert_eq!(
        serde_json::to_value(coverage).unwrap(),
        json!({
            "state": "partial",
            "reason": "bounded_lookback",
            "source_start_seconds": 0,
            "source_end_seconds": 3600,
            "lookback_seconds": 3600,
        })
    );
}
