//! In-memory replay of every `EXPLAIN COSTS` plan in an external production
//! capture Markdown file. Only typed counts, enums and hashes are written;
//! source SQL, plan lines and paths are never persisted or printed.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    process::ExitCode,
};

use doctor_contracts::ExplainRuntimeState;
use doctor_core::{
    ExplainLimits, ExplainPlanNode, ExplainSnapshotInput, ExplainStructureState,
    assemble_explain_snapshot, normalize_explain_costs,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: &str = "sr3514.production_explain_corpus.receipt.v1";
const CAPTURE_MARKER: &str = "<!-- SR_PROFILE_CAPTURE";
const EXPLAIN_HEADING: &str = "### EXPLAIN COSTS";
const RAW_PROFILE_HEADING: &str = "### Raw profile";
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const EXPECTED_SOURCE_BYTES: usize = 2_241_478;
const EXPECTED_CAPTURE_COUNT: usize = 79;
const EXPECTED_NONEMPTY_RUNTIME_PROFILES: usize = 1;
const EXPECTED_SOURCE_SHA256: &str =
    "d0b780919b298f75f513035cd0c472c5f524628ceee94b67cb8aaea9d38e0370";
const OFFICIAL_3514_COMMIT: &str = "23a56ec517db763ea9bc7ee50b19357acdee0303";
const TOOL_SOURCE: &[u8] = include_bytes!("inspect_explain_corpus.rs");

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn state_name(state: &ExplainStructureState) -> &'static str {
    match state {
        ExplainStructureState::Complete => "complete",
        ExplainStructureState::Partial => "partial",
        ExplainStructureState::Unavailable => "unavailable",
    }
}

fn node_kind_name(node: &ExplainPlanNode) -> String {
    format!("{:?}", node.kind)
}

fn validate_frozen_source_identity(
    source_bytes: usize,
    source_hash: &str,
) -> Result<(), &'static str> {
    if source_bytes != EXPECTED_SOURCE_BYTES {
        return Err("source_size_mismatch");
    }
    if source_hash != EXPECTED_SOURCE_SHA256 {
        return Err("source_hash_mismatch");
    }
    Ok(())
}

fn fenced_payload_after<'a>(block: &'a str, heading: &str) -> Result<&'a str, &'static str> {
    if block.match_indices(heading).count() != 1 {
        return Err("section_heading_count_invalid");
    }
    let heading_at = block.find(heading).ok_or("section_heading_missing")?;
    let after_heading = &block[heading_at + heading.len()..];
    let fence = after_heading.find("```").ok_or("section_fence_missing")?;
    let fenced = &after_heading[fence + 3..];
    let payload_start = fenced.find('\n').ok_or("section_fence_invalid")? + 1;
    let payload_and_tail = &fenced[payload_start..];
    let payload_end = payload_and_tail
        .find("```")
        .ok_or("section_fence_unclosed")?;
    Ok(&payload_and_tail[..payload_end])
}

fn extract_corpus(source: &str) -> Result<(Vec<Vec<String>>, usize), &'static str> {
    let blocks = source.split(CAPTURE_MARKER).skip(1).collect::<Vec<_>>();
    if blocks.is_empty() || blocks.len() > 10_000 {
        return Err("capture_count_invalid");
    }
    let mut capture_ids = BTreeSet::new();
    let mut nonempty_runtime_profiles = 0_usize;
    let plans = blocks
        .into_iter()
        .map(|block| {
            let marker_end = block.find("-->").ok_or("capture_marker_unclosed")?;
            let targets = block[..marker_end]
                .split_ascii_whitespace()
                .filter_map(|field| field.strip_prefix("target="))
                .collect::<Vec<_>>();
            if targets.len() != 1 {
                return Err("capture_index_invalid");
            }
            let target = targets[0]
                .parse::<usize>()
                .map_err(|_| "capture_index_invalid")?;
            let capture_id_fields = block[..marker_end]
                .split_ascii_whitespace()
                .filter_map(|field| field.strip_prefix("capture_id="))
                .collect::<Vec<_>>();
            if target == 0 || capture_id_fields.len() != 1 || capture_id_fields[0].is_empty() {
                return Err("capture_marker_invalid");
            }
            if !capture_ids.insert(capture_id_fields[0].to_owned()) {
                return Err("capture_id_duplicate");
            }
            let block = &block[marker_end + 3..];
            let rows: Value = serde_json::from_str(fenced_payload_after(block, EXPLAIN_HEADING)?)
                .map_err(|_| "explain_rows_invalid")?;
            let rows = rows.as_array().ok_or("explain_rows_not_array")?;
            if rows.is_empty() {
                return Err("explain_rows_empty");
            }
            let mut plan = Vec::with_capacity(rows.len());
            for row in rows {
                let line = row
                    .get("Explain String")
                    .and_then(Value::as_str)
                    .ok_or("explain_row_shape_invalid")?;
                plan.push(line.to_owned());
            }
            if !fenced_payload_after(block, RAW_PROFILE_HEADING)?
                .trim()
                .is_empty()
            {
                nonempty_runtime_profiles += 1;
            }
            Ok(plan)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((plans, nonempty_runtime_profiles))
}

#[cfg(test)]
fn extract_explain_plans(source: &str) -> Result<Vec<Vec<String>>, &'static str> {
    extract_corpus(source).map(|(plans, _)| plans)
}

