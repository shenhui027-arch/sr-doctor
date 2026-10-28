//! Rule and summary inputs exactly as the 3.5.14 FE prints them, starting from
//! a real Profile captured from the pinned image (oracle corpus c01) and
//! changing only the line under test: the spill counter's query subject and
//! its "%.3f B" / rounded byte values, and a truncated OperatorTotalTime.

use doctor_contracts::{
    AnalysisSnapshotReport, RequestedSourceKind, RuleEvaluationStatus, SummaryValueState,
};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

const C01: &str =
    include_str!("../../../../fixtures/runtime/3.5.14/oracle-corpus/c01_scan_filter.profile.txt");
const OFFICIAL_SPILL_LINE: &str = "     - QuerySpillBytes: 0.000 B";
const PROJECT_TIME_LINE: &str = "- OperatorTotalTime: 14.199us";

fn analyze(text: String) -> AnalysisSnapshotReport {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, text.into_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).unwrap();
    evaluate_rules(&mut output).unwrap();
    assemble_public_report(output).unwrap()
}

fn with_spill(value: &str) -> AnalysisSnapshotReport {
    assert!(
        C01.contains(OFFICIAL_SPILL_LINE),
        "fixture keeps the official spill line"
    );
    analyze(C01.replacen(
        OFFICIAL_SPILL_LINE,
        &format!("     - QuerySpillBytes: {value}"),
        1,
    ))
}

fn spill(report: &AnalysisSnapshotReport) -> (RuleEvaluationStatus, Vec<String>, Vec<String>) {
    let evaluation = report
        .rule_evaluations
        .iter()
        .find(|evaluation| evaluation.rule_id == "SR3514.SPILL_OBSERVED")
        .expect("spill rule evaluated");
    (
        evaluation.status.clone(),
        evaluation.comparisons.clone(),
        evaluation.missing_prerequisites.clone(),
    )
}

#[test]
fn the_official_zero_spill_line_is_an_exact_not_triggered_query_metric() {
    let (status, comparisons, missing) = spill(&analyze(C01.to_owned()));
    assert_eq!(
        status,
        RuleEvaluationStatus::NotTriggered,
        "{comparisons:?} {missing:?}"
    );
    assert!(
        comparisons
            .iter()
            .any(|comparison| comparison == "canonical_exact_zero"),
        "{comparisons:?}"
    );
}

#[test]
fn sub_kilobyte_and_rounded_spill_values_trigger_the_rule() {
    for value in ["512.000 B", "1.234 GB"] {
        let (status, comparisons, missing) = spill(&with_spill(value));
        assert_eq!(
            status,
            RuleEvaluationStatus::Triggered,
            "{value}: {comparisons:?} {missing:?}"
        );
    }
}

#[test]
fn a_truncated_profile_missing_operator_total_time_is_unavailable_not_zero() {
    let lines = C01.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.trim() == PROJECT_TIME_LINE)
        .expect("fixture keeps PROJECT's OperatorTotalTime");
    // Drop the base counter and its __MAX_OF_ / __MIN_OF_ children.
    let truncated = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !(start..start + 3).contains(index))
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n");
    let report = analyze(truncated);
    let project = report
        .execution_summary
        .nodes
        .iter()
        .find(|node| node.plan_node_id == "1")
        .expect("PROJECT summarized");
    assert_eq!(project.total_time.state, SummaryValueState::Unavailable);
    assert_eq!(
        project.total_time.reasons,
        vec!["operator_total_time_missing".to_owned()]
    );
    assert!(project.time_share_basis_points.is_none());
}
