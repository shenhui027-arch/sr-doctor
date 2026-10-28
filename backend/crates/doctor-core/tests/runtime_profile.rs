#![allow(clippy::result_large_err)]

use doctor_contracts::{
    AnalysisSnapshotReport, AnalysisState, GraphState, MergeState, MetricCellState, PackMatch,
    ProfileLevel, QueryState, RequestedSourceKind, RuleEvaluationStatus,
    RuntimeInventoryFormatterShape, SnapshotKind,
};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};
use sha2::{Digest, Sha256};

const LIVE_RUNTIME_PROFILE_TEXT: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/docker_information_schema_columns.profile.txt"
);
const PIPELINE_DRIVER_OCCURRENCES_PROFILE: &str = include_str!(
    "../../../../fixtures/runtime-profiles/starrocks-3.5.14/pipeline-driver-occurrences.profile.txt"
);

fn context() -> AdapterContext {
    AdapterContext::new(v01_analysis_receipt()).unwrap()
}

#[test]
fn runtime_receipt_matches_generated_contract_fixture() {
    let report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    assert_eq!(v01_analysis_receipt(), report.analysis_receipt);
}

fn adapt_text(content: &str) -> Result<doctor_core::AdapterOutput, doctor_core::AdapterError> {
    adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, content.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
}

fn output_rows_metric<'a>(
    output: &'a doctor_core::AdapterOutput,
    plan_node_id: &str,
) -> &'a doctor_core::MetricObservation {
    let group = output
        .ir
        .operator_groups
        .iter()
        .find(|group| group.plan_node_id.as_deref() == Some(plan_node_id))
        .unwrap();
    output
        .ir
        .metric_observations
        .iter()
        .find(|metric| {
            metric.subject_ref.as_deref() == Some(group.operator_group_id.as_str())
                && metric.metric_id.as_deref() == Some("OutputRows")
        })
        .unwrap()
}

fn resolved_runtime_output_before_rules() -> doctor_core::AdapterOutput {
    resolved_runtime_text_before_rules(LIVE_RUNTIME_PROFILE_TEXT)
}

fn resolved_runtime_text_before_rules(content: &str) -> doctor_core::AdapterOutput {
    let mut output = adapt_text(content).unwrap();
    let graph_evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, graph_evidence).unwrap();
    output
}

/// Official ExplainAnalyzer role: every operator without IsSubordinate is native.
fn native_occurrences(output: &doctor_core::AdapterOutput) -> std::collections::BTreeSet<String> {
    output
        .ir
        .physical_operator_memberships
        .iter()
        .filter(|membership| membership.source_role == doctor_contracts::RuntimeSourceRole::Native)
        .map(|membership| membership.physical_operator_ref.clone())
        .collect()
}

fn assert_scan_not_triggered(mut output: doctor_core::AdapterOutput) {
    evaluate_rules(&mut output).unwrap();
    assert_ne!(
        output
            .ir
            .rule_evaluations
            .iter()
            .find(|evaluation| evaluation.rule_id == "SR3514.SCAN_TIME_SHARE")
            .unwrap()
            .status,
        RuleEvaluationStatus::Triggered
    );
}

#[test]
fn runtime_profile_live_text_projects_query_metrics_operators_and_explicit_topology() {
    let output = adapt_text(LIVE_RUNTIME_PROFILE_TEXT).unwrap();
    assert_eq!(
        output
            .ir
            .version
            .reported_version
            .as_ref()
            .map(|version| version.value.as_str()),
        Some("3.5.14")
    );
    assert_eq!(
        output.ir.profile_context.profile_level,
        ProfileLevel::Level1
    );
    assert_eq!(output.ir.profile_context.snapshot_kind, SnapshotKind::Final);
    assert_eq!(output.ir.profile_context.merge_state, MergeState::Merged);
    // A valid official Topology is closed (ProfilingExecPlan.Topology), so the graph is complete.
    assert_eq!(output.ir.graph.state, GraphState::Complete);
    assert!(output.ir.graph.unresolved_relations.is_empty());
    assert_eq!(output.ir.graph.nodes.len(), 3);
    assert_eq!(output.ir.graph.edges.len(), 2);
    assert_eq!(
        output
            .ir
            .query_summary
            .as_ref()
            .and_then(|summary| summary.query_id.as_deref()),
        Some("aa562a2c-a3f3-11f1-9cad-16fcc63f18d7")
    );
    assert_eq!(output.ir.profile_context.query_state, QueryState::Finished);
    assert!(
        output
            .ir
            .derived_metric_cells
            .iter()
            .any(|metric| metric.metric_id == "QueryExecutionWallTime")
    );
    let planning = output
        .ir
        .derived_metric_cells
        .iter()
        .find(|metric| metric.metric_id == "QueryPlanningTime")
        .unwrap();
    assert_eq!(planning.state(), MetricCellState::Exact);
    assert_eq!(
        planning
            .canonical_number()
            .map(|number| number.value.as_str()),
        Some("209000000")
    );
    assert_eq!(planning.source_refs, vec!["text:19"]);
    let scan_group = output
        .ir
        .operator_groups
        .iter()
        .find(|operator| operator.plan_node_id.as_deref() == Some("0"))
        .unwrap();
    assert_eq!(scan_group.canonical_type, "SCAN");
    assert!(
        scan_group
            .raw_labels
            .iter()
            .any(|label| label == "SCHEMA_SCAN")
    );
    assert!(!scan_group.metric_observation_refs.is_empty());
    let aggregate_group = output
        .ir
        .operator_groups
        .iter()
        .find(|operator| operator.plan_node_id.as_deref() == Some("2"))
        .unwrap();
    assert_eq!(aggregate_group.canonical_type, "AGGREGATION");
    // Ordered group refs keep the unmarked encoding; the membership carries
    // the official native role (see finalize_native_operator_roles).
    assert!(aggregate_group.native_operator_refs.is_empty());
    assert_eq!(
        output
            .ir
            .operator_groups
            .iter()
            .filter(|operator| operator.plan_node_id.as_deref() == Some("2"))
            .count(),
        1
    );
    let aggregate_occurrences = output
        .ir
        .metric_observations
        .iter()
        .filter(|metric| {
            metric.subject_ref.as_deref() == Some(aggregate_group.operator_group_id.as_str())
                && metric.value_role == "reported_base"
        })
        .filter_map(|metric| metric.runtime_occurrence_ref.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(aggregate_occurrences.len(), 2);
    assert!(
        output
            .ir
            .metric_observations
            .iter()
            .filter(|metric| {
                metric.subject_ref.as_deref() == Some(aggregate_group.operator_group_id.as_str())
                    && metric.value_role == "reported_base"
            })
            .any(|metric| metric.operator_ref.is_none())
    );
    assert!(
        output
            .ir
            .metric_observations
            .iter()
            .filter(|metric| {
                metric.subject_ref.as_deref() == Some(aggregate_group.operator_group_id.as_str())
                    && metric.value_role == "reported_base"
            })
            .all(|metric| metric.source_merge_state == "unknown")
    );
    assert!(output.ir.operator_groups.iter().any(|operator| {
        operator.canonical_type == "PIPELINE"
            && operator
                .metric_observation_refs
                .iter()
                .any(|id| id.contains("ActiveTime"))
    }));

    let mut output = output;
    let graph_evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, graph_evidence).unwrap();
    evaluate_rules(&mut output).unwrap();
    assert!(output.ir.rule_evaluations.iter().all(|evaluation| {
        evaluation.subject_ref.as_deref().is_none_or(|subject_ref| {
            !subject_ref.starts_with("fragment:") && !subject_ref.starts_with("pipeline:")
        })
    }));
    let scan_evaluation = output
        .ir
        .rule_evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == "SR3514.SCAN_TIME_SHARE")
        .unwrap();
    assert_eq!(
        scan_evaluation.status,
        doctor_contracts::RuleEvaluationStatus::Triggered
    );
    assert!(scan_evaluation.missing_prerequisites.is_empty());
    assert!(scan_evaluation.applicability_checks.iter().any(|check| {
        check == "source_counter_stage:official_3514_stored_pipeline_counter_stage_complete"
    }));
    assert!(scan_evaluation.limitations.iter().any(|limitation| {
        limitation.contains("source_merge_state_unknown_not_promoted")
            && limitation.contains("verified-derived")
    }));
    assert!(
        output
            .ir
            .profile_context
            .evidence_refs
            .iter()
            .all(|reference| { scan_evaluation.evidence_refs.contains(reference) })
    );
    assert!(
        output
            .ir
            .metric_observations
            .iter()
            .filter(|observation| {
                scan_evaluation
                    .input_observation_refs
                    .contains(&observation.observation_id)
            })
            .all(|observation| observation.source_merge_state == "unknown")
    );
    let scan_finding = output
        .ir
        .findings
        .iter()
        .find(|finding| finding.rule_id == "SR3514.SCAN_TIME_SHARE")
        .unwrap();
    assert_eq!(
        scan_finding.confidence,
        doctor_contracts::FindingConfidence::Medium
    );
    let aggregate_evaluation = output
        .ir
        .rule_evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == "SR3514.AGGREGATE_TIME_SHARE")
        .unwrap();
    assert!(
        !aggregate_evaluation
            .missing_prerequisites
            .iter()
            .any(|reason| reason == "matching_operator_group_missing")
    );
    let report = assemble_public_report(output).unwrap();
    assert_eq!(
        report
            .query_summary
            .as_ref()
            .and_then(|summary| summary.query_type.as_deref()),
        Some("Query")
    );
    assert!(
        report
            .derived_metric_cells
            .iter()
            .any(|metric| metric.state() == MetricCellState::Exact)
    );
    assert!(report.operator_rows.iter().all(|operator| {
        operator.operator_type != "PIPELINE" && operator.operator_type != "FRAGMENT"
    }));
    assert!(
        report.operator_rows.iter().any(|operator| {
            operator.operator_type == "SCAN" && !operator.metric_cells.is_empty()
        })
    );
    assert_eq!(
        report
            .operator_rows
            .iter()
            .filter(|operator| operator.plan_node_id.as_deref() == Some("2"))
            .count(),
        1
    );
    assert!(report.findings.iter().any(|finding| {
        finding.rule_id == "SR3514.SCAN_TIME_SHARE"
            && !finding.evidence.is_empty()
            && !finding.recommendation.expected_direction.is_empty()
            && !finding.recommendation.validation_steps.is_empty()
    }));
    let safe_view = report
        .source_view
        .plain_text_lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for canary in ["SELECT", "root", "127.0.0.1", "Default Db"] {
        assert!(!safe_view.contains(canary), "safe view leaked {canary}");
    }
}