fn safe_projection(facts: &doctor_core::ExplainPlanFacts) -> Value {
    json!({
        "state": state_name(&facts.state),
        "fragments": facts.fragments.iter().map(|fragment| json!({
            "fragment_ref": fragment.fragment_ref,
            "ordinal": fragment.ordinal,
            "sink_kind": format!("{:?}", fragment.sink_kind),
        })).collect::<Vec<_>>(),
        "nodes": facts.nodes.iter().map(|node| json!({
            "node_ref": node.node_ref,
            "fragment_ref": node.fragment_ref,
            "plan_node_id": node.plan_node_id,
            "kind": node_kind_name(node),
        })).collect::<Vec<_>>(),
        "relations": facts.safe_structure.relation_refs,
        "limitations": facts.limitations.iter().map(|item| item.code.as_str()).collect::<Vec<_>>(),
    })
}

fn inspect_source(source_bytes: &[u8], enforce_frozen_source: bool) -> Result<Value, &'static str> {
    if source_bytes.len() > MAX_SOURCE_BYTES {
        return Err("source_byte_budget_exceeded");
    }
    let source_hash = sha256(source_bytes);
    if enforce_frozen_source {
        validate_frozen_source_identity(source_bytes.len(), &source_hash)?;
    }
    let source = std::str::from_utf8(source_bytes).map_err(|_| "source_utf8_invalid")?;
    let (plans, nonempty_runtime_profiles) = extract_corpus(source)?;
    if enforce_frozen_source && plans.len() != EXPECTED_CAPTURE_COUNT {
        return Err("capture_count_mismatch");
    }
    if enforce_frozen_source && nonempty_runtime_profiles != EXPECTED_NONEMPTY_RUNTIME_PROFILES {
        return Err("runtime_profile_coverage_mismatch");
    }

    let mut state_counts = BTreeMap::from([
        ("complete", 0_usize),
        ("partial", 0_usize),
        ("unavailable", 0_usize),
    ]);
    let mut limitation_counts = BTreeMap::<String, usize>::new();
    let mut captures = Vec::with_capacity(plans.len());
    let mut totals = [0_usize; 9];
    let mut max_nodes = 0_usize;
    let mut max_fragments = 0_usize;

    for (ordinal, rows) in plans.into_iter().enumerate() {
        let row_count = rows.len();
        let grammar = format!("{}\n", rows.join("\n"));
        let facts = normalize_explain_costs(&grammar, &ExplainLimits::default());
        if facts.fragments.is_empty() || facts.nodes.is_empty() {
            return Err("typed_structure_empty");
        }
        if facts
            .limitations
            .iter()
            .any(|limitation| limitation.code.contains("exceeded"))
        {
            return Err("plan_budget_exceeded");
        }
        let state = state_name(&facts.state);
        *state_counts.get_mut(state).ok_or("state_count_failed")? += 1;
        for limitation in &facts.limitations {
            *limitation_counts
                .entry(limitation.code.clone())
                .or_default() += 1;
        }
        let projection =
            serde_json::to_vec(&safe_projection(&facts)).map_err(|_| "safe_projection_failed")?;
        let relations = facts.plan_children.len() + facts.verified_exchange_links.len();
        let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
            rows,
            // Grammar calibration only. The source file proves this version for
            // one Runtime Profile, not for all 79 plan captures.
            detected_version: "3.5.14-23a56ec".to_owned(),
            default_database: None,
        });
        if !matches!(snapshot.runtime_state, ExplainRuntimeState::Unavailable)
            || snapshot.graph_view.nodes.len() != facts.nodes.len()
            || snapshot.graph_view.edges.len() != relations
            || snapshot.plan_node_fact_sets.len() != facts.nodes.len()
            || snapshot
                .graph_view
                .nodes
                .iter()
                .any(|node| node.source_refs.is_empty())
            || snapshot
                .graph_view
                .edges
                .iter()
                .any(|edge| edge.evidence_refs.is_empty())
        {
            return Err("snapshot_projection_invariant_failed");
        }
        totals[0] += facts.fragments.len();
        totals[1] += facts.nodes.len();
        totals[2] += facts.plan_children.len();
        totals[3] += facts.verified_exchange_links.len();
        totals[4] += facts.plan_node_facts.len();
        totals[5] += facts.limitations.len();
        totals[6] += snapshot.graph_view.nodes.len();
        totals[7] += snapshot.graph_view.edges.len();
        totals[8] += row_count;
        max_nodes = max_nodes.max(facts.nodes.len());
        max_fragments = max_fragments.max(facts.fragments.len());
        captures.push(json!({
            "capture_index": ordinal + 1,
            "explain_row_occurrences": row_count,
            "typed_projection_sha256": sha256(&projection),
            "state": state,
            "fragments": facts.fragments.len(),
            "nodes": facts.nodes.len(),
            "relations": relations,
            "plan_children": facts.plan_children.len(),
            "verified_exchange_links": facts.verified_exchange_links.len(),
            "fact_sets": facts.plan_node_facts.len(),
            "limitation_count": facts.limitations.len(),
            "snapshot_graph_nodes": snapshot.graph_view.nodes.len(),
            "snapshot_graph_edges": snapshot.graph_view.edges.len(),
        }));
    }

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "source": {
            "label": "local-production-capture",
            "sha256": source_hash,
            "bytes": source_bytes.len(),
            "capture_count": captures.len(),
        },
        "execution": {
            "sql_executed": false,
            "source_read_only": true,
            "source_text_persisted": false,
            "source_path_persisted": false,
            "parser": "doctor_core::normalize_explain_costs",
            "parser_input": "in_memory_explain_string_rows",
            "runtime_profile_coverage": {
                "nonempty": nonempty_runtime_profiles,
                "empty": captures.len() - nonempty_runtime_profiles,
            },
            "snapshot_projection": "3.5.14_formatter_calibration_only_not_source_version_evidence",
            "parser_pack": "starrocks_3_5_14_formatter_grammar",
            "source_version_attestation": "one_of_79_runtime_profiles_only",
        },
        "tool": {
            "source_sha256": sha256(TOOL_SOURCE),
            "official_3514_commit": OFFICIAL_3514_COMMIT,
        },
        "state_counts": state_counts,
        "limitation_counts": limitation_counts,
        "totals": {
            "fragments": totals[0],
            "nodes": totals[1],
            "plan_children": totals[2],
            "verified_exchange_links": totals[3],
            "fact_sets": totals[4],
            "limitations": totals[5],
            "snapshot_graph_nodes": totals[6],
            "snapshot_graph_edges": totals[7],
            "explain_row_occurrences": totals[8],
        },
        "maxima": { "fragments": max_fragments, "nodes": max_nodes },
        "captures": captures,
    }))
}

