use doctor_contracts::{AuditMetricId, AuditMetricInput, AuditMetricState};
use doctor_core::{
    AuditConcurrencyDelta, AuditConcurrencyError, AuditInterval, AuditIntervalExclusion,
    MAX_AUDIT_QUERY_TIME_MS, audit_concurrency_from_deltas, audit_concurrency_steps,
    audit_interval, normalize_audit_metric,
};
use serde_json::Value;

const FIXTURE: &str = include_str!("../../../../fixtures/synthetic/audit-concurrency-v1.json");

fn text(value: &str) -> AuditMetricInput {
    AuditMetricInput::Text {
        value: value.to_owned(),
    }
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("synthetic concurrency fixture is valid JSON")
}

fn group<'a>(fixture: &'a Value, group_id: &str) -> &'a Value {
    fixture["groups"]
        .as_array()
        .expect("fixture groups")
        .iter()
        .find(|candidate| candidate["group_id"] == group_id)
        .unwrap_or_else(|| panic!("missing fixture group {group_id}"))
}

fn events(group: &Value) -> &[Value] {
    group["events"].as_array().expect("group events")
}

fn event<'a>(group: &'a Value, event_id: &str) -> &'a Value {
    events(group)
        .iter()
        .find(|candidate| candidate["id"] == event_id)
        .unwrap_or_else(|| panic!("missing event {event_id}"))
}

fn string_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value
        .as_str()
        .or_else(|| value.get(field).and_then(Value::as_str))
        .unwrap_or_else(|| panic!("{field} must be decimal text"))
}

fn decimal(value: &Value, field: &str) -> i128 {
    string_field(value, field)
        .parse::<i128>()
        .unwrap_or_else(|error| panic!("{field} must be an i128 decimal: {error}"))
}

fn optional_start(event: &Value) -> Option<i128> {
    event["recorded_start_ms"]
        .as_str()
        .and_then(|value| value.parse::<i128>().ok())
}

fn metric_input(event: &Value) -> AuditMetricInput {
    event["duration_ms"]
        .as_str()
        .map(text)
        .unwrap_or(AuditMetricInput::SqlNull)
}

fn expected_ids(group: &Value, field: &str) -> Vec<String> {
    group["expected"][field]
        .as_array()
        .unwrap_or_else(|| panic!("expected.{field} must be an array"))
        .iter()
        .map(|value| string_field(value, &format!("expected.{field}[]")).to_owned())
        .collect()
}

fn expected_count(group: &Value, field: &str) -> i128 {
    decimal(
        &group["expected"]["counts"][field],
        &format!("expected.counts.{field}"),
    )
}

fn interval_records(group: &Value) -> Vec<(String, AuditInterval)> {
    events(group)
        .iter()
        .filter(|candidate| candidate["state"] == "available")
        .filter_map(|candidate| {
            let id = string_field(candidate, "id").to_owned();
            match audit_interval(optional_start(candidate), true, metric_input(candidate)) {
                Ok(interval) => Some((id, interval)),
                Err(AuditIntervalExclusion::ZeroDuration) => None,
                Err(exclusion) => {
                    panic!("available event {id} unexpectedly excluded: {exclusion:?}")
                }
            }
        })
        .collect()
}

fn step_at_or_before(steps: &[doctor_core::AuditConcurrencyStep], at_ms: i128) -> i128 {
    steps
        .iter()
        .rev()
        .find(|step| step.at_ms <= at_ms)
        .unwrap_or_else(|| panic!("missing step at or before {at_ms}"))
        .active_count
}