#[test]
fn official_stored_pipeline_source_stage_proof_fails_closed() {
    let baseline = resolved_runtime_output_before_rules();

    let mut level_two = baseline.clone();
    level_two.ir.profile_context.profile_level = ProfileLevel::Level2;
    assert_scan_not_triggered(level_two);

    let mut running = baseline.clone();
    running.ir.profile_context.query_state = QueryState::Running;
    assert_scan_not_triggered(running);

    let mut runtime_snapshot = baseline.clone();
    runtime_snapshot.ir.profile_context.snapshot_kind = SnapshotKind::Runtime;
    assert_scan_not_triggered(runtime_snapshot);

    let mut unmerged = baseline.clone();
    unmerged.ir.profile_context.merge_state = MergeState::Unmerged;
    assert_scan_not_triggered(unmerged);

    let mut unknown_formatter = baseline.clone();
    unknown_formatter.ir.runtime_formatter_shape = RuntimeInventoryFormatterShape::Unknown;
    assert_scan_not_triggered(unknown_formatter);

    let mut no_pipeline = baseline.clone();
    no_pipeline.ir.pipeline_containers.clear();
    assert_scan_not_triggered(no_pipeline);

    let mut no_physical_membership = baseline.clone();
    no_physical_membership
        .ir
        .physical_operator_memberships
        .clear();
    assert_scan_not_triggered(no_physical_membership);

    let scan_group_ref = baseline
        .ir
        .operator_groups
        .iter()
        .find(|group| group.canonical_type == "SCAN")
        .map(|group| group.operator_group_id.clone())
        .expect("fixture has a scan group");
    let baseline_natives = native_occurrences(&baseline);
    let mut subordinate_only = baseline.clone();
    subordinate_only
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| {
            observation.subject_ref.as_deref() == Some(scan_group_ref.as_str())
                && observation.metric_id.as_deref() == Some("OperatorTotalTime")
                && observation
                    .runtime_occurrence_ref
                    .as_ref()
                    .is_some_and(|reference| baseline_natives.contains(reference))
        })
        .expect("fixture has a native OperatorTotalTime")
        .operator_ref = Some("forced-subordinate".into());
    assert_scan_not_triggered(subordinate_only);

    let mut occurrence_not_in_group = baseline.clone();
    occurrence_not_in_group
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| {
            observation.subject_ref.as_deref() == Some(scan_group_ref.as_str())
                && observation.metric_id.as_deref() == Some("OperatorTotalTime")
        })
        .expect("fixture has scan OperatorTotalTime")
        .runtime_occurrence_ref = Some("physical:outside-selected-group".into());
    assert_scan_not_triggered(occurrence_not_in_group);

    let mut non_exact_pack = baseline.clone();
    non_exact_pack.ir.version.pack_match = PackMatch::NoPack;
    assert_scan_not_triggered(non_exact_pack);

    let mut missing_profile_ref = baseline.clone();
    missing_profile_ref.ir.profile_context.evidence_refs = vec!["text:missing".into()];
    assert_scan_not_triggered(missing_profile_ref);

    let mut derived_denominator = baseline.clone();
    derived_denominator
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| {
            observation.metric_id.as_deref() == Some("QueryCumulativeOperatorTime")
                && observation.scope == "query"
        })
        .unwrap()
        .value_role = "derived".into();
    assert_scan_not_triggered(derived_denominator);

    let mut non_allowlisted = baseline;
    let scan_metric = non_allowlisted
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| observation.metric_id.as_deref() == Some("ScanTime"))
        .unwrap();
    scan_metric.metric_id = Some("ActiveTime".into());
    scan_metric.source_name = "ActiveTime".into();
    scan_metric.definition_ref = Some("sr3514-metric-registry:ActiveTime".into());
    assert_scan_not_triggered(non_allowlisted);
}

#[test]
fn runtime_profile_parses_official_decorated_max_counter_as_exact_base_metric() {
    let with_max = LIVE_RUNTIME_PROFILE_TEXT.replacen(
        "             - OperatorTotalTime: 1.237ms",
        "             - OperatorTotalTime: 1.237ms\n             - __MAX_OF_OperatorTotalTime: 9s",
        1,
    );
    let output = adapt_text(&with_max).unwrap();
    let maximum = output
        .ir
        .metric_observations
        .iter()
        .find(|observation| observation.source_name == "__MAX_OF_OperatorTotalTime")
        .expect("decorated maximum counter is projected");
    assert_eq!(maximum.metric_id.as_deref(), Some("OperatorTotalTime"));
    assert_eq!(maximum.value_role, "reported_max");
    assert_eq!(
        maximum
            .canonical_number
            .as_ref()
            .map(|number| number.value.as_str()),
        Some("9000000000")
    );
}

#[test]
fn official_stored_pipeline_rules_fail_closed_on_not_identical_merge_tree() {
    let not_identical =
        LIVE_RUNTIME_PROFILE_TEXT.replacen("Execution:", "Execution:\n   - NotIdentical", 1);
    let output = resolved_runtime_text_before_rules(&not_identical);
    assert!(output.ir.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "runtime_profile_not_identical" && !diagnostic.source_refs.is_empty()
    }));
    assert!(
        output
            .ir
            .profile_context
            .limitations
            .iter()
            .any(|limitation| limitation == "runtime_profile_not_identical")
    );
    assert_scan_not_triggered(output);
}

#[test]
fn official_stored_pipeline_rules_fail_closed_on_missing_or_running_instances() {
    for (content, expected_code) in [
        (
            LIVE_RUNTIME_PROFILE_TEXT.replacen(
                "Execution:",
                "Execution:\n   - MissingInstanceIds: redacted-instance",
                1,
            ),
            "runtime_profile_missing_instances",
        ),
        (
            LIVE_RUNTIME_PROFILE_TEXT.replacen(
                "          CommonMetrics:",
                "          CommonMetrics:\n             - IsRunning",
                1,
            ),
            "runtime_operator_running",
        ),
    ] {
        let output = resolved_runtime_text_before_rules(&content);
        assert!(output.ir.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == expected_code && !diagnostic.source_refs.is_empty()
        }));
        assert!(
            output
                .ir
                .profile_context
                .limitations
                .iter()
                .any(|limitation| limitation == expected_code)
        );
        assert_scan_not_triggered(output);
    }
}

