//! v0.6-04: every finding publishes what its rule measured, in the rule's own
//! terms, from the same typed inputs the rule compared. The browser renders
//! these values verbatim; this test pins them on the official 1 FE + 3 BE
//! 3.5.14 corpus and checks each input resolves to exactly one public cell.

use std::{collections::BTreeMap, fs, path::PathBuf};

use doctor_contracts::{AnalysisSnapshotReport, RequestedSourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

fn fixture(dir: &str, name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/runtime/3.5.14")
        .join(dir)
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()))
}

/// `(measurement_id, display_text, threshold_text, operator_type)`.
type Measured = (String, String, Option<String>, Option<String>);

fn report(dir: &str, name: &str) -> AnalysisSnapshotReport {
    let profile = fixture(dir, name);
    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, profile.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .expect("Profile adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("Profile resolves");
    evaluate_rules(&mut output).expect("rules evaluate");
    assemble_public_report(output).expect("report assembles")
}

fn multi_be(name: &str) -> AnalysisSnapshotReport {
    report("oracle-corpus-multi-be", name)
}

/// `rule suffix -> [(measurement_id, display_text, threshold_text, operator_type)]`.
fn measurements(report: &AnalysisSnapshotReport) -> BTreeMap<String, Vec<Measured>> {
    report
        .findings
        .iter()
        .map(|finding| {
            (
                format!(
                    "{}@{}",
                    finding.rule_id.trim_start_matches("SR3514."),
                    finding.subject_ref.as_deref().unwrap_or("query")
                ),
                finding
                    .measurements
                    .iter()
                    .map(|item| {
                        (
                            item.measurement_id.clone(),
                            item.display_text.clone(),
                            item.threshold_text.clone(),
                            item.operator_type.clone(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn owned(values: &[(&str, &str, Option<&str>, Option<&str>)]) -> Vec<Measured> {
    values
        .iter()
        .map(|(id, text, threshold, operator)| {
            (
                (*id).into(),
                (*text).into(),
                threshold.map(Into::into),
                operator.map(Into::into),
            )
        })
        .collect()
}

#[test]
fn skew_finding_states_slowest_over_mean_and_the_driver_times() {
    let report = multi_be("c15_skewed_shuffle_join.profile.txt");
    assert_eq!(
        measurements(&report)["OPERATOR_TIME_SKEW@operator-group:2:4"],
        owned(&[
            (
                "slowest_over_mean_driver_time",
                "10.3 – 10.4×",
                Some("> 2×"),
                Some("HASH_JOIN_PROBE")
            ),
            (
                "slowest_driver_time",
                "499.854 ms",
                Some("≥ 100 ms"),
                Some("HASH_JOIN_PROBE")
            ),
            (
                "mean_driver_time",
                "48.241 ms",
                None,
                Some("HASH_JOIN_PROBE")
            ),
            (
                "fastest_driver_time",
                "2.001 ms",
                None,
                Some("HASH_JOIN_PROBE")
            ),
        ])
    );
}

#[test]
fn why_findings_state_their_ratio_bytes_or_kept_rows() {
    let fanout = measurements(&multi_be("c16_join_fanout.profile.txt"));
    assert_eq!(
        fanout["JOIN_PROBE_FANOUT@operator-group:2:3"],
        owned(&[
            ("probe_output_over_input_rows", "20.0×", Some("> 10×"), None),
            ("probe_input_rows", "1,000,000", None, None),
            ("probe_output_rows", "20,000,000", Some("≥ 100,000"), None),
        ])
    );
    assert_eq!(
        fanout["ROW_FLOW_AMPLIFICATION@operator-group:2:3"],
        owned(&[
            (
                "output_over_child_output_rows",
                "19.6×",
                Some("> 10×"),
                None
            ),
            ("node_output_rows", "20,000,000", None, None),
            ("child_output_rows", "1,020,000", None, None),
        ])
    );
    assert_eq!(
        measurements(&multi_be("c17_large_broadcast.profile.txt"))["BROADCAST_EXCHANGE_BYTES@operator-group:2:2"],
        owned(&[(
            "broadcast_bytes_sent",
            "576.647 MB",
            Some("> 100 MB"),
            Some("EXCHANGE_SINK")
        )])
    );
    assert_eq!(
        measurements(&multi_be("c18_high_cardinality_agg.profile.txt"))["STREAMING_PREAGG_INEFFECTIVE@operator-group:2:1"],
        owned(&[
            (
                "preaggregation_output_over_input_rows",
                "100.0%",
                Some("≥ 90%"),
                None
            ),
            (
                "preaggregation_input_rows",
                "3,000,000",
                Some("≥ 1,000,000"),
                Some("AGGREGATE_STREAMING_SINK")
            ),
            (
                "preaggregation_output_rows",
                "3,000,000",
                None,
                Some("AGGREGATE_STREAMING_SOURCE")
            ),
        ])
    );
}

/// A time-share finding's share and time are the execution summary's own
/// display of its node; every measurement input is one public metric cell.
#[test]
fn every_corpus_finding_is_measured_from_public_cells() {
    let manifest: serde_json::Value =
        serde_json::from_str(&fixture("oracle-corpus-multi-be", "manifest.json"))
            .expect("manifest");
    let mut findings = 0;
    for case in manifest["cases"].as_array().expect("cases") {
        let name = case["profile_file"].as_str().expect("profile file");
        let report = multi_be(name);
        let mut cells = BTreeMap::<&str, usize>::new();
        for cell in report.derived_metric_cells.iter().chain(
            report
                .operator_rows
                .iter()
                .flat_map(|row| row.metric_cells.iter()),
        ) {
            *cells.entry(cell.metric_cell_ref.as_str()).or_default() += 1;
        }
        for finding in &report.findings {
            findings += 1;
            assert!(
                !finding.measurements.is_empty(),
                "{name}: {} has no measurement",
                finding.finding_id
            );
            for item in &finding.measurements {
                assert!(
                    !item.input_refs.is_empty(),
                    "{name}: {} without inputs",
                    item.measurement_id
                );
                for input in &item.input_refs {
                    assert_eq!(
                        cells.get(input.as_str()),
                        Some(&1),
                        "{name}: {input} is one public cell"
                    );
                }
            }
            if finding.rule_id.ends_with("_TIME_SHARE") {
                let group = report
                    .operator_rows
                    .iter()
                    .find(|row| Some(row.operator_ref.as_str()) == finding.subject_ref.as_deref())
                    .expect("subject row");
                let node = report
                    .execution_summary
                    .nodes
                    .iter()
                    .filter(|node| Some(&node.plan_node_id) == group.plan_node_id.as_ref())
                    .find(|node| {
                        finding.subject_ref.as_deref()
                            == Some(&format!(
                                "operator-group:{}:{}",
                                node.fragment_ref.as_deref().unwrap_or("unknown"),
                                node.plan_node_id
                            ))
                    })
                    .expect("summary node");
                assert_eq!(finding.measurements[0].measurement_id, "node_time_share");
                assert_eq!(
                    Some(&finding.measurements[0].display_text),
                    node.time_share_display.as_ref()
                );
                assert_eq!(
                    Some(&finding.measurements[1].display_text),
                    node.total_time.display_text.as_ref()
                );
            }
        }
    }
    assert!(findings >= 36, "only {findings} findings measured");
}

#[test]
fn fail_closed_profiles_publish_no_measured_finding() {
    for name in [
        "s01_level2_shuffle_join.profile.txt",
        "s02_level2_multi_instance_agg.profile.txt",
        "s03_running_snapshot.profile.txt",
    ] {
        let report = report("profile-states", name);
        assert!(report.findings.is_empty(), "{name}: no finding");
        assert!(
            report
                .rule_evaluations
                .iter()
                .all(|evaluation| evaluation.status
                    == doctor_contracts::RuleEvaluationStatus::NotApplicable),
            "{name}: no rule evaluated"
        );
    }
}
