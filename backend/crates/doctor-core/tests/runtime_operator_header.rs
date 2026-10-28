use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    AdapterContext, AdapterLimits, AdapterOutput, InputEnvelope, adapt_input, v01_analysis_receipt,
};
use serde_json::json;
use std::collections::BTreeSet;

// Synthetic formatter fixtures; the dual suffix was observed in the fixed
// 3.5.14 complex profile (Task33), not inferred from SQL or business semantics.
fn profile(kind: RequestedSourceKind, headers: &[(&str, &str)]) -> String {
    if kind == RequestedSourceKind::TextProfile {
        let mut text = String::from(
            "Query:\n  Summary:\n    - Query ID: 00000000-0000-0000-0000-000000000033\n    - Query Type: Query\n    - Query State: Finished\n    - StarRocks Version: 3.5.14-23a56ec\n  Execution:\n    Fragment 0:\n      Pipeline (id=0):\n        PipelineDriver (id=0):\n",
        );
        for (header, duration) in headers {
            text.push_str(&format!(
                "          {header}:\n            CommonMetrics:\n              - OperatorTotalTime: {duration}\n"
            ));
        }
        text
    } else {
        let mut pipeline = serde_json::Map::new();
        for (header, duration) in headers {
            pipeline.insert(
                (*header).into(),
                json!({"CommonMetrics": {"OperatorTotalTime": duration}}),
            );
        }
        json!({"Query": {
            "Summary": {
                "Query ID": "00000000-0000-0000-0000-000000000033",
                "Query Type": "Query", "Query State": "Finished",
                "StarRocks Version": "3.5.14-23a56ec"
            },
            "Execution": {"Fragment 0": {"Pipeline (id=0)": pipeline}}
        }})
        .to_string()
    }
}

fn adapt(kind: RequestedSourceKind, input: &str) -> AdapterOutput {
    adapt_input(
        InputEnvelope::new(kind, input.as_bytes()),
        &AdapterLimits::default(),
        &AdapterContext::new(v01_analysis_receipt()).unwrap(),
    )
    .unwrap()
}

fn durations(output: &AdapterOutput) -> Vec<&str> {
    let mut values: Vec<_> = output
        .ir
        .metric_observations
        .iter()
        .filter(|m| m.metric_id.as_deref() == Some("OperatorTotalTime"))
        .map(|m| m.canonical_number.as_ref().unwrap().value.as_str())
        .collect();
    values.sort();
    values
}