#[test]
fn official_stored_pipeline_rules_fail_closed_on_malformed_reserved_markers() {
    for content in [
        LIVE_RUNTIME_PROFILE_TEXT.replacen(
            "Execution:",
            "Execution:\n   - NotIdentical: unexpected",
            1,
        ),
        LIVE_RUNTIME_PROFILE_TEXT.replacen("Execution:", "Execution:\n   - MissingInstanceIds:", 1),
        LIVE_RUNTIME_PROFILE_TEXT.replacen(
            "          CommonMetrics:",
            "          CommonMetrics:\n             - IsRunning: unexpected",
            1,
        ),
        LIVE_RUNTIME_PROFILE_TEXT.replacen(
            "             - IsSubordinate",
            "             - IsSubordinate: unexpected",
            1,
        ),
        LIVE_RUNTIME_PROFILE_TEXT.replacen(
            "             - IsSubordinate",
            "          UniqueMetrics:\n             - IsSubordinate\n          CommonMetrics:",
            1,
        ),
        LIVE_RUNTIME_PROFILE_TEXT.replacen(
            "             - IsSubordinate",
            "             - IsSubordinate\n             - IsSubordinate",
            1,
        ),
    ] {
        let output = resolved_runtime_text_before_rules(&content);
        assert!(output.ir.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "runtime_profile_incomplete_marker_malformed"
                && !diagnostic.source_refs.is_empty()
        }));
        assert_scan_not_triggered(output);
    }
}

#[test]
fn official_stored_pipeline_rules_require_complete_min_max_pairs() {
    let with_min_only = LIVE_RUNTIME_PROFILE_TEXT.replacen(
        "             - OperatorTotalTime: 1.218ms",
        "             - OperatorTotalTime: 1.218ms\n             - __MIN_OF_OperatorTotalTime: 1ms",
        1,
    );
    let output = resolved_runtime_text_before_rules(&with_min_only);
    assert!(output.ir.metric_observations.iter().any(|observation| {
        observation.source_name == "__MIN_OF_OperatorTotalTime"
            && observation.value_role == "reported_min"
    }));
    assert_scan_not_triggered(output);

    let duplicate_max = LIVE_RUNTIME_PROFILE_TEXT.replacen(
        "             - OperatorTotalTime: 1.218ms",
        "             - OperatorTotalTime: 1.218ms\n             - __MIN_OF_OperatorTotalTime: 1ms\n             - __MAX_OF_OperatorTotalTime: 2ms\n             - __MAX_OF_OperatorTotalTime: 3ms",
        1,
    );
    assert_scan_not_triggered(resolved_runtime_text_before_rules(&duplicate_max));

    let invalid_max = LIVE_RUNTIME_PROFILE_TEXT.replacen(
        "             - OperatorTotalTime: 1.218ms",
        "             - OperatorTotalTime: 1.218ms\n             - __MIN_OF_OperatorTotalTime: 1ms\n             - __MAX_OF_OperatorTotalTime: invalid",
        1,
    );
    assert_scan_not_triggered(resolved_runtime_text_before_rules(&invalid_max));

    for replacement in [
        "             - OperatorTotalTime: invalid\n             - __MIN_OF_OperatorTotalTime: 1ms\n             - __MAX_OF_OperatorTotalTime: 2ms",
        "             - OperatorTotalTime: 1.218ms\n             - __MIN_OF_OperatorTotalTime: invalid\n             - __MAX_OF_OperatorTotalTime: 2ms",
        "             - OperatorTotalTime: 1.218ms\n             - __MIN_OF_OperatorTotalTime: 3ms\n             - __MAX_OF_OperatorTotalTime: 2ms",
    ] {
        let invalid_pair = LIVE_RUNTIME_PROFILE_TEXT.replacen(
            "             - OperatorTotalTime: 1.218ms",
            replacement,
            1,
        );
        assert_scan_not_triggered(resolved_runtime_text_before_rules(&invalid_pair));
    }
}

#[test]
fn official_stored_pipeline_rules_bind_special_metrics_to_the_proven_native_occurrence() {
    let mut output = resolved_runtime_output_before_rules();
    let scan_group = output
        .ir
        .operator_groups
        .iter()
        .find(|group| group.canonical_type == "SCAN")
        .cloned()
        .expect("fixture has scan group");
    let mut subordinate_membership = output
        .ir
        .physical_operator_memberships
        .iter()
        .find(|membership| {
            membership.plan_node_id == scan_group.plan_node_id
                && membership.fragment_ref == scan_group.fragment_ref
        })
        .cloned()
        .expect("fixture has a scan occurrence");
    subordinate_membership.physical_operator_ref =
        format!("{}:role-swap", subordinate_membership.physical_operator_ref);
    subordinate_membership.source_role = doctor_contracts::RuntimeSourceRole::Subordinate;
    let subordinate_occurrence = subordinate_membership.physical_operator_ref.clone();
    output
        .ir
        .physical_operator_memberships
        .push(subordinate_membership);
    let natives = native_occurrences(&output);
    let scan_time = output
        .ir
        .metric_observations
        .iter_mut()
        .find(|observation| {
            observation.subject_ref.as_deref() == Some(scan_group.operator_group_id.as_str())
                && observation.metric_id.as_deref() == Some("ScanTime")
                && observation
                    .runtime_occurrence_ref
                    .as_ref()
                    .is_some_and(|reference| natives.contains(reference))
        })
        .expect("fixture has a native scan-time counter");
    scan_time.runtime_occurrence_ref = Some(subordinate_occurrence);
    assert_scan_not_triggered(output);
}

#[test]
fn official_stored_pipeline_rules_reject_multiple_unmarked_native_occurrences() {
    let mut output = resolved_runtime_output_before_rules();
    let scan_group = output
        .ir
        .operator_groups
        .iter()
        .find(|group| group.canonical_type == "SCAN")
        .cloned()
        .expect("fixture has scan group");
    let mut duplicate_membership = output
        .ir
        .physical_operator_memberships
        .iter()
        .find(|membership| {
            membership.plan_node_id == scan_group.plan_node_id
                && membership.fragment_ref == scan_group.fragment_ref
                && membership.source_role == doctor_contracts::RuntimeSourceRole::Native
        })
        .cloned()
        .expect("fixture has a native scan occurrence");
    duplicate_membership.physical_operator_ref = format!(
        "{}:second-unmarked",
        duplicate_membership.physical_operator_ref
    );
    let duplicate_occurrence = duplicate_membership.physical_operator_ref.clone();
    output
        .ir
        .physical_operator_memberships
        .push(duplicate_membership);

    let natives = native_occurrences(&output);
    let mut duplicate_total = output
        .ir
        .metric_observations
        .iter()
        .find(|observation| {
            observation.subject_ref.as_deref() == Some(scan_group.operator_group_id.as_str())
                && observation.metric_id.as_deref() == Some("OperatorTotalTime")
                && observation.value_role == "reported_base"
                && observation
                    .runtime_occurrence_ref
                    .as_ref()
                    .is_some_and(|reference| natives.contains(reference))
        })
        .cloned()
        .expect("fixture has a native total");
    duplicate_total.observation_id = format!("{}:second-unmarked", duplicate_total.observation_id);
    duplicate_total.runtime_occurrence_ref = Some(duplicate_occurrence);
    let duplicate_total_ref = duplicate_total.observation_id.clone();
    output.ir.metric_observations.push(duplicate_total);
    output
        .ir
        .operator_groups
        .iter_mut()
        .find(|group| group.operator_group_id == scan_group.operator_group_id)
        .expect("scan group")
        .metric_observation_refs
        .push(duplicate_total_ref);

    assert_scan_not_triggered(output);
}

