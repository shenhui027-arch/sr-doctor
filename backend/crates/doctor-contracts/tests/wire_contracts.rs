use doctor_contracts::{
    AccessLoginRequest, AnalysisAttempt, AnalysisSnapshotReport, BoundedInterval,
    COMPARISON_SCHEMA_VERSION, CandidateProfileResponsibility, Comparison, ConnectedContractGolden,
    ConnectedProfileComparisonRequest, ConnectedSchemaVersion, ConnectionCreateRequest,
    EXPLAIN_SCHEMA_VERSION, EXPORT_MEDIA_TYPE, ExplainCardFactSlot, ExplainFactValue,
    ExplainNodeFact, ExplainRequest, ExplainRuntimeState, ExplainSchemaVersion, ExplainSnapshot,
    ExplainSourceLocator, ExportArtifact, ExportRequest, ExportSubject, Finding, FindingConfidence,
    MetricCell, MetricCellState, MetricDelta, PackMatch, QueryDiscoverySource, RuntimeDeltaState,
    SCHEMA_VERSION, SourceKind, canonical_hash, canonical_json, explain_facts_hash_v1,
};
use schemars::schema_for;
use serde_json::json;
use ts_rs::TS;

#[test]
fn checked_in_report_is_a_public_wire_value() {
    let report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .expect("fixture must remain compatible with the public DTO");
    assert_eq!(report.schema_version, "2.0");
    assert_eq!(report.graph_view.edges.len(), 0);
    assert_eq!(report.version.pack_match, PackMatch::ExactPack);
    assert_eq!(report.facts_hash.len(), 64);
    assert_eq!(report.input.source_hash.len(), 64);
}

#[test]
fn analysis_and_comparison_schema_versions_are_independent() {
    assert_eq!(SCHEMA_VERSION, "2.0");
    assert_eq!(COMPARISON_SCHEMA_VERSION, "2.0");
}

#[test]
fn finding_confidence_is_a_closed_three_value_wire_enum() {
    for value in ["high", "medium", "low"] {
        let parsed: FindingConfidence =
            serde_json::from_value(json!(value)).expect("frozen confidence value");
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(value));
    }
    assert!(serde_json::from_value::<FindingConfidence>(json!("unknown")).is_err());

    let finding_schema = serde_json::to_value(schema_for!(Finding)).unwrap();
    assert_eq!(
        finding_schema.pointer("/properties/confidence/$ref"),
        Some(&json!("#/definitions/FindingConfidence"))
    );
    assert_eq!(
        finding_schema.pointer("/definitions/FindingConfidence/enum"),
        Some(&json!(["high", "medium", "low"]))
    );
}

#[test]
fn explain_snapshot_v04_round_trips_without_revising_the_outer_connected_shape() {
    let golden: ConnectedContractGolden = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_connected_contract_golden.json"
    ))
    .expect("connected golden follows the public DTO");
    assert_eq!(golden.schema_version, ConnectedSchemaVersion::V0_1);
    assert_eq!(EXPLAIN_SCHEMA_VERSION, "0.4");
    let snapshot = serde_json::to_value(&golden.explain_response.snapshot)
        .expect("Explain snapshot serializes");
    let round_trip: ExplainSnapshot = serde_json::from_value(snapshot.clone())
        .expect("Explain snapshot has its required v0.4 shape");
    assert_eq!(
        round_trip.explain_schema_version,
        ExplainSchemaVersion::V0_4
    );

    let mut old_shape = snapshot;
    old_shape
        .as_object_mut()
        .expect("snapshot object")
        .remove("explain_schema_version");
    assert!(serde_json::from_value::<ExplainSnapshot>(old_shape).is_err());
}

#[test]
fn explain_fact_outer_state_and_kind_value_xor_are_closed() {
    let available = json!({
        "state":"available", "fact_ref":"f", "node_ref":"n",
        "fact_kind":"plan_child_count", "evidence_kind":"verified_derived", "value":2,
        "source_refs":["explain:row:1:line:1"]
    });
    let parsed: ExplainNodeFact =
        serde_json::from_value(available.clone()).expect("available fact");
    assert!(matches!(
        parsed,
        ExplainNodeFact::Available {
            value: ExplainFactValue::PlanChildCount(2),
            ..
        }
    ));
    let mut mismatched = available;
    mismatched["fact_kind"] = json!("distribution");
    assert!(serde_json::from_value::<ExplainNodeFact>(mismatched).is_err());
    assert!(
        serde_json::from_value::<ExplainNodeFact>(json!({
            "state":"unavailable", "fact_ref":"f", "node_ref":"n", "fact_kind":"distribution",
            "source_refs":[], "reason_codes":[]
        }))
        .is_err()
    );
}