#[test]
fn fixture_intervals_match_manual_endpoints_and_sets() {
    let fixture = fixture();
    for group_id in [
        "cross-window-endpoint-zero",
        "latest-tail-unknown",
        "typed-exclusion-bigint",
        "figma-window-population-mock",
    ] {
        let group = group(&fixture, group_id);
        let mut intervals = Vec::new();
        for candidate in events(group)
            .iter()
            .filter(|candidate| candidate["state"] == "available")
        {
            let id = string_field(candidate, "id");
            let start = optional_start(candidate).expect("available start is decoded");
            let duration = decimal(candidate, "duration_ms");
            let expected_end = decimal(&group["expected"]["end_ms"][id], "expected.end_ms");
            assert_eq!(
                start + duration,
                expected_end,
                "{group_id}.{id} manual endpoint"
            );
            match audit_interval(Some(start), true, metric_input(candidate)) {
                Ok(interval) => {
                    assert_eq!(
                        interval.end_ms, expected_end,
                        "{group_id}.{id} core endpoint"
                    );
                    intervals.push((id.to_owned(), interval));
                }
                Err(AuditIntervalExclusion::ZeroDuration) => {
                    assert!(
                        expected_ids(group, "zero_duration_ids")
                            .iter()
                            .any(|zero_id| zero_id == id),
                        "{group_id}.{id} zero duration must be hand-listed"
                    );
                }
                Err(exclusion) => panic!("{group_id}.{id} unexpectedly excluded: {exclusion:?}"),
            }
        }

        assert_eq!(
            events(group)
                .iter()
                .filter(|candidate| candidate["state"] == "available")
                .count() as i128,
            expected_count(group, "available_records"),
            "{group_id} available record count"
        );
        assert_eq!(
            intervals.len() as i128,
            expected_count(group, "known_nonzero_intervals"),
            "{group_id} known nonzero count"
        );
        let window_start = decimal(&group["window"]["start_ms"], "window.start_ms");
        let window_end = decimal(&group["window"]["end_ms"], "window.end_ms");
        let overlap_ids = intervals
            .iter()
            .filter(|(_, interval)| interval.overlaps(window_start, window_end))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            overlap_ids,
            expected_ids(group, "overlap_ids"),
            "{group_id} overlap"
        );
        for (point, expected_active) in group["expected"]["active_at"]
            .as_object()
            .expect("expected.active_at object")
        {
            let point = point
                .parse::<i128>()
                .unwrap_or_else(|error| panic!("active point is decimal: {error}"));
            let actual = intervals
                .iter()
                .filter(|(_, interval)| interval.active_at(point))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let expected = expected_active
                .as_array()
                .expect("active_at ids")
                .iter()
                .map(|id| string_field(id, "active_at[]").to_owned())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{group_id} active at {point}");
        }
    }
}

#[test]
fn interval_steps_match_manual_counts_and_omit_window_right_endpoint() {
    let fixture = fixture();
    for group_id in [
        "cross-window-endpoint-zero",
        "latest-tail-unknown",
        "typed-exclusion-bigint",
        "figma-window-population-mock",
    ] {
        let group = group(&fixture, group_id);
        let intervals = interval_records(group);
        let window_start = decimal(&group["window"]["start_ms"], "window.start_ms");
        let window_end = decimal(&group["window"]["end_ms"], "window.end_ms");
        let steps = audit_concurrency_steps(
            window_start,
            window_end,
            &intervals
                .iter()
                .map(|(_, interval)| interval.clone())
                .collect::<Vec<_>>(),
            64,
        )
        .unwrap_or_else(|error| panic!("{group_id} steps failed: {error:?}"));
        assert_eq!(steps.first().map(|step| step.at_ms), Some(window_start));
        assert!(steps.iter().all(|step| step.at_ms < window_end));
        for (point, expected_active) in group["expected"]["active_at"]
            .as_object()
            .expect("expected.active_at object")
        {
            let point = point.parse::<i128>().expect("active point is decimal");
            if point < window_end {
                let expected_count = expected_active.as_array().expect("active ids").len() as i128;
                assert_eq!(
                    step_at_or_before(&steps, point),
                    expected_count,
                    "{group_id} step count at {point}"
                );
            }
        }
    }
}

