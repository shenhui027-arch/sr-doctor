//! v0.6-07: the report names the line of every text source reference, so the
//! node inspector can say "line 885" for a counter line that the safe source
//! view does not carry, without the browser parsing `text:885`.

use std::collections::BTreeSet;

use doctor_contracts::{AnalysisSnapshotReport, RequestedSourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

const C16: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/oracle-corpus-multi-be/c16_join_fanout.profile.txt"
);

fn analyze(text: &str) -> AnalysisSnapshotReport {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, text.as_bytes().to_vec()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).unwrap();
    evaluate_rules(&mut output).unwrap();
    assemble_public_report(output).unwrap()
}

#[test]
fn every_text_reference_of_a_plan_node_has_its_source_line() {
    let report = analyze(C16);
    let view = &report.source_view;
    let lines = view
        .line_refs
        .iter()
        .map(|line| (line.source_ref.as_str(), line.line_no))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(lines.len(), view.line_refs.len(), "one line per reference");
    let provenance = report
        .provenance_index
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert!(lines.keys().all(|reference| provenance.contains(reference)));

    // The references the node inspector lists all resolve to a line.
    let mut node_refs = BTreeSet::new();
    for node in &report.graph_view.nodes {
        node_refs.extend(node.source_refs.iter().map(String::as_str));
    }
    for summary in &report.execution_summary.nodes {
        node_refs.extend(summary.total_time.source_refs.iter().map(String::as_str));
        node_refs.extend(summary.output_rows.source_refs.iter().map(String::as_str));
    }
    assert!(!node_refs.is_empty());
    for reference in &node_refs {
        assert!(lines.contains_key(reference), "{reference} has a line");
    }

    // The line agrees with the safe text view wherever both carry the reference,
    // and the view itself still carries no counter text beyond its allowlist.
    for line in &view.plain_text_lines {
        assert_eq!(lines.get(line.source_ref.as_str()), Some(&line.line_no));
    }
    assert!(view.plain_text_lines.len() < view.line_refs.len());
    assert!(
        view.limitations
            .contains(&"runtime_profile_safe_allowlist_only".to_owned())
    );
}