#[test]
fn explain_unavailable_fact_schema_and_typescript_require_a_reason() {
    let schema = serde_json::to_value(schema_for!(ExplainNodeFact)).expect("fact schema");
    let unavailable = schema
        .pointer("/oneOf")
        .and_then(serde_json::Value::as_array)
        .and_then(|arms| {
            arms.iter().find(|arm| {
                arm.pointer("/properties/state/enum/0")
                    .and_then(serde_json::Value::as_str)
                    == Some("unavailable")
            })
        })
        .expect("unavailable schema arm");
    assert_eq!(
        unavailable.pointer("/properties/reason_codes/minItems"),
        Some(&json!(1))
    );
    assert!(ExplainNodeFact::decl().contains("reason_codes: [string, ...string[]]"));
}

#[test]
fn explain_cardinality_wire_requires_canonical_unsigned_integer() {
    let valid = json!({
        "state":"available", "fact_ref":"f", "node_ref":"n",
        "fact_kind":"cardinality_estimate", "evidence_kind":"explicit",
        "value":{"kind":"integer","value":"123456789","scale":null},
        "source_refs":["explain:row:1:line:1"]
    });
    assert!(serde_json::from_value::<ExplainNodeFact>(valid.clone()).is_ok());
    for invalid_number in [
        json!({"kind":"decimal","value":"1","scale":null}),
        json!({"kind":"integer","value":"1","scale":0}),
        json!({"kind":"integer","value":"-1","scale":null}),
        json!({"kind":"integer","value":"1e3","scale":null}),
        json!({"kind":"integer","value":"01","scale":null}),
        json!({"kind":"integer","value":"1234567890123456789012345678901234567890","scale":null}),
    ] {
        let mut candidate = valid.clone();
        candidate["value"] = invalid_number;
        assert!(
            serde_json::from_value::<ExplainNodeFact>(candidate).is_err(),
            "invalid cardinality accepted"
        );
    }
}

#[test]
fn explain_card_slot_is_a_schema_and_typescript_discriminated_union() {
    let schema = serde_json::to_value(schema_for!(ExplainCardFactSlot)).expect("slot schema");
    assert_eq!(
        schema
            .pointer("/oneOf")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(3)
    );
    let schema_text = schema.to_string();
    for state in ["selected", "unavailable", "not_applicable"] {
        assert!(schema_text.contains(state), "missing schema arm {state}");
    }
    let declaration = ExplainCardFactSlot::decl();
    assert!(declaration.contains("state: \"selected\""));
    assert!(declaration.contains("state: \"unavailable\""));
    assert!(declaration.contains("state: \"not_applicable\""));
    assert!(declaration.contains("fact_ref: string"));
    assert!(declaration.contains("fact_ref: null"));
}

#[test]
fn generated_explain_golden_uses_the_shared_facts_projection() {
    let golden: ConnectedContractGolden = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_connected_contract_golden.json"
    ))
    .expect("connected golden");
    let snapshot = golden.explain_response.snapshot;
    assert_eq!(
        snapshot.facts_hash,
        explain_facts_hash_v1(
            &snapshot.graph_view,
            &snapshot.plan_node_fact_sets,
            &snapshot.receipt,
        )
    );
    assert_eq!(
        snapshot.facts_hash,
        "c3706008a10cba92ed9f76981fe1f3d2ce1a848f53874ff68e4ea5b43b5b53bc"
    );
}