#[test]
fn typed_unknown_zero_and_invalid_fixture_states_remain_excluded() {
    let fixture = fixture();
    let latest = group(&fixture, "latest-tail-unknown");
    for event_id in ["latest-unfinished", "latest-not-ingested"] {
        let candidate = event(latest, event_id);
        assert_eq!(
            audit_interval(optional_start(candidate), false, metric_input(candidate)),
            Err(AuditIntervalExclusion::NotTerminal),
            "{event_id} remains unknown"
        );
    }
    assert_eq!(expected_count(latest, "unknown_records"), 2);
    assert_eq!(interval_records(latest).len() as i128, 2);

    let typed = group(&fixture, "typed-exclusion-bigint");
    assert_eq!(
        audit_interval(
            optional_start(event(typed, "missing-duration")),
            true,
            metric_input(event(typed, "missing-duration")),
        ),
        Err(AuditIntervalExclusion::QueryTime(AuditMetricState::Missing))
    );
    assert_eq!(
        audit_interval(
            optional_start(event(typed, "negative-duration")),
            true,
            metric_input(event(typed, "negative-duration")),
        ),
        Err(AuditIntervalExclusion::QueryTime(
            AuditMetricState::NegativeSentinel
        ))
    );
    assert!(
        event(typed, "invalid-start")["recorded_start_ms"]
            .as_str()
            .expect("invalid start text")
            .parse::<i128>()
            .is_err()
    );
    assert_eq!(
        audit_interval(None, true, text("2")),
        Err(AuditIntervalExclusion::TimestampMissing)
    );
    assert_eq!(
        audit_interval(
            optional_start(event(typed, "overflow-duration")),
            true,
            metric_input(event(typed, "overflow-duration")),
        ),
        Err(AuditIntervalExclusion::QueryTime(
            AuditMetricState::Overflow
        ))
    );
    assert_eq!(
        audit_interval(Some(1), true, text("not-a-decimal")),
        Err(AuditIntervalExclusion::QueryTime(AuditMetricState::Invalid))
    );
    for event_id in ["duplicate-record", "conflicting-record"] {
        let candidate = event(typed, event_id);
        assert_ne!(candidate["state"], "available");
        assert!(candidate["exclusion_reason"].as_str().is_some());
    }
    assert_eq!(expected_count(typed, "typed_excluded_records"), 6);
    assert_eq!(interval_records(typed).len() as i128, 2);
}

#[test]
fn query_time_precision_and_parser_boundary_use_canonical_ns() {
    let maximum = MAX_AUDIT_QUERY_TIME_MS;
    let maximum_text = maximum.to_string();
    let fact = normalize_audit_metric(AuditMetricId::QueryTime, text(&maximum_text));
    assert_eq!(fact.state, AuditMetricState::PrecisionUnsafe);
    assert_eq!(fact.canonical_unit.as_deref(), Some("ns"));
    assert_eq!(
        fact.canonical_number
            .as_ref()
            .map(|number| number.value.as_str()),
        Some("9223372036854000000")
    );
    let interval =
        audit_interval(Some(0), true, text(&maximum_text)).expect("max query time is valid");
    assert_eq!(interval.end_ms, i128::from(maximum));

    let next = (maximum + 1).to_string();
    let next_fact = normalize_audit_metric(AuditMetricId::QueryTime, text(&next));
    assert_eq!(next_fact.state, AuditMetricState::Overflow);
    assert_eq!(
        audit_interval(Some(0), true, text(&next)),
        Err(AuditIntervalExclusion::QueryTime(
            AuditMetricState::Overflow
        ))
    );
}

#[test]
fn interval_rejects_negative_start_and_i128_endpoint_overflow() {
    assert_eq!(
        audit_interval(Some(-1), true, text("1")),
        Err(AuditIntervalExclusion::TimestampInvalid)
    );
    assert_eq!(
        audit_interval(Some(i128::MAX), true, text("1")),
        Err(AuditIntervalExclusion::EndpointOverflow)
    );
}

