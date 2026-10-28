use std::collections::BTreeMap;

use doctor_contracts::{RequestedSourceKind, SourceKind, canonical_hash};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

const SHORT_CHAIN: &str = include_str!(
    "../../../../fixtures/runtime-profiles/starrocks-3.5.14/p04/short-chain.profile.txt"
);
const CROSS_FRAGMENT: &str = include_str!(
    "../../../../fixtures/synthetic/3.5.14/sr3514_cross_fragment_exchange.profile.txt"
);
const RUNTIME_DRIVER_OCCURRENCES: &str = include_str!(
    "../../../../fixtures/runtime-profiles/starrocks-3.5.14/p04/runtime-driver-occurrences.profile.txt"
);

/// Production-derived captures exist only in the private repository; the public
/// one leaves out the profiles and their goldens. Both are present or neither.
const PRIVATE_FIXTURES: [(&str, &str, &str); 2] = [
    (
        "production-derived-multi-pipeline",
        "fixtures/runtime-profiles/starrocks-3.5.14/p04/production-derived-multi-pipeline.profile.txt",
        "fixtures/expected/3.5.14/p04/production-derived-multi-pipeline.report.json",
    ),
    (
        "production-complex-synthetic",
        "fixtures/runtime-profiles/starrocks-3.5.14/p04/production-complex-synthetic.profile.txt",
        "fixtures/expected/3.5.14/p04/production-complex-synthetic.report.json",
    ),
];

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The private fixtures with their profile text, or none in the public repository.
fn private_fixtures() -> Vec<(&'static str, String, &'static str)> {
    let present = PRIVATE_FIXTURES
        .iter()
        .filter(|(_, input, _)| repo_root().join(input).exists())
        .count();
    assert!(
        present == 0 || present == PRIVATE_FIXTURES.len(),
        "private P04 fixtures must be all present or all absent"
    );
    PRIVATE_FIXTURES
        .iter()
        .take(present)
        .map(|(id, input, golden)| {
            let text = std::fs::read_to_string(repo_root().join(input)).expect("private profile");
            (*id, text, *golden)
        })
        .collect()
}

fn private_fixture(fixture_id: &str) -> Option<String> {
    private_fixtures()
        .into_iter()
        .find(|(id, _, _)| *id == fixture_id)
        .map(|(_, text, _)| text)
}

/// Public fixtures, then the private ones when this repository has them.
fn p04_reports() -> Vec<(
    &'static str,
    doctor_contracts::AnalysisSnapshotReport,
    &'static str,
)> {
    let mut reports = P04_GOLDENS
        .iter()
        .zip([SHORT_CHAIN, CROSS_FRAGMENT, RUNTIME_DRIVER_OCCURRENCES])
        .map(|((id, golden), input)| (*id, report(input), *golden))
        .collect::<Vec<_>>();
    for (id, text, golden) in private_fixtures() {
        reports.push((id, report(&text), golden));
    }
    reports
}

const P04_GOLDENS: [(&str, &str); 3] = [
    (
        "short-chain",
        "fixtures/expected/3.5.14/p04/short-chain.report.json",
    ),
    (
        "cross-fragment-degradation",
        "fixtures/expected/3.5.14/p04/cross-fragment-degradation.report.json",
    ),
    (
        "runtime-driver-occurrences",
        "fixtures/expected/3.5.14/p04/runtime-driver-occurrences.report.json",
    ),
];

fn metric_state_counts(cells: impl Iterator<Item = serde_json::Value>) -> serde_json::Value {
    let mut counts = BTreeMap::<String, u64>::new();
    for cell in cells {
        if let Some(state) = cell.get("state").and_then(serde_json::Value::as_str) {
            *counts.entry(state.to_owned()).or_default() += 1;
        }
    }
    serde_json::to_value(counts).expect("metric state counts")
}

fn graph_view_counts(report: &serde_json::Value) -> serde_json::Value {
    let graph = &report["graph_view"];
    serde_json::json!({
        "state": graph["state"],
        "topology_source": graph["topology_source"],
        "node_count": graph["nodes"].as_array().map_or(0, Vec::len),
        "edge_count": graph["edges"].as_array().map_or(0, Vec::len),
        "root_count": graph["root_node_ids"].as_array().map_or(0, Vec::len),
        "unconnected_count": graph["unconnected_node_ids"].as_array().map_or(0, Vec::len),
        "unresolved_count": graph["unresolved_relations"].as_array().map_or(0, Vec::len),
    })
}

fn execution_summary_counts(report: &serde_json::Value) -> serde_json::Value {
    let summary = &report["execution_summary"];
    let nodes = summary["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let exact = |field: &str| {
        nodes
            .iter()
            .filter(|node| node[field]["state"] == "exact")
            .count()
    };
    serde_json::json!({
        "state": summary["state"],
        "reasons": summary["reasons"],
        "node_count": nodes.len(),
        "exact_time_count": exact("total_time"),
        "exact_rows_count": exact("output_rows"),
        "hotspot_count": nodes.iter().filter(|node| node["hotspot"] == "most_consuming" || node["hotspot"] == "second_most_consuming").count(),
    })
}