/// ExplainAnalyzer.ProfileNodeParser keeps every unmarked operator of a plan
/// node as native in print order: a LIMIT pushed into the scan pipeline is a
/// second native of a different type and must not void the source-stage proof.
#[test]
fn official_stored_pipeline_accepts_distinct_native_types_in_one_pipeline() {
    let mut output = resolved_runtime_output_before_rules();
    let scan_group = output
        .ir
        .operator_groups
        .iter()
        .find(|group| group.canonical_type == "SCAN")
        .cloned()
        .expect("fixture has scan group");
    let mut limit = output
        .ir
        .physical_operator_memberships
        .iter()
        .find(|membership| {
            membership.plan_node_id == scan_group.plan_node_id
                && membership.fragment_ref == scan_group.fragment_ref
                && membership.source_role == doctor_contracts::RuntimeSourceRole::Native
        })
        .cloned()
        .expect("fixture has a native scan occurrence");
    limit.physical_operator_ref = format!("{}:pushed-limit", limit.physical_operator_ref);
    limit.canonical_type = "LIMIT".into();
    limit.source_occurrence_ordinal += 1_000;
    let limit_occurrence = limit.physical_operator_ref.clone();
    output.ir.physical_operator_memberships.push(limit);
    let mut limit_total = output
        .ir
        .metric_observations
        .iter()
        .find(|observation| {
            observation.subject_ref.as_deref() == Some(scan_group.operator_group_id.as_str())
                && observation.metric_id.as_deref() == Some("OperatorTotalTime")
                && observation.value_role == "reported_base"
                && observation.operator_ref.is_none()
        })
        .cloned()
        .expect("fixture has a native total");
    limit_total.observation_id = format!("{}:pushed-limit", limit_total.observation_id);
    limit_total.runtime_occurrence_ref = Some(limit_occurrence);
    let limit_total_ref = limit_total.observation_id.clone();
    output.ir.metric_observations.push(limit_total);
    output
        .ir
        .operator_groups
        .iter_mut()
        .find(|group| group.operator_group_id == scan_group.operator_group_id)
        .expect("scan group")
        .metric_observation_refs
        .push(limit_total_ref.clone());

    evaluate_rules(&mut output).unwrap();
    let evaluation = output
        .ir
        .rule_evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == "SR3514.SCAN_TIME_SHARE")
        .expect("scan evaluation");
    assert_ne!(evaluation.status, RuleEvaluationStatus::NotApplicable);
    assert!(evaluation.input_observation_refs.contains(&limit_total_ref));
    assert!(evaluation.applicability_checks.iter().any(|check| {
        check == "source_counter_stage:official_3514_stored_pipeline_counter_stage_complete"
    }));
}

#[test]
fn official_stored_pipeline_time_rules_prefer_explicit_max_counter() {
    let mut output = resolved_runtime_output_before_rules();
    let scan_group_ref = output
        .ir
        .operator_groups
        .iter()
        .find(|group| group.canonical_type == "SCAN")
        .map(|group| group.operator_group_id.clone())
        .expect("fixture has a scan group");
    let natives = native_occurrences(&output);
    let base_index = output
        .ir
        .metric_observations
        .iter()
        .position(|observation| {
            observation.subject_ref.as_deref() == Some(scan_group_ref.as_str())
                && observation.metric_id.as_deref() == Some("OperatorTotalTime")
                && observation.value_role == "reported_base"
                && observation
                    .runtime_occurrence_ref
                    .as_ref()
                    .is_some_and(|reference| natives.contains(reference))
        })
        .expect("fixture has a native base counter");
    let base_id = output.ir.metric_observations[base_index]
        .observation_id
        .clone();
    let mut minimum = output.ir.metric_observations[base_index].clone();
    minimum.observation_id = format!("{base_id}:explicit-min");
    minimum.source_name = "__MIN_OF_OperatorTotalTime".into();
    minimum.value_role = "reported_min".into();
    let minimum_id = minimum.observation_id.clone();
    let mut maximum = output.ir.metric_observations[base_index].clone();
    maximum.observation_id = format!("{base_id}:explicit-max");
    maximum.source_name = "__MAX_OF_OperatorTotalTime".into();
    maximum.value_role = "reported_max".into();
    maximum
        .canonical_number
        .as_mut()
        .expect("exact duration")
        .value = "9000000000".into();
    // A lossless 9 s maximum: the base counter's text bounds do not apply to it.
    maximum.fidelity = doctor_contracts::Fidelity::Exact;
    maximum.bounds = None;
    let maximum_id = maximum.observation_id.clone();
    output.ir.metric_observations.push(minimum);
    output.ir.metric_observations.push(maximum);
    output
        .ir
        .operator_groups
        .iter_mut()
        .find(|group| group.operator_group_id == scan_group_ref)
        .expect("scan group")
        .metric_observation_refs
        .extend([minimum_id, maximum_id.clone()]);

    evaluate_rules(&mut output).unwrap();
    let evaluation = output
        .ir
        .rule_evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == "SR3514.SCAN_TIME_SHARE")
        .expect("scan evaluation");
    assert_eq!(evaluation.status, RuleEvaluationStatus::Triggered);
    assert!(evaluation.input_observation_refs.contains(&maximum_id));
    assert!(!evaluation.input_observation_refs.contains(&base_id));
}

#[test]
fn runtime_profile_derives_level_two_unmerged_from_official_non_default_session_evidence() {
    let level_two = LIVE_RUNTIME_PROFILE_TEXT.replace(
        r#""enable_profile":{"defaultValue":false,"actualValue":true}"#,
        r#""enable_profile":{"defaultValue":false,"actualValue":true},"pipeline_profile_level":{"defaultValue":1,"actualValue":2}"#,
    );
    let mut output = adapt_text(&level_two).unwrap();
    assert_eq!(
        output.ir.profile_context.profile_level,
        ProfileLevel::Level2
    );
    assert_eq!(output.ir.profile_context.snapshot_kind, SnapshotKind::Final);
    assert_eq!(output.ir.profile_context.merge_state, MergeState::Unmerged);
    let graph_evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, graph_evidence).unwrap();
    evaluate_rules(&mut output).unwrap();
    assert!(output.ir.findings.is_empty());
    assert!(output.ir.rule_evaluations.iter().all(|evaluation| {
        evaluation.status != doctor_contracts::RuleEvaluationStatus::Triggered
    }));
}

#[test]
fn runtime_profile_rejects_malformed_stored_context_evidence() {
    let malformed = LIVE_RUNTIME_PROFILE_TEXT
        .replace(
            r#"{"enable_adaptive_sink_dop"#,
            r#"{"pipeline_profile_level":{"defaultValue":1,"actualValue":3},"enable_adaptive_sink_dop"#,
        )
        .replace("- IsProfileAsync: true", "- IsProfileAsync: maybe");
    let output = adapt_text(&malformed).unwrap();
    assert_eq!(
        output.ir.profile_context.profile_level,
        ProfileLevel::Unknown
    );
    assert_eq!(
        output.ir.profile_context.snapshot_kind,
        SnapshotKind::Unknown
    );
    assert_eq!(output.ir.profile_context.merge_state, MergeState::Unknown);
    assert!(
        output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message_id == "profile_context_invalid")
    );

    let duplicate_session_key = LIVE_RUNTIME_PROFILE_TEXT.replace(
        r#"{"enable_adaptive_sink_dop"#,
        r#"{"pipeline_profile_level":{"defaultValue":1,"actualValue":2},"pipeline_profile_level":{"defaultValue":1,"actualValue":2},"enable_adaptive_sink_dop"#,
    );
    let output = adapt_text(&duplicate_session_key).unwrap();
    assert_eq!(
        output.ir.profile_context.profile_level,
        ProfileLevel::Unknown
    );
    assert_eq!(output.ir.profile_context.merge_state, MergeState::Unknown);
    assert!(
        output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message_id == "profile_context_invalid")
    );
}