#[test]
fn source_prefix_deltas_match_hand_oracle_and_keep_same_time_net_zero() {
    let fixture = fixture();
    let figma = group(&fixture, "figma-window-population-mock");
    let deltas = [
        AuditConcurrencyDelta {
            at_ms: 2_100_000,
            delta: 1,
        },
        AuditConcurrencyDelta {
            at_ms: 2_700_000,
            delta: 1,
        },
        AuditConcurrencyDelta {
            at_ms: 3_000_000,
            delta: 1,
        },
        AuditConcurrencyDelta {
            at_ms: 3_300_000,
            delta: 0,
        },
        AuditConcurrencyDelta {
            at_ms: 3_600_000,
            delta: -1,
        },
        AuditConcurrencyDelta {
            at_ms: 3_900_000,
            delta: -1,
        },
        AuditConcurrencyDelta {
            at_ms: 4_200_000,
            delta: 0,
        },
        AuditConcurrencyDelta {
            at_ms: 4_500_000,
            delta: -1,
        },
        AuditConcurrencyDelta {
            at_ms: 4_800_000,
            delta: 0,
        },
        AuditConcurrencyDelta {
            at_ms: 5_100_000,
            delta: -1,
        },
        AuditConcurrencyDelta {
            at_ms: 5_220_000,
            delta: 1,
        },
    ];
    let steps = audit_concurrency_from_deltas(1_800_000, 5_400_000, 1, &deltas, 12)
        .expect("Figma source-prefix deltas");
    assert_eq!(
        steps
            .iter()
            .map(|step| (step.at_ms, step.active_count))
            .collect::<Vec<_>>(),
        vec![
            (1_800_000, 1),
            (2_100_000, 2),
            (2_700_000, 3),
            (3_000_000, 4),
            (3_300_000, 4),
            (3_600_000, 3),
            (3_900_000, 2),
            (4_200_000, 2),
            (4_500_000, 1),
            (4_800_000, 1),
            (5_100_000, 0),
            (5_220_000, 1),
        ]
    );
    assert!(steps.iter().all(|step| step.at_ms < 5_400_000));
    let expected_at_1352 = figma["expected"]["active_at"]["3120000"]
        .as_array()
        .expect("Figma 13:52 active ids")
        .len() as i128;
    assert_eq!(step_at_or_before(&steps, 3_120_000), expected_at_1352);
}

#[test]
fn source_prefix_deltas_reject_invalid_transitions_and_budget_without_truncation() {
    let duplicate_time = [
        AuditConcurrencyDelta {
            at_ms: 150,
            delta: 1,
        },
        AuditConcurrencyDelta {
            at_ms: 150,
            delta: -1,
        },
    ];
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, 0, &duplicate_time, 4),
        Err(AuditConcurrencyError::InvalidSourceTransitions)
    );
    let unsorted = [
        AuditConcurrencyDelta {
            at_ms: 160,
            delta: 1,
        },
        AuditConcurrencyDelta {
            at_ms: 150,
            delta: -1,
        },
    ];
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, 0, &unsorted, 4),
        Err(AuditConcurrencyError::InvalidSourceTransitions)
    );
    for at_ms in [100, 200, 99, 201] {
        let out_of_window = [AuditConcurrencyDelta { at_ms, delta: 1 }];
        assert_eq!(
            audit_concurrency_from_deltas(100, 200, 0, &out_of_window, 4),
            Err(AuditConcurrencyError::InvalidSourceTransitions),
            "delta at {at_ms}"
        );
    }
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, -1, &[], 1,),
        Err(AuditConcurrencyError::InvalidSourceTransitions)
    );
    let negative_count = [AuditConcurrencyDelta {
        at_ms: 150,
        delta: -1,
    }];
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, 0, &negative_count, 2),
        Err(AuditConcurrencyError::InvalidSourceTransitions)
    );
    let overflow = [AuditConcurrencyDelta {
        at_ms: 150,
        delta: 1,
    }];
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, i128::MAX, &overflow, 2),
        Err(AuditConcurrencyError::CountOverflow)
    );
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, 0, &overflow, 1),
        Err(AuditConcurrencyError::PointBudgetExceeded)
    );
    assert_eq!(
        audit_concurrency_from_deltas(100, 200, 0, &[], 0),
        Err(AuditConcurrencyError::PointBudgetExceeded)
    );
}

#[test]
fn source_prefix_counts_remain_exact_above_javascript_safe_integer() {
    let huge = 9_007_199_254_740_993_i128;
    let deltas = [AuditConcurrencyDelta {
        at_ms: 150,
        delta: 2,
    }];
    let steps = audit_concurrency_from_deltas(100, 200, huge, &deltas, 2)
        .expect("large source-prefix count");
    assert_eq!(steps[0].active_count, huge);
    assert_eq!(steps[1].active_count, huge + 2);
}

#[test]
fn empty_source_zero_is_only_a_local_projection() {
    let steps =
        audit_concurrency_from_deltas(100, 200, 0, &[], 1).expect("empty source local projection");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].at_ms, 100);
    assert_eq!(steps[0].active_count, 0);
}