#[test]
fn text_and_json_dual_suffix_preserve_plan_identity_and_distinct_occurrences() {
    for kind in [
        RequestedSourceKind::TextProfile,
        RequestedSourceKind::JsonProfile,
    ] {
        let input = profile(
            kind.clone(),
            &[
                ("LIMIT (plan_node_id=27) (operator id=3)", "1.315us"),
                ("LIMIT (plan_node_id=27) (operator id=4)", "2us"),
                ("PROJECT (plan_node_id=27)", "3us"),
                ("RESULT_SINK (plan_node_id=-1)", "4us"),
            ],
        );
        let output = adapt(kind.clone(), &input);
        let members = &output.ir.physical_operator_memberships;
        assert_eq!(members.len(), 4);
        assert_eq!(
            members
                .iter()
                .filter(|m| m.plan_node_id.as_deref() == Some("27"))
                .count(),
            3
        );
        assert_eq!(
            members
                .iter()
                .filter(|m| m.plan_node_id.as_deref() == Some("-1"))
                .count(),
            1
        );
        assert_eq!(
            members
                .iter()
                .map(|m| &m.physical_operator_ref)
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert!(members.iter().all(|m| !m.source_refs.is_empty()));
        assert_eq!(durations(&output), ["1315", "2000", "3000", "4000"]);
        for member in members {
            let metrics: Vec<_> = output
                .ir
                .metric_observations
                .iter()
                .filter(|m| {
                    m.runtime_occurrence_ref.as_deref()
                        == Some(member.physical_operator_ref.as_str())
                        && m.metric_id.as_deref() == Some("OperatorTotalTime")
                })
                .collect();
            assert_eq!(metrics.len(), 1);
            assert_eq!(metrics[0].value_role, "reported_base");
            assert_eq!(member.container_ref, members[0].container_ref);
        }
        for (header, value) in [
            ("LIMIT (plan_node_id=27) (operator id=3)", "1315"),
            ("LIMIT (plan_node_id=27) (operator id=4)", "2000"),
        ] {
            let source = if kind == RequestedSourceKind::TextProfile {
                let line = input
                    .lines()
                    .position(|line| line.trim() == format!("{header}:"))
                    .unwrap()
                    + 1;
                format!("text:{line}")
            } else {
                format!("json:/Query/Execution/Fragment 0/Pipeline (id=0)/{header}")
            };
            let member = members
                .iter()
                .find(|m| m.source_refs == [source.clone()])
                .unwrap();
            assert!(output.ir.metric_observations.iter().any(|m| {
                m.runtime_occurrence_ref.as_deref() == Some(member.physical_operator_ref.as_str())
                    && m.canonical_number
                        .as_ref()
                        .is_some_and(|n| n.value == value)
            }));
        }
    }
}

#[test]
fn invalid_operator_subtrees_never_leak_into_adjacent_operators_or_query() {
    let malformed = [
        "LIMIT (plan_node_id=)",
        "LIMIT (plan_node_id=x)",
        "LIMIT (plan_node_id=-2)",
        "LIMIT (plan_node_id=2147483648)",
        "LIMIT (plan_node_id=27) (operator id=)",
        "LIMIT (plan_node_id=27) (operator id=-1)",
        "LIMIT (plan_node_id=27) (operator id=x)",
        "LIMIT (plan_node_id=27) (operator id=2147483648)",
        "LIMIT (plan_node_id=27) (operator id=3) (operator id=4)",
        "LIMIT (plan_node_id=27) (unknown=3)",
        "LIMIT (plan_node_id=27) trailing",
        "LIMIT (plan_node_id=27",
        "LIMIT (operator id=3)",
        "LIMIT (plan_node_i=27)",
        "LIMIT",
    ];
    for kind in [
        RequestedSourceKind::TextProfile,
        RequestedSourceKind::JsonProfile,
    ] {
        for header in malformed {
            let input = profile(
                kind.clone(),
                &[
                    ("PROJECT (plan_node_id=1)", "1us"),
                    (header, "999us"),
                    ("PROJECT (plan_node_id=2)", "2us"),
                ],
            );
            let output = adapt(kind.clone(), &input);
            assert_eq!(durations(&output), ["1000", "2000"], "{header}");
            let members = &output.ir.physical_operator_memberships;
            assert_eq!(members.len(), 2, "{header}");
            for (plan, value) in [("1", "1000"), ("2", "2000")] {
                let group = output
                    .ir
                    .operator_groups
                    .iter()
                    .find(|g| g.plan_node_id.as_deref() == Some(plan))
                    .unwrap();
                assert!(
                    output
                        .ir
                        .metric_observations
                        .iter()
                        .any(
                            |m| m.subject_ref.as_deref() == Some(&group.operator_group_id)
                                && m.canonical_number
                                    .as_ref()
                                    .is_some_and(|n| n.value == value)
                        ),
                    "{kind:?} / {header} / plan {plan}"
                );
            }
            let diagnostics: Vec<_> = output
                .ir
                .diagnostics
                .iter()
                .filter(|d| d.code == "runtime_operator_header_invalid")
                .collect();
            assert_eq!(diagnostics.len(), 1, "{header}");
            let expected_ref = if kind == RequestedSourceKind::TextProfile {
                let line = input
                    .lines()
                    .position(|line| line.trim() == format!("{header}:"))
                    .unwrap()
                    + 1;
                format!("text:{line}")
            } else {
                format!("json:/Query/Execution/Fragment 0/Pipeline (id=0)/{header}")
            };
            assert_eq!(diagnostics[0].source_refs, [expected_ref]);
            if kind == RequestedSourceKind::TextProfile {
                assert_eq!(members[0].emission_ordinal, Some(0));
                assert_eq!(
                    members[1].emission_ordinal, None,
                    "no chain across {header}"
                );
            }
        }
    }
}

#[test]
fn text_invalid_subtree_flags_and_blank_lines_do_not_mutate_previous_operator() {
    let input = profile(
        RequestedSourceKind::TextProfile,
        &[
            ("PROJECT (plan_node_id=1)", "1us"),
            ("LIMIT (plan_node_id=x)", "999us"),
            ("PROJECT (plan_node_id=2)", "2us"),
        ],
    )
    .replace(
        "- OperatorTotalTime: 999us",
        "- IsSubordinate\n\n              - IsFinalSink\n              - OperatorTotalTime: 999us",
    );
    let output = adapt(RequestedSourceKind::TextProfile, &input);
    assert_eq!(durations(&output), ["1000", "2000"]);
    assert!(
        output
            .ir
            .physical_operator_memberships
            .iter()
            .all(|m| m.source_role != doctor_contracts::RuntimeSourceRole::Subordinate)
    );
}

#[test]
fn json_duplicate_native_identity_remains_rejected_after_normalization() {
    let input = profile(
        RequestedSourceKind::JsonProfile,
        &[
            ("LIMIT (plan_node_id=27) (operator id=3)", "1us"),
            ("LIMIT (plan_node_id=027) (operator id=03)", "2us"),
        ],
    );
    assert!(
        adapt_input(
            InputEnvelope::new(RequestedSourceKind::JsonProfile, input.as_bytes()),
            &AdapterLimits::default(),
            &AdapterContext::new(v01_analysis_receipt()).unwrap(),
        )
        .is_err()
    );
}

#[test]
fn dedented_bad_block_counters_do_not_escape_quarantine_into_query_metrics() {
    let control = profile(RequestedSourceKind::TextProfile, &[]).replace(
        "  Execution:\n",
        "  Execution:\n    - QueryExecutionWallTime: 999ms\n",
    );
    assert!(
        adapt(RequestedSourceKind::TextProfile, &control)
            .ir
            .metric_observations
            .iter()
            .any(|m| m.metric_id.as_deref() == Some("QueryExecutionWallTime")
                && m.canonical_number
                    .as_ref()
                    .is_some_and(|n| n.value == "999000000"))
    );
    for bad_counters in [
        "          - QueryExecutionWallTime: 999ms\n",
        "          CommonMetrics:\n            - QueryExecutionWallTime: 999ms\n",
        "        - QueryExecutionWallTime: 999ms\n",
    ] {
        let input = profile(
            RequestedSourceKind::TextProfile,
            &[
                ("PROJECT (plan_node_id=1)", "1us"),
                ("LIMIT (plan_node_id=x)", "999us"),
                ("PROJECT (plan_node_id=2)", "2us"),
            ],
        )
        .replace(
            "          PROJECT (plan_node_id=2):",
            &format!("{bad_counters}          PROJECT (plan_node_id=2):"),
        );
        let output = adapt(RequestedSourceKind::TextProfile, &input);
        assert_eq!(durations(&output), ["1000", "2000"]);
        assert!(
            output
                .ir
                .metric_observations
                .iter()
                .all(|m| m.metric_id.as_deref() != Some("QueryExecutionWallTime"))
        );
    }
}

#[test]
fn text_missing_header_colon_is_quarantined_and_next_driver_recovers() {
    let input = profile(
        RequestedSourceKind::TextProfile,
        &[
            ("PROJECT (plan_node_id=1)", "1us"),
            ("LIMIT (plan_node_id=27) (operator id=3)", "999us"),
            ("PROJECT (plan_node_id=2)", "2us"),
        ],
    )
    .replace(
        "LIMIT (plan_node_id=27) (operator id=3):",
        "LIMIT (plan_node_id=27) (operator id=3)",
    )
    .replace(
        "          PROJECT (plan_node_id=2):",
        "        PipelineDriver (id=1):\n          PROJECT (plan_node_id=2):",
    );
    let output = adapt(RequestedSourceKind::TextProfile, &input);
    assert_eq!(durations(&output), ["1000", "2000"]);
    let members = &output.ir.physical_operator_memberships;
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].emission_ordinal, Some(0));
    assert_eq!(members[1].emission_ordinal, Some(0));
    assert_ne!(
        members[0].driver_occurrence_ref,
        members[1].driver_occurrence_ref
    );
}