#[test]
fn runtime_profile_planner_total_alias_is_scoped_and_conservative() {
    let exact = r#"Query:
  Summary:
    - Query Type: Query
    - StarRocks Version: 3.5.14-23a56ec
  Planner:
    - -- Total[1] 209ms
  Execution:
    - QueryExecutionWallTime: 4s240ms
    - -- Total[1] 7s
"#;
    let exact = adapt_text(exact).unwrap();
    let planning = exact
        .ir
        .derived_metric_cells
        .iter()
        .find(|metric| metric.metric_id == "QueryPlanningTime")
        .unwrap();
    assert_eq!(planning.state(), MetricCellState::Exact);
    assert_eq!(
        planning
            .canonical_number()
            .map(|number| number.value.as_str()),
        Some("209000000")
    );
    assert_eq!(planning.source_refs, vec!["text:6"]);
    assert_eq!(
        exact
            .ir
            .metric_observations
            .iter()
            .filter(|metric| metric.metric_id.as_deref() == Some("QueryPlanningTime"))
            .count(),
        1
    );

    let missing = adapt_text(
        r#"Query:
  Summary:
    - Query Type: Query
    - StarRocks Version: 3.5.14-23a56ec
  Planner:
    - -- Parser[1] 23ms
  Execution:
    - QueryExecutionWallTime: 4s240ms
    - -- Total[1] 7s
"#,
    )
    .unwrap();
    let missing = missing
        .ir
        .derived_metric_cells
        .iter()
        .find(|metric| metric.metric_id == "QueryPlanningTime")
        .unwrap();
    assert_eq!(missing.state(), MetricCellState::Unavailable);
    assert!(
        missing
            .reasons
            .iter()
            .any(|reason| reason == "query_planning_time_unavailable")
    );

    let ambiguous = adapt_text(
        r#"Query:
  Summary:
    - Query Type: Query
    - StarRocks Version: 3.5.14-23a56ec
  Planner:
    - -- Total[1] 209ms
    - -- Total[1] 210ms
  Execution:
    - QueryExecutionWallTime: 4s240ms
"#,
    )
    .unwrap();
    let ambiguous = ambiguous
        .ir
        .derived_metric_cells
        .iter()
        .find(|metric| metric.metric_id == "QueryPlanningTime")
        .unwrap();
    assert_eq!(ambiguous.state(), MetricCellState::Unavailable);
    assert!(
        ambiguous
            .reasons
            .iter()
            .any(|reason| reason == "query_planning_time_ambiguous")
    );
}

#[test]
fn runtime_profile_planner_total_is_a_trusted_fact_without_version_or_execution_metrics() {
    let output = adapt_text(
        r#"Query:
  Summary:
    - Query Type: Query
  Planner:
    - -- Total[1] 209ms
  Execution:
"#,
    )
    .unwrap();

    assert_eq!(output.analysis_state, AnalysisState::Partial);
    let planning = output
        .ir
        .derived_metric_cells
        .iter()
        .find(|metric| metric.metric_id == "QueryPlanningTime")
        .unwrap();
    assert_eq!(planning.state(), MetricCellState::Exact);
    assert_eq!(
        planning
            .canonical_number()
            .map(|number| number.value.as_str()),
        Some("209000000")
    );
}

#[test]
fn runtime_profile_json_formatter_shape_prefers_explicit_active_ns() {
    // Source-derived, offline formatter shape: RuntimeProfile.JsonProfileFormatter
    // nests each RuntimeProfile by name and emits Active_ns only as a numeric raw value.
    let json = r#"{
      "Query": {
        "Summary": {
          "Query ID": "00000000-0000-0000-0000-000000000001",
          "Query Type": "Query",
          "Query State": "Finished",
          "StarRocks Version": "3.5.14-23a56ec",
          "Sql Statement": "SELECT secret"
        },
        "Planner": {
          "Total": "209ms",
          "Active": "999ms",
          "Nested": {"Total": "not-a-planning-counter"}
        },
        "Execution": {
          "Topology": "{\"rootId\":2,\"nodes\":[{\"id\":2,\"name\":\"AGGREGATION\",\"properties\":{\"sinkIds\":[]},\"children\":[1]},{\"id\":1,\"name\":\"PROJECT\",\"properties\":{\"sinkIds\":[]},\"children\":[0]},{\"id\":0,\"name\":\"SCHEMA_SCAN\",\"properties\":{\"sinkIds\":[]},\"children\":[]}]}",
          "QueryExecutionWallTime": "9ms",
          "Fragment 0": {
            "Pipeline (id=0)": {
              "Active": "2ms",
              "Active_ns": 2000000,
              "PendingTime": "1ms",
              "SCAN (plan_node_id=0)": {
                "CommonMetrics": {"OperatorTotalTime":"1ms","PushRowNum":"2"},
                "UniqueMetrics": {"SqlText":"must-not-project"}
              }
            }
          }
        }
      }
    }"#;
    let output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    assert_eq!(
        output
            .ir
            .query_summary
            .as_ref()
            .and_then(|summary| summary.query_id.as_deref()),
        Some("00000000-0000-0000-0000-000000000001")
    );
    assert_eq!(output.ir.profile_context.query_state, QueryState::Finished);
    assert_eq!(output.ir.graph.nodes.len(), 3);
    assert_eq!(output.ir.graph.edges.len(), 2);
    let active = output
        .ir
        .metric_observations
        .iter()
        .find(|metric| metric.metric_id.as_deref() == Some("Active"))
        .unwrap();
    assert_eq!(
        active
            .canonical_number
            .as_ref()
            .map(|number| number.value.as_str()),
        Some("2000000")
    );
    assert_eq!(active.scope, "pipeline");
    let planning = output
        .ir
        .metric_observations
        .iter()
        .find(|metric| metric.metric_id.as_deref() == Some("QueryPlanningTime"))
        .unwrap();
    assert_eq!(
        planning
            .canonical_number
            .as_ref()
            .map(|number| number.value.as_str()),
        Some("209000000")
    );
    assert_eq!(planning.source_refs, vec!["json:/Query/Planner/Total"]);
    assert_eq!(planning.display_value, "209ms");
    let report = assemble_public_report(output).unwrap();
    let tokens = report
        .source_view
        .json_tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!tokens.contains("SELECT secret"));
    assert!(!tokens.contains("must-not-project"));
}

#[test]
fn runtime_profile_json_captures_incomplete_merge_markers_without_exposing_values() {
    let json = r#"{
      "Query": {
        "Summary": {
          "Query ID": "00000000-0000-0000-0000-000000000001",
          "Query State": "Finished",
          "StarRocks Version": "3.5.14-23a56ec"
        },
        "Execution": {
          "MissingInstanceIds": "sensitive-instance-id",
          "Fragment 0": {
            "Pipeline (id=0)": {
              "SCAN (plan_node_id=0)": {
                "CommonMetrics": {"IsRunning":"","OperatorTotalTime":"1ms"},
                "UniqueMetrics": {"ScanTime":"2ms"}
              }
            }
          }
        }
      }
    }"#;
    let output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    for code in [
        "runtime_profile_missing_instances",
        "runtime_operator_running",
    ] {
        assert!(
            output.ir.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == code && !diagnostic.source_refs.is_empty()
            })
        );
        assert!(
            output
                .ir
                .profile_context
                .limitations
                .iter()
                .any(|limitation| limitation == code)
        );
    }
    assert!(
        !output
            .source_view
            .json_tokens
            .iter()
            .any(|token| { token.text.contains("sensitive-instance-id") })
    );
}

#[test]
fn runtime_profile_json_fails_closed_on_malformed_reserved_markers() {
    let json = r#"{
      "Query": {
        "Summary": {
          "Query ID": "00000000-0000-0000-0000-000000000001",
          "Query State": "Finished",
          "StarRocks Version": "3.5.14-23a56ec"
        },
        "Execution": {
          "NotIdentical": "unexpected",
          "MissingInstanceIds": "",
          "Fragment 0": {
            "Pipeline (id=0)": {
              "SCAN (plan_node_id=0)": {
                "CommonMetrics": {
                  "IsRunning":"unexpected",
                  "IsSubordinate":"unexpected",
                  "OperatorTotalTime":"1ms"
                },
                "UniqueMetrics": {"IsSubordinate":"","ScanTime":"2ms"}
              }
            }
          }
        }
      }
    }"#;
    let output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    assert_eq!(
        output
            .ir
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == "runtime_profile_incomplete_marker_malformed"
            })
            .count(),
        5
    );
    assert!(
        output
            .ir
            .profile_context
            .limitations
            .iter()
            .any(|limitation| limitation == "runtime_profile_incomplete_marker_malformed")
    );
    assert!(
        !output
            .source_view
            .json_tokens
            .iter()
            .any(|token| token.text.contains("unexpected"))
    );
    assert_eq!(output.ir.physical_operator_memberships.len(), 1);
    assert!(
        output
            .ir
            .physical_operator_memberships
            .iter()
            .all(|membership| {
                membership.source_role == doctor_contracts::RuntimeSourceRole::Unknown
            })
    );
    assert!(
        output
            .ir
            .operator_groups
            .iter()
            .all(|group| group.native_operator_refs.is_empty())
    );
}