#[test]
fn explain_row_line_locator_requires_one_based_forward_original_cell_ranges() {
    let valid = json!({
        "kind": "row_line",
        "source_ref": "explain:row:1:line:1",
        "row_no": 1,
        "row_line_no": 1,
        "global_line_no": 1,
        "byte_start": 0,
        "byte_end": 0,
    });
    assert!(serde_json::from_value::<ExplainSourceLocator>(valid.clone()).is_ok());
    for field in ["row_no", "row_line_no", "global_line_no"] {
        let mut zero = valid.clone();
        zero[field] = json!(0);
        assert!(serde_json::from_value::<ExplainSourceLocator>(zero).is_err());
    }
    let mut reverse = valid;
    reverse["byte_start"] = json!(4);
    reverse["byte_end"] = json!(3);
    assert!(serde_json::from_value::<ExplainSourceLocator>(reverse).is_err());

    let schema = serde_json::to_value(schema_for!(ExplainSourceLocator)).unwrap();
    for field in ["row_no", "row_line_no", "global_line_no"] {
        assert_eq!(
            schema.pointer(&format!("/oneOf/1/properties/{field}/minimum")),
            Some(&json!(1.0)),
            "{field} must be one-based in the wire schema"
        );
    }
}

#[test]
fn connected_profile_comparison_request_is_strict_and_carries_only_opaque_ids() {
    let request = json!({
        "connection_id": "opaque-session-id",
        "baseline_query_id": "00000000-0000-0000-0000-000000003514",
        "candidate_query_id": "00000000-0000-0000-0000-000000003515",
        "discovery_generation": 1,
    });
    assert!(serde_json::from_value::<ConnectedProfileComparisonRequest>(request.clone()).is_ok());

    let mut unknown = request;
    unknown["analysis_snapshot_report"] = json!({"forbidden": true});
    assert!(serde_json::from_value::<ConnectedProfileComparisonRequest>(unknown).is_err());
}