fn report(input: &str) -> doctor_contracts::AnalysisSnapshotReport {
    let context = AdapterContext::new(v01_analysis_receipt()).expect("valid receipt");
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, input.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .expect("sanitized fixture adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("version and graph resolve");
    evaluate_rules(&mut output).expect("existing rules evaluate");
    assemble_public_report(output).expect("public report assembles")
}

#[test]
fn production_derived_multi_pipeline_fixture_replays_without_raw_source() {
    let Some(text) = private_fixture("production-derived-multi-pipeline") else {
        return; // public repository: the production-derived capture is not included
    };
    let report = report(&text);
    assert_eq!(report.input.detected_kind, SourceKind::TextProfile);
    assert!(!report.graph_view.nodes.is_empty());
    assert!(!report.operator_rows.is_empty());
}

#[test]
fn p04_tiers_remain_adapter_assembled_and_never_fabricate_edges() {
    let short = report(SHORT_CHAIN);
    assert_eq!(short.graph_view.nodes.len(), 3);
    assert_eq!(short.graph_view.edges.len(), 2);
    assert!(
        short
            .operator_rows
            .iter()
            .any(|row| row.operator_type == "OLAP_SCAN")
    );

    // The same plan node id in two fragments does not prove an exchange link.
    let cross = report(CROSS_FRAGMENT);
    assert!(cross.graph_view.edges.is_empty());
    assert_eq!(cross.graph_view.unresolved_relations.len(), 1);

    let Some(complex) = private_fixture("production-complex-synthetic") else {
        return;
    };
    let complex = report(&complex);
    assert_eq!(complex.graph_view.nodes.len(), 45);
    assert_eq!(complex.operator_rows.len(), 45);
    // Synthetic captures carry no merge evidence; the official per-node reading
    // needs a merged profile, so the summary is explicitly unavailable.
    assert!(complex.execution_summary.nodes.is_empty());
    assert_eq!(complex.execution_summary.reasons, ["profile_not_merged"]);
}
#[test]
fn p04_checked_in_goldens_match_the_adapter_assembly() {
    for (fixture_id, report, golden_path) in p04_reports() {
        let expected = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(repo_root().join(golden_path))
                .expect("P04 golden is checked in"),
        )
        .expect("P04 golden is JSON");
        assert_eq!(
            serde_json::to_value(report).expect("P04 report JSON"),
            expected,
            "{fixture_id}"
        );
    }
}

#[test]
fn p04_manifest_is_a_replay_receipt_for_every_checked_in_golden() {
    let manifest = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(
            repo_root().join("fixtures/manifests/sr3514-complex-profile-regression.json"),
        )
        .expect("P04 manifest"),
    )
    .expect("P04 manifest JSON");
    let actual = p04_reports();
    let tiers = manifest["tiers"].as_array().expect("P04 tiers");
    assert_eq!(
        tiers.len(),
        actual.len(),
        "one manifest tier per fixture in this repository"
    );

    for (fixture_id, report, _) in actual {
        let report = serde_json::to_value(report).expect("P04 report JSON");
        let tier = tiers
            .iter()
            .find(|tier| tier["fixture_id"] == fixture_id)
            .expect("P04 tier receipt");
        assert_eq!(tier["analysis_state"], report["analysis_state"]);
        assert_eq!(tier["facts_hash"], report["facts_hash"]);
        assert_eq!(tier["analysis_key"], report["analysis_key"]);
        assert_eq!(tier["report_canonical_sha256"], canonical_hash(&report));
        assert_eq!(tier["graph_view"], graph_view_counts(&report));
        assert_eq!(tier["execution_summary"], execution_summary_counts(&report));
        assert_eq!(
            tier["operator_metric_state_counts"],
            metric_state_counts(
                report["operator_rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|row| row["metric_cells"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .cloned()),
            )
        );
        assert_eq!(
            tier["derived_metric_state_counts"],
            metric_state_counts(
                report["derived_metric_cells"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned()
            )
        );
    }
}
#[test]
fn p04_same_bytes_replay_has_stable_report_and_hashes() {
    let private = private_fixtures();
    let inputs = [SHORT_CHAIN, CROSS_FRAGMENT, RUNTIME_DRIVER_OCCURRENCES]
        .into_iter()
        .chain(private.iter().map(|(_, text, _)| text.as_str()));
    for input in inputs {
        let first = report(input);
        let second = report(input);
        assert_eq!(first, second);
        assert_eq!(first.input.source_hash, second.input.source_hash);
        assert_eq!(first.facts_hash, second.facts_hash);
        assert_eq!(first.analysis_key, second.analysis_key);
    }
}