#[test]
fn runtime_profile_json_rejects_duplicate_object_keys_before_role_inference() {
    let json = r#"{
      "Query": {
        "Summary": {"StarRocks Version":"3.5.14-23a56ec"},
        "Execution": {
          "Fragment 0": {
            "Pipeline (id=0)": {
              "SCAN (plan_node_id=0)": {
                "CommonMetrics": {
                  "IsSubordinate":"unexpected",
                  "IsSubordinate":"",
                  "OperatorTotalTime":"1ms"
                }
              }
            }
          }
        }
      }
    }"#;
    let error = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap_err();
    assert_eq!(error.code, "parse_failed");
    assert_eq!(error.stage, "parse");
    assert_eq!(error.message_id, "json_invalid");
}

#[test]
fn runtime_profile_rejects_duplicate_keys_in_embedded_topology_json() {
    let text = LIVE_RUNTIME_PROFILE_TEXT.replacen(r#"{"rootId":2"#, r#"{"rootId":1,"rootId":2"#, 1);
    let text_output = adapt_text(&text).unwrap();
    assert!(
        text_output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message_id == "runtime_topology_invalid" })
    );

    let json = r#"{
      "Query": {
        "Summary": {"StarRocks Version":"3.5.14-23a56ec"},
        "Execution": {
          "Topology":"{\"rootId\":1,\"rootId\":2,\"nodes\":[]}"
        }
      }
    }"#;
    let json_output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    assert!(
        json_output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message_id == "runtime_topology_invalid" })
    );
}

#[test]
fn runtime_profile_output_rows_follow_explain_analyzer_recipes() {
    let json = r#"{
      "Query": {
        "Summary": {"Query Type":"Query","StarRocks Version":"3.5.14-23a56ec"},
        "Execution": {
          "Topology": "{\"rootId\":3,\"nodes\":[{\"id\":3,\"name\":\"EXCHANGE\",\"properties\":{\"sinkIds\":[]},\"children\":[2]},{\"id\":2,\"name\":\"AGGREGATION\",\"properties\":{\"sinkIds\":[]},\"children\":[1]},{\"id\":1,\"name\":\"UNION\",\"properties\":{\"sinkIds\":[]},\"children\":[]}]}",
          "Fragment 0": {
            "InstanceNum": "2",
            "Pipeline (id=0)": {
              "RESULT_SINK (plan_node_id=-1)": {
                "CommonMetrics":{"IsFinalSink":"","PushRowNum":"9"}
              },
              "UNION_SOURCE (plan_node_id=1)": {
                "CommonMetrics":{"PullRowNum":"2"}
              },
              "UNION_SINK (plan_node_id=1)": {
                "CommonMetrics":{"PullRowNum":"3"}
              },
              "CHUNK_ACCUMULATE (plan_node_id=1)": {
                "CommonMetrics":{"IsSubordinate":"","PullRowNum":"999"}
              },
              "AGGREGATE_BLOCKING_SINK (plan_node_id=2)": {
                "CommonMetrics":{"PullRowNum":"17"}
              },
              "AGGREGATE_BLOCKING_SOURCE (plan_node_id=2)": {
                "CommonMetrics":{"PullRowNum":"11"}
              },
              "EXCHANGE (plan_node_id=3)": {
                "CommonMetrics":{"PullRowNum":"7"},
                "UniqueMetrics":{"PartType":"UNPARTITIONED"}
              }
            }
          }
        }
      }
    }"#;
    let output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    let output_rows = |plan_node_id: &str| {
        let group = output
            .ir
            .operator_groups
            .iter()
            .find(|group| group.plan_node_id.as_deref() == Some(plan_node_id))
            .unwrap();
        output
            .ir
            .metric_observations
            .iter()
            .find(|metric| {
                metric.subject_ref.as_deref() == Some(group.operator_group_id.as_str())
                    && metric.metric_id.as_deref() == Some("OutputRows")
            })
            .unwrap()
    };
    // Official ExplainAnalyzer: an operator without IsSubordinate is native.
    let value = |plan_node_id: &str| {
        let metric = output_rows(plan_node_id);
        assert_eq!(metric.parse_state, doctor_contracts::ParseState::Exact);
        assert_eq!(metric.value_role, "derived");
        metric.canonical_number.as_ref().unwrap().value.clone()
    };
    // Final sink: PushRowNum. UNION: Σ native PullRowNum, subordinate excluded.
    assert_eq!(value("-1"), "9");
    assert_eq!(value("1"), "5");
    // Aggregation: the first native that is not a non-last _SINK.
    assert_eq!(value("2"), "11");
    // UNPARTITIONED exchange: PullRowNum / InstanceNum.
    assert_eq!(value("3"), "3");

    let partitioned = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            json.replace("UNPARTITIONED", "HASH_PARTITIONED")
                .into_bytes(),
        ),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    let exchange = partitioned
        .ir
        .operator_groups
        .iter()
        .find(|group| group.plan_node_id.as_deref() == Some("3"))
        .unwrap();
    let output_rows = partitioned
        .ir
        .metric_observations
        .iter()
        .find(|metric| {
            metric.subject_ref.as_deref() == Some(exchange.operator_group_id.as_str())
                && metric.metric_id.as_deref() == Some("OutputRows")
        })
        .unwrap();
    // A partitioned exchange reads PullRowNum without broadcast division.
    assert_eq!(output_rows.canonical_number.as_ref().unwrap().value, "7");
}

#[test]
fn runtime_profile_output_rows_requires_proven_identity_and_complete_union_inputs() {
    let without_topology = r#"{
      "Query": {
        "Summary": {"Query Type":"Query","StarRocks Version":"3.5.14-23a56ec"},
        "Execution": {
          "Fragment 0": {"Pipeline (id=0)": {
            "RESULT_SINK (plan_node_id=-1)": {"CommonMetrics":{"IsFinalSink":"","PushRowNum":"9"}},
            "UNION_SOURCE (plan_node_id=1)": {"CommonMetrics":{"PullRowNum":"2"}},
            "UNION_SINK (plan_node_id=1)": {"CommonMetrics":{"PullRowNum":"3"}},
            "AGGREGATE_BLOCKING_SOURCE (plan_node_id=2)": {"CommonMetrics":{"PullRowNum":"11"}}
          }}
        }
      }
    }"#;
    let output = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            without_topology.as_bytes(),
        ),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    for plan_node_id in ["1", "2"] {
        let metric = output_rows_metric(&output, plan_node_id);
        assert_eq!(metric.semantic_state, "unavailable");
        assert!(
            metric.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "output_rows_operator_identity_unavailable"
            })
        );
    }
    // The final sink needs no topology identity: IsFinalSink + PushRowNum.
    assert_eq!(
        output_rows_metric(&output, "-1").parse_state,
        doctor_contracts::ParseState::Exact
    );

    let duplicate_identity = without_topology.replacen(
        "\"Execution\": {",
        "\"Execution\": {\"Topology\":\"{\\\"rootId\\\":1,\\\"nodes\\\":[{\\\"id\\\":1,\\\"name\\\":\\\"UNION\\\",\\\"properties\\\":{\\\"sinkIds\\\":[]},\\\"children\\\":[]},{\\\"id\\\":1,\\\"name\\\":\\\"PROJECT\\\",\\\"properties\\\":{\\\"sinkIds\\\":[]},\\\"children\\\":[]}] }\",",
        1,
    );
    let duplicate_identity = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            duplicate_identity.as_bytes(),
        ),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    assert!(
        output_rows_metric(&duplicate_identity, "1")
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "output_rows_operator_identity_unavailable" })
    );

    let union = r#"Query:
  Summary:
    - StarRocks Version: 3.5.14-23a56ec
  Execution:
    - Topology: {"rootId":1,"nodes":[{"id":1,"name":"UNION","properties":{"sinkIds":[]},"children":[]}]}
    Fragment 0:
      Pipeline (id=0):
        UNION_SOURCE (plan_node_id=1):
          CommonMetrics:
            - PullRowNum: 2
        UNION_SINK (plan_node_id=1):
          CommonMetrics:
            - PullRowNum: 3
"#;
    for input in [
        union.replace("            - PullRowNum: 3\n", ""),
        union.replace(
            "            - PullRowNum: 3\n",
            "            - PullRowNum: 3\n            - PullRowNum: 4\n",
        ),
        union.replace(
            "            - PullRowNum: 3",
            "            - PullRowNum: invalid",
        ),
    ] {
        let output = adapt_text(&input).unwrap();
        let metric = output_rows_metric(&output, "1");
        assert_eq!(metric.semantic_state, "unavailable");
        assert!(
            metric.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "output_rows_selected_counter_unavailable"
            })
        );
    }
}