fn run() -> Result<String, &'static str> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(args.next().ok_or("usage_invalid")?);
    let output = PathBuf::from(args.next().ok_or("usage_invalid")?);
    if args.next().is_some() || output.exists() {
        return Err("usage_invalid");
    }
    let source = fs::read(input).map_err(|_| "source_read_failed")?;
    let receipt = inspect_source(&source, true)?;
    let encoded = serde_json::to_vec_pretty(&receipt).map_err(|_| "receipt_encoding_failed")?;
    let payload = [&encoded[..], b"\n"].concat();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|_| "receipt_directory_failed")?;
    }
    fs::write(output, &payload).map_err(|_| "receipt_write_failed")?;
    Ok(sha256(&payload))
}

fn main() -> ExitCode {
    match run() {
        Ok(receipt_hash) => {
            println!("CORPUS_RECEIPT_SHA256={receipt_hash}");
            println!("CAPTURE_COUNT={EXPECTED_CAPTURE_COUNT}");
            ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("{code}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EXPECTED_SOURCE_BYTES, EXPECTED_SOURCE_SHA256, extract_explain_plans, inspect_source,
        validate_frozen_source_identity,
    };

    fn capture(index: usize, node: &str) -> String {
        let rows = serde_json::to_string(&vec![
            serde_json::json!({"Explain String": "PLAN FRAGMENT 0(F0)"}),
            serde_json::json!({"Explain String": "  RESULT SINK"}),
            serde_json::json!({"Explain String": ""}),
            serde_json::json!({"Explain String": format!("  {index}:{node}")}),
            serde_json::json!({"Explain String": "  |  cardinality: 7"}),
        ])
        .expect("synthetic rows serialize");
        format!(
            "<!-- SR_PROFILE_CAPTURE category=synthetic capture_id=redacted-{index} target={index} -->\n### SQL\n```sql\nSELECT secret_{index} FROM private_{index};\n```\n### EXPLAIN COSTS\n```\n{rows}\n```\n### Raw profile\n```\n{}\n```\n",
            if index == 1 { "runtime-fixture" } else { "" }
        )
    }

    #[test]
    fn visits_every_capture_without_publishing_business_text() {
        let source = format!("{}\n{}", capture(1, "Project"), capture(2, "OlapScanNode"));
        let receipt = inspect_source(source.as_bytes(), false).expect("synthetic corpus inspects");
        assert_eq!(receipt["source"]["capture_count"], 2);
        assert_eq!(receipt["captures"].as_array().expect("captures").len(), 2);
        let text = serde_json::to_string(&receipt).expect("receipt serializes");
        assert!(!text.contains("secret_"));
        assert!(!text.contains("private_"));
        assert!(!text.contains("SELECT"));
    }

    #[test]
    fn unknown_node_is_not_silently_removed_before_the_parser() {
        let error = inspect_source(capture(1, "PRIVATE_MAGIC_NODE").as_bytes(), false)
            .expect_err("no registered typed structure must fail");
        assert_eq!(error, "typed_structure_empty");

        let source = capture(1, "Project").replace("cardinality: 7", "unknown fact: secret");
        let receipt = inspect_source(source.as_bytes(), false).expect("known node remains typed");
        assert_eq!(receipt["source"]["capture_count"], 1);
    }

    #[test]
    fn malformed_or_empty_capture_fails_closed() {
        assert_eq!(
            extract_explain_plans("plain markdown").expect_err("marker is required"),
            "capture_count_invalid"
        );
        assert_eq!(
            extract_explain_plans(
                "<!-- SR_PROFILE_CAPTURE category=synthetic capture_id=redacted target=1 -->\n### EXPLAIN COSTS\n```\n[]\n```\n### Raw profile\n```\n\n```\n"
            )
            .expect_err("empty rows fail"),
            "explain_rows_empty"
        );
        let missing_target = capture(1, "Project").replace(" target=1", "");
        assert_eq!(
            extract_explain_plans(&missing_target).expect_err("capture marker shape is exact"),
            "capture_index_invalid"
        );
    }

    #[test]
    fn frozen_identity_requires_exact_bytes_before_hash() {
        assert_eq!(
            validate_frozen_source_identity(EXPECTED_SOURCE_BYTES - 1, EXPECTED_SOURCE_SHA256)
                .expect_err("wrong byte length must fail"),
            "source_size_mismatch"
        );
        assert_eq!(
            validate_frozen_source_identity(EXPECTED_SOURCE_BYTES, "0")
                .expect_err("wrong hash must fail"),
            "source_hash_mismatch"
        );
    }

    #[test]
    fn plan_budget_limit_is_a_hard_failure() {
        let oversized = "X".repeat(300_000);
        let source = capture(1, "Project").replace("cardinality: 7", &oversized);
        assert_eq!(
            inspect_source(source.as_bytes(), false).expect_err("line budget must fail"),
            "plan_budget_exceeded"
        );
    }
}