#[test]
fn comparison_rejects_the_analysis_schema_version() {
    let comparison = json!({
        "schema_version":"1.0", "comparison_receipt":{"components":[{"id":"comparator","version":"0.1.0","manifest_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}, "comparison_key":"key",
        "baseline":{"error":{"code":"parse_failed","stage":"parse","message_id":"parse_failed","recoverability":"change_input","affected_side":"baseline"}},
        "candidate":{"error":{"code":"parse_failed","stage":"parse","message_id":"parse_failed","recoverability":"change_input","affected_side":"candidate"}},
        "state":"not_comparable", "reasons":[], "compatibility_delta":[],
        "structure_delta":{"state":"not_comparable","matches":[],"added":[],"removed":[],"changed":[],"edge_delta":[],"unresolved":[],"runtime_operators":{"state":"not_comparable","reasons":["candidate_attempt_failed"]}},
        "metric_deltas":[], "finding_delta":{"added":[],"resolved":[],"persisting":[],"changed":[],"incomparable":[],"algorithm_changed":[]},
        "diagnostics":[], "limitations":[]
    });
    assert!(serde_json::from_value::<Comparison>(comparison.clone()).is_err());
    let mut comparison_1_1 = comparison;
    comparison_1_1["schema_version"] = json!(COMPARISON_SCHEMA_VERSION);
    assert!(serde_json::from_value::<Comparison>(comparison_1_1.clone()).is_ok());

    let mut empty_receipt = comparison_1_1.clone();
    empty_receipt["comparison_receipt"] = json!({"components":[]});
    assert!(serde_json::from_value::<Comparison>(empty_receipt).is_err());

    let mut multiple_receipt = comparison_1_1.clone();
    multiple_receipt["comparison_receipt"]["components"].as_array_mut().unwrap().push(json!({
        "id":"another", "version":"0.1.0", "manifest_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }));
    assert!(serde_json::from_value::<Comparison>(multiple_receipt).is_err());

    let mut bad_hash = comparison_1_1.clone();
    bad_hash["comparison_receipt"]["components"][0]["manifest_hash"] = json!("not-a-hash");
    assert!(serde_json::from_value::<Comparison>(bad_hash).is_err());

    comparison_1_1["future_comparison_extension"] = json!({"v":"ignored-by-current-reader"});
    assert!(serde_json::from_value::<Comparison>(comparison_1_1).is_ok());
}

#[test]
fn semantic_states_are_not_parse_or_zero_states() {
    for (wire, expected) in [
        ("unavailable", MetricCellState::Unavailable),
        ("incompatible", MetricCellState::Incompatible),
        ("unknown", MetricCellState::Unknown),
    ] {
        let state: MetricCellState = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
        assert_eq!(state, expected);
        assert_ne!(serde_json::to_string(&state).unwrap(), "\"exact\"");
    }
}

#[test]
fn task21_finding_delta_reads_legacy_but_writes_explicit_partial_evidence() {
    let legacy = json!({"added":[], "resolved":[], "persisting":[], "changed":[], "incomparable":[], "algorithm_changed":[]});
    let mut delta: doctor_contracts::FindingDelta = serde_json::from_value(legacy).unwrap();
    assert!(delta.partial_evidence.is_empty());
    assert_eq!(
        serde_json::to_value(&delta).unwrap()["partial_evidence"],
        json!([])
    );
    delta.resolved.push("SR3514.TEST:operator-0".into());
    delta.partial_evidence = delta.resolved.clone();
    let wire = serde_json::to_value(&delta).unwrap();
    assert_eq!(wire["partial_evidence"], wire["resolved"]);
    assert_eq!(
        serde_json::from_value::<doctor_contracts::FindingDelta>(wire).unwrap(),
        delta
    );
}

#[test]
fn task21_history_unavailable_requires_a_known_reason_and_stable_code() {
    for (reason, code) in [
        (
            "migration_requires_manual_resolution",
            "history_store_migration_requires_manual_resolution",
        ),
        ("io_or_permission", "history_store_io_or_permission"),
        ("other", "history_store_open_failed"),
    ] {
        let wire = json!({"state":"unavailable", "reason":reason, "code":code});
        let state: doctor_contracts::HistoryStoreState =
            serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), wire);
    }
    for invalid in [
        json!({"state":"unavailable"}),
        json!({"state":"unavailable", "reason":"internal_details", "code":"history_store_open_failed"}),
        json!({"state":"unavailable", "reason":"other", "code":"history_store_open_failed", "internal_error":"private"}),
    ] {
        assert!(serde_json::from_value::<doctor_contracts::HistoryStoreState>(invalid).is_err());
    }
}

#[test]
fn canonical_json_sorts_objects_and_keeps_array_order() {
    let left = json!({"z": 1, "a": ["source-second", "source-first"]});
    let right = json!({"a": ["source-second", "source-first"], "z": 1});
    assert_eq!(
        canonical_json(&left),
        "{\"a\":[\"source-second\",\"source-first\"],\"z\":1}"
    );
    assert_eq!(canonical_hash(&left), canonical_hash(&right));
}

#[test]
fn analysis_attempt_requires_exactly_one_outcome_or_error() {
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    let error = json!({
        "code": "parse_failed",
        "stage": "parse",
        "message_id": "parse_failed",
        "recoverability": "change_input",
        "affected_side": "baseline"
    });
    assert!(
        serde_json::from_value::<AnalysisAttempt>(json!({"outcome":{"report":report.clone()}}))
            .is_ok()
    );
    assert!(serde_json::from_value::<AnalysisAttempt>(json!({"error":error.clone()})).is_ok());
    assert!(serde_json::from_value::<AnalysisAttempt>(json!({})).is_err());
    assert!(
        serde_json::from_value::<AnalysisAttempt>(
            json!({"outcome":{"report":report},"error":error})
        )
        .is_err()
    );
}

#[test]
fn metric_cell_rejects_fake_zero_and_exact_without_value() {
    let common = json!({
        "metric_cell_ref":"m1", "metric_id":"QuerySpillBytes", "subject_ref":"operator-1",
        "display_text":"—", "order_rank":null, "source_refs":[], "reasons":[]
    });
    let mut missing_zero = common.clone();
    missing_zero.as_object_mut().unwrap().extend(
        json!({
            "state":"missing", "canonical_number":{"kind":"integer","value":"0","scale":null},
            "canonical_unit":null, "fidelity":null, "bounds":null
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    assert!(serde_json::from_value::<MetricCell>(missing_zero).is_err());

    let mut exact_null = common;
    exact_null.as_object_mut().unwrap().extend(json!({
        "state":"exact", "canonical_number":null, "canonical_unit":null, "fidelity":null, "bounds":null
    }).as_object().unwrap().clone());
    assert!(serde_json::from_value::<MetricCell>(exact_null).is_err());
}

#[test]
fn metric_cell_enforces_fidelity_and_bounds_matrix() {
    let bounds = json!({
        "lower":{"kind":"integer","value":"1","scale":null},
        "upper":{"kind":"integer","value":"2","scale":null},
        "lower_inclusive":true,
        "upper_inclusive":false
    });
    let cell = |fidelity: &str, bounds: serde_json::Value| {
        json!({
            "metric_cell_ref":"m1", "metric_id":"QuerySpillBytes", "subject_ref":"operator-1",
            "display_text":"1 B", "order_rank":null, "source_refs":[], "reasons":[],
            "state":"exact", "canonical_number":{"kind":"integer","value":"1","scale":null},
            "canonical_unit":"byte", "fidelity":fidelity, "bounds":bounds
        })
    };
    assert!(
        serde_json::from_value::<MetricCell>(cell("rounded", serde_json::Value::Null)).is_err()
    );
    assert!(
        serde_json::from_value::<MetricCell>(cell("truncated", serde_json::Value::Null)).is_err()
    );
    assert!(
        serde_json::from_value::<MetricCell>(cell("interval", serde_json::Value::Null)).is_err()
    );
    assert!(serde_json::from_value::<MetricCell>(cell("exact", bounds.clone())).is_err());
    assert!(serde_json::from_value::<MetricCell>(cell("unknown", bounds.clone())).is_err());
    assert!(serde_json::from_value::<MetricCell>(cell("rounded", bounds)).is_ok());
    assert!(serde_json::from_value::<MetricCell>(cell("exact", serde_json::Value::Null)).is_ok());
    assert!(serde_json::from_value::<MetricCell>(cell("unknown", serde_json::Value::Null)).is_ok());
}

#[test]
fn bounded_interval_rejects_invalid_or_empty_ranges_on_the_wire() {
    let integer = |value: &str| json!({"kind":"integer", "value":value, "scale":null});
    assert!(
        serde_json::from_value::<BoundedInterval>(json!({
            "lower": integer("2"), "upper": integer("1"),
            "lower_inclusive": true, "upper_inclusive": true
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<BoundedInterval>(json!({
            "lower": integer("1"), "upper": integer("1"),
            "lower_inclusive": true, "upper_inclusive": false
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<BoundedInterval>(json!({
            "lower": integer("not-a-number"), "upper": integer("2"),
            "lower_inclusive": true, "upper_inclusive": false
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<BoundedInterval>(json!({
            "lower": integer("1"), "upper": {"kind":"decimal", "value":"2.0", "scale":1},
            "lower_inclusive": true, "upper_inclusive": false
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<BoundedInterval>(json!({
            "lower": integer("-5"), "upper": integer("10"),
            "lower_inclusive": true, "upper_inclusive": true
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<BoundedInterval>(json!({
            "lower": integer("-1"), "upper": integer("-5"),
            "lower_inclusive": true, "upper_inclusive": true
        }))
        .is_err()
    );
}

#[test]
fn metric_delta_accepts_independent_absolute_and_percentage_intervals() {
    let mut delta = json!({
        "metric_id":"QuerySpillBytes", "subject_match":"operator-1", "state":"comparable",
        "baseline_metric_cell_ref":"b1", "candidate_metric_cell_ref":"c1",
        "absolute_delta":null,
        "percentage_state":"approximate", "percentage_value":null, "fidelity":"interval",
        "bounds":{
            "lower":{"kind":"integer","value":"-5","scale":null},
            "upper":{"kind":"integer","value":"10","scale":null},
            "lower_inclusive":true, "upper_inclusive":true
        },
        "percentage_bounds":{
            "lower":{"kind":"decimal","value":"-0.8","scale":1},
            "upper":{"kind":"decimal","value":"9.0","scale":1},
            "lower_inclusive":true, "upper_inclusive":true
        },
        "order_rank":0, "limitations":[]
    });
    assert!(serde_json::from_value::<MetricDelta>(delta.clone()).is_ok());
    let mut partial_delta = delta.clone();
    partial_delta["state"] = json!("partially_comparable");
    assert!(serde_json::from_value::<MetricDelta>(partial_delta).is_err());
    delta["future_metric_delta_extension"] = json!("ignored-by-current-reader");
    assert!(serde_json::from_value::<MetricDelta>(delta).is_ok());
}

#[test]
fn metric_delta_rejects_mixed_absolute_and_percentage_modes() {
    let base = json!({
        "metric_id":"QuerySpillBytes", "subject_match":"operator-1", "state":"comparable",
        "baseline_metric_cell_ref":"b1", "candidate_metric_cell_ref":"c1",
        "absolute_delta":{"kind":"integer","value":"2","scale":null},
        "percentage_state":"computed", "percentage_value":{"kind":"integer","value":"5","scale":null},
        "fidelity":"exact", "bounds":null, "percentage_bounds":null,
        "order_rank":0, "limitations":[]
    });
    assert!(serde_json::from_value::<MetricDelta>(base.clone()).is_ok());

    let mut exact_with_absolute_bounds = base.clone();
    exact_with_absolute_bounds["bounds"] = json!({
        "lower":{"kind":"integer","value":"1","scale":null},
        "upper":{"kind":"integer","value":"2","scale":null},
        "lower_inclusive":true, "upper_inclusive":true
    });
    assert!(serde_json::from_value::<MetricDelta>(exact_with_absolute_bounds).is_err());

    let mut computed_with_percentage_bounds = base.clone();
    computed_with_percentage_bounds["percentage_bounds"] = json!({
        "lower":{"kind":"integer","value":"1","scale":null},
        "upper":{"kind":"integer","value":"2","scale":null},
        "lower_inclusive":true, "upper_inclusive":true
    });
    assert!(serde_json::from_value::<MetricDelta>(computed_with_percentage_bounds).is_err());

    let mut missing_with_value = base;
    missing_with_value["absolute_delta"] = serde_json::Value::Null;
    missing_with_value["fidelity"] = serde_json::Value::Null;
    missing_with_value["percentage_state"] = json!("missing");
    assert!(serde_json::from_value::<MetricDelta>(missing_with_value).is_err());
}

#[test]
fn connected_golden_keeps_explain_plan_only_and_candidate_rerun_external() {
    let golden: ConnectedContractGolden = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_connected_contract_golden.json"
    ))
    .expect("connected fixture must remain compatible with the public DTO");

    assert_eq!(golden.schema_version, ConnectedSchemaVersion::V0_1);
    assert_eq!(
        golden.explain_response.snapshot.runtime_state,
        ExplainRuntimeState::Unavailable
    );
    assert_eq!(
        golden.plan_comparison.runtime_delta_state,
        RuntimeDeltaState::NotComparableMissingRuntimeProfile
    );
    assert_eq!(
        golden.experiment.candidate_profile_responsibility,
        CandidateProfileResponsibility::UserExternalRerun
    );
    assert_eq!(
        golden.profile_fetch_response.source_kind,
        SourceKind::TextProfile
    );
    assert_eq!(
        golden.profile_comparison_request.connection_id,
        golden.connection.connection_id
    );
    assert_ne!(
        golden.profile_comparison_request.baseline_query_id,
        golden.profile_comparison_request.candidate_query_id
    );
    assert_eq!(
        golden.profile_comparison_response.baseline.input_origin,
        doctor_contracts::InputOrigin::ConnectedProfileFetch
    );
    assert_eq!(
        golden.profile_comparison_response.candidate.input_origin,
        doctor_contracts::InputOrigin::CandidateProfileUserExternalRerun
    );
    assert!(matches!(
        golden.profile_comparison_response.comparison.baseline,
        AnalysisAttempt::Outcome(_)
    ));
    assert!(matches!(
        golden.profile_comparison_response.comparison.candidate,
        AnalysisAttempt::Outcome(_)
    ));
    assert_eq!(
        golden
            .profile_comparison_response
            .comparison
            .comparison_key
            .len(),
        64
    );
    assert_eq!(
        golden.plan_comparison.correlation.baseline_source_kind,
        SourceKind::TextProfile
    );
    assert_eq!(golden.query_response.source, QueryDiscoverySource::Audit);
    assert!(
        !serde_json::to_string(&golden.query_response)
            .expect("query response serializes")
            .contains("\"sql\"")
    );
    assert_eq!(
        golden.query_sql_request.query_id,
        golden.query_response.queries[0].query_id
    );
    assert_eq!(
        golden.query_sql_response.sql,
        "SELECT synthetic_query FROM demo"
    );
    assert_eq!(
        golden.query_response.queries[0].observed_at_utc.as_deref(),
        Some("2026-08-30T00:00:00Z")
    );
    assert_eq!(
        golden
            .connection
            .time_zone
            .as_ref()
            .map(|zone| zone.effective_time_zone.as_str()),
        Some("Etc/UTC")
    );
    assert!(golden.query_response.limitations.iter().any(
        |item| item == "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch"
    ));
    assert_eq!(
        golden.profile_fetch_response.source_hash,
        "3aeb9a126d70be2f999f4bca2929cb7207542b9f6d1e2cd21f51f163e7db6c9b"
    );
    let analysis_key = golden.profile_fetch_response.analysis_key.as_str();
    assert_eq!(analysis_key.len(), 64);
    assert!(analysis_key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let fetched_query = &golden.profile_fetch_response.query;
    assert_eq!(
        fetched_query.discovery_source,
        Some(QueryDiscoverySource::Audit)
    );
    assert_eq!(fetched_query.discovery_generation, Some(1));
    assert_eq!(
        fetched_query.observed_at_utc.as_deref(),
        Some("2026-08-30T00:00:00Z")
    );
    assert_eq!(fetched_query.status, "finished");
    assert_eq!(
        fetched_query.digest.as_deref(),
        Some("sha256:synthetic-query-digest")
    );
    assert_eq!(fetched_query.database.as_deref(), Some("demo"));
    assert!(fetched_query.resource_group.is_none());
    assert_eq!(fetched_query.query_time_ms, Some(500));
    assert!(
        fetched_query
            .source_refs
            .contains(&"profile:query_id".to_owned())
    );
    let fetched_report = &golden.profile_fetch_response.outcome.report;
    assert_eq!(
        &golden.profile_fetch_response.source_hash,
        &fetched_report.input.source_hash
    );
    assert_eq!(
        &golden.profile_fetch_response.analysis_key,
        &fetched_report.analysis_key
    );
    assert_eq!(
        &golden.profile_fetch_response.analysis_receipt,
        &fetched_report.analysis_receipt
    );
    assert_eq!(
        &golden.profile_fetch_response.source_kind,
        &fetched_report.input.detected_kind
    );
    assert!(golden.experiment.plan_comparison.is_some());

    let serialized = serde_json::to_string(&golden).expect("golden serializes");
    for forbidden in [
        "\"password\"",
        "\"username\"",
        "\"candidate_sql\"",
        "\"execution_receipt\"",
        "\"profile_text\"",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "connected golden leaked {forbidden}"
        );
    }

    let mut plan =
        serde_json::to_value(&golden.plan_comparison).expect("plan comparison serializes");
    plan.as_object_mut()
        .expect("plan comparison object")
        .insert(
            "execution_receipt".to_owned(),
            json!({"status":"forbidden"}),
        );
    assert!(serde_json::from_value::<doctor_contracts::PlanComparison>(plan).is_err());

    let mut wrong_plan_version =
        serde_json::to_value(&golden.plan_comparison).expect("plan comparison serializes");
    wrong_plan_version["schema_version"] = json!("0.2");
    assert!(
        serde_json::from_value::<doctor_contracts::PlanComparison>(wrong_plan_version).is_err()
    );

    let mut wrong_golden_version = serde_json::to_value(&golden).expect("golden serializes");
    wrong_golden_version["schema_version"] = json!("0.2");
    assert!(serde_json::from_value::<ConnectedContractGolden>(wrong_golden_version).is_err());
}

#[test]
fn fetched_profile_success_cannot_omit_or_null_its_analysis_evidence() {
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_connected_contract_golden.json"
    ))
    .expect("connected fixture is JSON");
    let response = golden["profile_fetch_response"].clone();

    for field in ["source_hash", "analysis_key", "analysis_receipt", "outcome"] {
        let mut missing = response.clone();
        missing
            .as_object_mut()
            .expect("response is an object")
            .remove(field);
        assert!(
            serde_json::from_value::<doctor_contracts::ProfileFetchResponse>(missing).is_err(),
            "success response accepted missing {field}"
        );

        let mut null = response.clone();
        null[field] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<doctor_contracts::ProfileFetchResponse>(null).is_err(),
            "success response accepted null {field}"
        );
    }

    let mut unavailable = response;
    unavailable["state"] = json!("unavailable");
    assert!(
        serde_json::from_value::<doctor_contracts::ProfileFetchResponse>(unavailable).is_err(),
        "failure states must use a non-2xx ErrorEnvelope"
    );
}

#[test]
fn write_only_connection_and_explain_inputs_never_echo() {
    let connection = ConnectionCreateRequest {
        host: "127.0.0.1".to_owned(),
        port: 9030,
        username: "workload_user".to_owned(),
        password: "do-not-echo".to_owned(),
        catalog: None,
        database: None,
        audit_table: None,
        tls_mode: doctor_contracts::TlsMode::DockerLoopbackPlaintext,
    };
    let explain = ExplainRequest::UserSql {
        connection_id: "conn_opaque_3514".to_owned(),
        mode: doctor_contracts::ExplainMode::Costs,
        candidate_sql: "select secret".to_owned(),
    };
    assert_eq!(
        format!("{connection:?}"),
        "ConnectionCreateRequest { redacted: true }"
    );
    assert_eq!(format!("{explain:?}"), "ExplainRequest { redacted: true }");
    let connection_json =
        serde_json::to_value(connection).expect("connection request serializes safely");
    let explain_json = serde_json::to_value(explain).expect("explain request serializes safely");
    assert!(connection_json.get("username").is_none() && connection_json.get("password").is_none());
    assert!(explain_json.get("candidate_sql").is_none());
    let login = AccessLoginRequest {
        username: "admin".to_owned(),
        password: "do-not-echo-password".to_owned(),
    };
    assert_eq!(
        format!("{login:?}"),
        "AccessLoginRequest { redacted: true }"
    );
    let login_json = serde_json::to_value(login).expect("login request serializes safely");
    assert!(login_json.get("username").is_none() && login_json.get("password").is_none());

    let connection_schema = serde_json::to_value(schema_for!(ConnectionCreateRequest))
        .expect("connection schema serializes");
    let explain_schema =
        serde_json::to_value(schema_for!(ExplainRequest)).expect("explain schema serializes");
    assert_eq!(
        connection_schema.pointer("/properties/username/writeOnly"),
        Some(&json!(true))
    );
    assert_eq!(
        connection_schema.pointer("/properties/password/writeOnly"),
        Some(&json!(true))
    );
    assert_eq!(
        explain_schema.pointer("/oneOf/0/properties/candidate_sql/writeOnly"),
        Some(&json!(true))
    );

    let reference = json!({
        "input_kind": "plan_sql_reference",
        "connection_id": "conn_opaque_3514",
        "query_id": "00000000-0000-0000-0000-000000003514",
        "discovery_generation": 7,
        "plan_sql_ref": "plan-000000000000000000000000000000000000000000000000"
    });
    assert!(serde_json::from_value::<ExplainRequest>(reference.clone()).is_ok());
    let mut reference_with_sql = reference;
    reference_with_sql["candidate_sql"] = json!("select 1");
    assert!(serde_json::from_value::<ExplainRequest>(reference_with_sql).is_err());
    assert!(
        serde_json::from_value::<ExplainRequest>(json!({
            "input_kind": "unknown",
            "connection_id": "conn_opaque_3514"
        }))
        .is_err()
    );
}

#[test]
fn export_contracts_keep_tagged_subjects_and_failed_attempt_hashes_strict() {
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    let request = json!({"kind":"analysis", "snapshot":report.clone()});
    assert!(serde_json::from_value::<ExportRequest>(request.clone()).is_ok());
    let mut extra_request = request;
    extra_request["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ExportRequest>(extra_request).is_err());
    assert!(
        serde_json::from_value::<ExportSubject>(json!({"kind":"analysis", "snapshot":report}))
            .is_ok()
    );

    let failed_hash = json!({"kind":"comparison", "comparison_key":"cmp", "baseline":{
        "state":"success", "analysis_key":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "facts_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }, "candidate":{"state":"failed", "error_state":"parse_failed"}});
    let mut failed_with_value = failed_hash.clone();
    failed_with_value["candidate"]["facts_hash"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    assert!(serde_json::from_value::<doctor_contracts::SubjectHashes>(failed_hash).is_ok());
    assert!(serde_json::from_value::<doctor_contracts::SubjectHashes>(failed_with_value).is_err());

    let analysis: ExportArtifact = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_export_analysis.json"
    ))
    .expect("analysis export golden follows the public DTO");
    assert_eq!(analysis.manifest.files.len(), 1);
    assert_eq!(analysis.manifest.files[0].path, "report.json");
    assert_eq!(analysis.manifest.files[0].media_type, EXPORT_MEDIA_TYPE);
    let comparison: ExportArtifact = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_export_comparison.json"
    ))
    .expect("comparison export golden follows the public DTO");
    match comparison.subject {
        doctor_contracts::ExportSubject::Comparison { snapshot } => {
            assert!(matches!(snapshot.candidate, AnalysisAttempt::Error(_)));
        }
        doctor_contracts::ExportSubject::Analysis { .. } => {
            panic!("comparison golden lost its tagged subject")
        }
    }
}