#[test]
fn runtime_profile_json_preserves_physical_operator_source_order_within_plan_node() {
    // The official formatter emits named child profiles as JSON object members. Their
    // source order is evidence: source/sink roles for one plan node must not be
    // alphabetically reordered by serde_json::Map before grouping.
    let json = r#"{
      "Query": {
        "Summary": {"Query Type":"Query","StarRocks Version":"3.5.14-23a56ec"},
        "Execution": {
          "Topology": "{\"rootId\":2,\"nodes\":[{\"id\":2,\"name\":\"AGGREGATION\",\"properties\":{\"sinkIds\":[]},\"children\":[]}]}",
          "Fragment 0": {
            "Pipeline (id=0)": {
              "AGGREGATE_BLOCKING_SOURCE (plan_node_id=2)": {
                "CommonMetrics":{"OperatorTotalTime":"2ms"}
              },
              "AGGREGATE_BLOCKING_SINK (plan_node_id=2)": {
                "CommonMetrics":{"OperatorTotalTime":"3ms"}
              }
            }
          }
        }
      }
    }"#;
    let output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    let aggregate_group = output
        .ir
        .operator_groups
        .iter()
        .find(|operator| operator.plan_node_id.as_deref() == Some("2"))
        .unwrap();

    assert_eq!(aggregate_group.canonical_type, "AGGREGATION");
    assert!(aggregate_group.raw_labels.ends_with(&[
        "AGGREGATE_BLOCKING_SOURCE".to_string(),
        "AGGREGATE_BLOCKING_SINK".to_string(),
    ]));
    // Ordered group refs keep the unmarked encoding; the membership carries
    // the official native role (see finalize_native_operator_roles).
    assert!(aggregate_group.native_operator_refs.is_empty());
    let observations = output
        .ir
        .metric_observations
        .iter()
        .filter(|metric| {
            metric.subject_ref.as_deref() == Some(aggregate_group.operator_group_id.as_str())
                && metric.value_role == "reported_base"
        })
        .map(|metric| {
            (
                metric.source_ordinal,
                metric.runtime_occurrence_ref.as_deref(),
                metric.operator_ref.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(observations.len(), 2);
    assert!(
        observations
            .iter()
            .all(|(_, occurrence, ordered)| { occurrence.is_some() && ordered.is_none() })
    );
    assert_ne!(observations[0].0, observations[1].0);
    assert_ne!(observations[0].1, observations[1].1);
}

#[test]
fn runtime_profile_keeps_driver_occurrences_distinct_and_enforces_metric_budget() {
    let output = adapt_text(PIPELINE_DRIVER_OCCURRENCES_PROFILE).unwrap();
    assert!(output.source_view.plain_text_lines.iter().all(|line| {
        !line.text.contains("redacted-instance")
            && !line.text.contains("127.0.0.1")
            && !line.text.contains("TNetworkAddress")
    }));
    assert_eq!(output.ir.graph.nodes.len(), 2);
    assert_eq!(output.ir.graph.edges.len(), 1);
    assert!(
        !output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message_id == "runtime_operator_identity_duplicate")
    );

    let scans = output
        .ir
        .operator_groups
        .iter()
        .find(|group| group.plan_node_id.as_deref() == Some("0"))
        .unwrap();
    assert!(scans.native_operator_refs.is_empty());
    let scan_occurrences = output
        .ir
        .physical_operator_memberships
        .iter()
        .filter(|membership| membership.plan_node_id.as_deref() == Some("0"))
        .map(|membership| membership.physical_operator_ref.as_str())
        .collect::<Vec<_>>();
    assert_eq!(scan_occurrences.len(), 2);
    assert_ne!(scan_occurrences[0], scan_occurrences[1]);
    assert!(scan_occurrences.iter().all(|identity| {
        !identity.contains("redacted-instance")
            && !identity.contains("127.0.0.1")
            && !identity.contains("TNetworkAddress")
    }));
    assert!(
        output
            .ir
            .operator_groups
            .iter()
            .any(|group| group.plan_node_id.as_deref() == Some("-1"))
    );
    let scan_output_rows = output
        .ir
        .metric_observations
        .iter()
        .find(|metric| {
            metric.subject_ref.as_deref() == Some(scans.operator_group_id.as_str())
                && metric.metric_id.as_deref() == Some("OutputRows")
        })
        .unwrap();
    // Two driver occurrences of one native scan are an unmerged source: the
    // recipe never sums or picks one of them.
    assert_eq!(scan_output_rows.semantic_state, "unavailable");
    assert!(
        scan_output_rows
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "output_rows_source_merge_unverified"),
        "{:?}",
        scan_output_rows.diagnostics
    );
    let scan_times = output
        .ir
        .metric_observations
        .iter()
        .filter(|metric| metric.metric_id.as_deref() == Some("ScanTime"))
        .collect::<Vec<_>>();
    assert_eq!(scan_times.len(), 2);
    assert!(scan_times.iter().all(|metric| {
        metric
            .canonical_number
            .as_ref()
            .is_some_and(|number| number.value != "0")
    }));
    let pipeline = output
        .ir
        .operator_groups
        .iter()
        .find(|group| {
            group.canonical_type == "PIPELINE" && group.pipeline_ref.as_deref() == Some("0")
        })
        .unwrap();
    let driver_active = output
        .ir
        .metric_observations
        .iter()
        .filter(|metric| {
            metric.subject_ref.as_deref() == Some(pipeline.operator_group_id.as_str())
                && metric.metric_id.as_deref() == Some("ActiveTime")
                && metric.scope == "pipeline"
        })
        .count();
    assert_eq!(driver_active, 2);
    assert!(!output.ir.metric_observations.iter().any(|metric| {
        metric.subject_ref.as_deref() == Some(scans.operator_group_id.as_str())
            && metric.metric_id.as_deref() == Some("ActiveTime")
    }));

    let shifted = adapt_text(&format!("\n{PIPELINE_DRIVER_OCCURRENCES_PROFILE}")).unwrap();
    let shifted_scan_occurrences = shifted
        .ir
        .physical_operator_memberships
        .iter()
        .filter(|membership| membership.plan_node_id.as_deref() == Some("0"))
        .map(|membership| membership.physical_operator_ref.as_str())
        .collect::<Vec<_>>();
    assert_eq!(scan_occurrences, shifted_scan_occurrences);

    let invalid_driver = PIPELINE_DRIVER_OCCURRENCES_PROFILE.replacen(
        "- PendingTime: 40us",
        "- PendingTime: invalid",
        1,
    );
    let invalid_driver_output = adapt_text(&invalid_driver).unwrap();
    assert!(
        invalid_driver_output
            .ir
            .metric_observations
            .iter()
            .any(|metric| {
                metric.metric_id.as_deref() == Some("PendingTime")
                    && metric.parse_state != doctor_contracts::ParseState::Exact
            })
    );
    assert!(
        invalid_driver_output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| {
                diagnostic.affected_ids.iter().any(|id| id == "PendingTime")
                    && !diagnostic
                        .affected_ids
                        .iter()
                        .any(|id| id == "OperatorTotalTime")
            })
    );

    let mut resolved = output;
    let graph_evidence = resolved.graph_evidence.clone();
    resolve_version_and_graph(&mut resolved, graph_evidence).unwrap();
    let report = assemble_public_report(resolved).unwrap();
    let serialized = serde_json::to_string(&report).unwrap();
    for forbidden_instance_detail in ["redacted-instance", "127.0.0.1", "TNetworkAddress"] {
        assert!(
            !serialized.contains(forbidden_instance_detail),
            "public report serialization retained instance detail: {forbidden_instance_detail}"
        );
    }

    let limits = AdapterLimits {
        max_normalized_metrics: 1,
        ..AdapterLimits::default()
    };
    let error = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::TextProfile,
            LIVE_RUNTIME_PROFILE_TEXT.as_bytes(),
        ),
        &limits,
        &context(),
    )
    .unwrap_err();
    assert_eq!(error.code, "too_many_metrics");
}

