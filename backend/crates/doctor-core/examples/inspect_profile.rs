//! Offline production-pipeline replay. Prints counts/enums/hashes, never raw input or SQL.
use std::{fs::File, io::Read, process::ExitCode};

use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};
use serde_json::{Value, json};

fn len(value: &Value) -> usize {
    value.as_array().map_or(0, Vec::len)
}

fn run() -> Result<Value, &'static str> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: inspect_profile LOCAL_TEXT_PROFILE")?;
    if args.next().is_some() {
        return Err("usage: inspect_profile LOCAL_TEXT_PROFILE");
    }
    let limits = AdapterLimits::default();
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "input_open_failed")?
        .take(limits.max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "input_read_failed")?;
    if bytes.len() > limits.max_bytes {
        return Err("input_byte_budget_exceeded");
    }
    let byte_count = bytes.len();
    let context = AdapterContext::new(v01_analysis_receipt()).map_err(|_| "receipt_failed")?;
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, bytes),
        &limits,
        &context,
    )
    .map_err(|_| "adapter_failed")?;
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).map_err(|_| "version_graph_failed")?;
    evaluate_rules(&mut output).map_err(|_| "rules_failed")?;
    let observation_count = output.ir.metric_observations.len();
    let report = assemble_public_report(output).map_err(report_error_code)?;
    let report = serde_json::to_value(report).map_err(|_| "report_encoding_failed")?;
    let graph = &report["graph_view"];
    let summary = &report["execution_summary"];
    let rows = report["operator_rows"]
        .as_array()
        .ok_or("report_shape_failed")?;
    let cells = rows
        .iter()
        .flat_map(|row| row["metric_cells"].as_array().into_iter().flatten())
        .chain(
            report["derived_metric_cells"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .collect::<Vec<_>>();
    // A fixed projection: never serialize labels, source_view, query IDs, SQL,
    // version lexemes or source locators.
    let states = [
        "exact",
        "missing",
        "invalid",
        "partial",
        "overflow",
        "unavailable",
        "incompatible",
        "unknown",
    ];
    let state_counts = states
        .into_iter()
        .map(|state| {
            (
                state.to_owned(),
                json!(cells.iter().filter(|cell| cell["state"] == state).count()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let nodes = summary["nodes"].as_array().ok_or("summary_shape_failed")?;
    let mut ranked = nodes
        .iter()
        .filter_map(|node| Some((node["time_share_basis_points"].as_u64()?, node)))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(basis_points, _)| std::cmp::Reverse(*basis_points));
    let top_five = ranked
        .iter()
        .take(5)
        .map(|(basis_points, node)| {
            json!({
                "graph_node_id": node["node_id"],
                "time_share_basis_points": basis_points,
                "hotspot": node["hotspot"],
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "input_bytes": byte_count,
        "source_hash": report["input"]["source_hash"],
        "facts_hash": report["facts_hash"],
        "analysis_key": report["analysis_key"],
        "analysis_state": report["analysis_state"],
        "pack_match": report["version"]["pack_match"],
        "compatibility": report["version"]["compatibility"],
        "profile_level": report["profile_context"]["profile_level"],
        "query_state": report["profile_context"]["query_state"],
        "merge_state": report["profile_context"]["merge_state"],
        "graph_state": graph["state"],
        "graph_nodes": len(&graph["nodes"]),
        "graph_edges": len(&graph["edges"]),
        "graph_unresolved": len(&graph["unresolved_relations"]),
        "summary_state": summary["state"],
        "summary_nodes": nodes.len(),
        "summary_nodes_with_exact_time": nodes.iter().filter(|node| node["total_time"]["state"] == "exact").count(),
        "summary_nodes_with_exact_rows": nodes.iter().filter(|node| node["output_rows"]["state"] == "exact").count(),
        "summary_top_five": top_five,
        "operator_rows": rows.len(),
        "metric_observations": observation_count,
        "operator_and_derived_cells": cells.len(),
        "operator_and_derived_cell_states": state_counts,
        "findings": len(&report["findings"]),
        "rule_evaluations": len(&report["rule_evaluations"]),
        "diagnostics": len(&report["diagnostics"]),
        "source_ref_count": len(&report["provenance_index"]),
        "receipt": report["analysis_receipt"]
    }))
}

fn main() -> ExitCode {
    match run().and_then(|summary| {
        serde_json::to_string_pretty(&summary).map_err(|_| "summary_encoding_failed")
    }) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(stage) => {
            eprintln!("{stage}");
            ExitCode::FAILURE
        }
    }
}

// Static variant names only; payloads can contain production identifiers.
fn report_error_code(error: doctor_core::PublicReportError) -> &'static str {
    use doctor_core::PublicReportError;
    match error {
        PublicReportError::ExactMetricMissingValue => "PublicReportError.ExactMetricMissingValue",
        PublicReportError::MissingMetricObservation { .. } => {
            "PublicReportError.MissingMetricObservation"
        }
        PublicReportError::DuplicateMetricObservation { .. } => {
            "PublicReportError.DuplicateMetricObservation"
        }
        PublicReportError::MissingDerivedMetricObservation { .. } => {
            "PublicReportError.MissingDerivedMetricObservation"
        }
        PublicReportError::AmbiguousDerivedMetricObservation { .. } => {
            "PublicReportError.AmbiguousDerivedMetricObservation"
        }
        PublicReportError::MissingPublicMetricContainer { .. } => {
            "PublicReportError.MissingPublicMetricContainer"
        }
        PublicReportError::AmbiguousPublicMetricContainer { .. } => {
            "PublicReportError.AmbiguousPublicMetricContainer"
        }
        PublicReportError::MissingPublicMetricReference { .. } => {
            "PublicReportError.MissingPublicMetricReference"
        }
        PublicReportError::AmbiguousPublicMetricReference { .. } => {
            "PublicReportError.AmbiguousPublicMetricReference"
        }
        PublicReportError::Cancelled => "PublicReportError.Cancelled",
    }
}