#[test]
#[ignore = "requires local read-only StarRocks Docker E2E runtime profiles"]
fn runtime_profile_real_docker_samples_smoke() {
    let directory = std::env::var("SR_DOCTOR_E2E_DIR")
        .expect("set SR_DOCTOR_E2E_DIR to the local read-only E2E artifact directory");
    let baseline = std::fs::read_to_string(format!("{directory}/runtime-baseline-current.profile"))
        .expect("read runtime-baseline-current.profile");
    let aggregate =
        std::fs::read_to_string(format!("{directory}/runtime-baseline-aggregate.profile"))
            .expect("read runtime-baseline-aggregate.profile");
    assert_eq!(
        format!("{:x}", Sha256::digest(baseline.as_bytes())),
        "ea1a1b25c78d6f016aca0beb1286e6f3bda6f7a650e561178e3e6ecd8765518f"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(aggregate.as_bytes())),
        "c85264d92da0c40ac871cb14876fdafa43ad483a134c682aeb30a39fffcba0d6"
    );

    let mut baseline_output = adapt_text(&baseline).expect("baseline runtime profile adapts");
    assert!(!baseline_output.ir.metric_observations.is_empty());
    let graph_evidence = baseline_output.graph_evidence.clone();
    resolve_version_and_graph(&mut baseline_output, graph_evidence).unwrap();
    assert_eq!(baseline_output.ir.graph.state, GraphState::Unavailable);
    assert!(baseline_output.ir.graph.nodes.is_empty());
    assert!(baseline_output.ir.graph.edges.is_empty());
    let baseline_report = assemble_public_report(baseline_output).unwrap();
    assert!(
        baseline_report
            .operator_rows
            .iter()
            .flat_map(|row| row.metric_cells.iter())
            .count()
            > 0
    );

    let aggregate_output = adapt_text(&aggregate).expect("aggregate runtime profile adapts");
    assert!(!aggregate_output.ir.metric_observations.is_empty());
    assert!(!aggregate_output.ir.operator_groups.is_empty());
    assert!(
        !aggregate_output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message_id == "runtime_operator_identity_duplicate")
    );
}
#[test]
fn runtime_profile_keeps_missing_topology_and_formatter_type_conflict_partial() {
    let text_without_topology = r#"Query:
  Summary:
     - Query ID: 00000000-0000-0000-0000-000000000002
     - Query Type: Query
     - Query State: Finished
     - StarRocks Version: 3.5.14-23a56ec
     - Sql Statement: SELECT canary
  Execution:
     - QueryExecutionWallTime: 1ms
"#;
    let output = adapt_text(text_without_topology).unwrap();
    assert_eq!(output.ir.graph.state, GraphState::Unavailable);
    assert!(!output.ir.derived_metric_cells.is_empty());
    assert!(
        !output
            .source_view
            .plain_text_lines
            .iter()
            .any(|line| line.text.contains("canary"))
    );

    let type_conflict = r#"{
      "Query":{"Summary":{"Query Type":"Query","StarRocks Version":"3.5.14-23a56ec"},
      "Execution":{"QueryExecutionWallTime":"1ms","Fragment 0":{"Pipeline (id=0)":{
        "Active":"1ms","Active_ns":"not-a-number",
        "SCAN (plan_node_id=0)":{"CommonMetrics":{"OperatorTotalTime":"1ms"}}
      }}}}
    }"#;
    let output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, type_conflict.as_bytes()),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    assert!(
        output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "runtime_json_counter_type_invalid")
    );
    assert!(
        !output
            .ir
            .metric_observations
            .iter()
            .any(|metric| metric.metric_id.as_deref() == Some("Active"))
    );

    let unsupported_raw_counter = r#"{
      "Query":{"Summary":{"Query Type":"Query","StarRocks Version":"3.5.14-23a56ec"},
      "Execution":{"Fragment 0":{"Pipeline (id=0)":{
        "SCAN (plan_node_id=0)":{"CommonMetrics":{"OperatorTotalTime_ns":1000000}}
      }}}}
    }"#;
    let output = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            unsupported_raw_counter.as_bytes(),
        ),
        &AdapterLimits::default(),
        &context(),
    )
    .unwrap();
    assert!(
        output
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "runtime_json_raw_counter_not_supported" })
    );
    assert!(
        !output
            .ir
            .metric_observations
            .iter()
            .any(|metric| { metric.metric_id.as_deref() == Some("OperatorTotalTime") })
    );
}

#[test]
fn runtime_profile_text_duplicate_versions_are_partial_and_conflicts_lock_unknown() {
    let duplicate = r#"Query:
  Summary:
     - StarRocks Version: 3.5.14-first
     - StarRocks Version: 3.5.14-second
  Execution:
     - QueryExecutionWallTime: 1ms
"#;
    let output = adapt_text(duplicate).unwrap();
    assert_eq!(output.analysis_state, AnalysisState::Partial);
    assert_eq!(
        output
            .ir
            .version
            .reported_version
            .as_ref()
            .map(|version| version.value.as_str()),
        Some("3.5.14")
    );
    let duplicate_diagnostic = output
        .ir
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "duplicate_version")
        .unwrap();
    assert_eq!(duplicate_diagnostic.recoverability, "partial");
    assert_eq!(duplicate_diagnostic.source_refs, ["text:3", "text:4"]);

    let conflict = r#"Query:
  Summary:
     - StarRocks Version: 3.5.14-first
     - StarRocks Version: 3.4.0-conflict
     - StarRocks Version: 3.5.14-late
  Execution:
     - QueryExecutionWallTime: 1ms
"#;
    let output = adapt_text(conflict).unwrap();
    assert_eq!(output.analysis_state, AnalysisState::Partial);
    assert!(output.ir.version.reported_version.is_none());
    assert!(output.ir.version.effective_version.is_none());
    assert!(
        output
            .source_view
            .plain_text_lines
            .iter()
            .all(|line| !line.text.starts_with("StarRocks Version:")),
        "conflicting version evidence must not leave a single version in the safe projection"
    );
    let conflict_diagnostic = output
        .ir
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "version_conflict"
                && diagnostic.source_refs.iter().any(|item| item == "text:4")
        })
        .unwrap();
    assert_eq!(conflict_diagnostic.recoverability, "partial");
    assert_eq!(conflict_diagnostic.source_refs, ["text:3", "text:4"]);
    let late_diagnostic = output
        .ir
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "version_conflict"
                && diagnostic.source_refs.iter().any(|item| item == "text:5")
        })
        .unwrap();
    assert_eq!(late_diagnostic.source_refs, ["text:3", "text:5"]);
}

#[test]
fn serialized_public_reports_exclude_untrusted_runtime_summary_canaries() {
    let query_id_canary = "LEAK_QUERY_ID_CANARY";
    let query_type_canary = "LEAK_QUERY_TYPE_CANARY";
    let query_state_canary = "LEAK_QUERY_STATE_CANARY";
    let text = format!(
        r#"Query:
  Summary:
     - Query ID: {query_id_canary}
     - Query Type: {query_type_canary}
     - Query State: {query_state_canary}
     - StarRocks Version: 3.5.14-23a56ec
  Execution:
     - QueryExecutionWallTime: 1ms
"#
    );
    let json = format!(
        r#"{{
          "Query": {{
            "Summary": {{
              "Query ID": "{query_id_canary}",
              "Query Type": "{query_type_canary}",
              "Query State": "{query_state_canary}",
              "StarRocks Version": "3.5.14-23a56ec"
            }},
            "Execution": {{"QueryExecutionWallTime": "1ms"}}
          }}
        }}"#
    );
    let reports = [
        assemble_public_report(adapt_text(&text).unwrap()).unwrap(),
        assemble_public_report(
            adapt_input(
                InputEnvelope::new(RequestedSourceKind::JsonProfile, json.as_bytes()),
                &AdapterLimits::default(),
                &context(),
            )
            .unwrap(),
        )
        .unwrap(),
    ];

    for report in reports {
        let summary = report.query_summary.as_ref().unwrap();
        assert!(summary.query_id.is_none());
        assert!(summary.query_type.is_none());
        assert_eq!(report.profile_context.query_state, QueryState::Unknown);
        for code in [
            "runtime_query_id_invalid",
            "runtime_query_type_invalid",
            "profile_context_invalid",
        ] {
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == code),
                "missing diagnostic {code}"
            );
        }
        let serialized = serde_json::to_string(&report).unwrap();
        for canary in [query_id_canary, query_type_canary, query_state_canary] {
            assert!(
                !serialized.contains(canary),
                "serialized public report leaked {canary}"
            );
        }
    }
}
