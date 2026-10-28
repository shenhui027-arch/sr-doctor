use std::{collections::BTreeMap, env, fs, path::PathBuf};

use doctor_contracts::ExplainSourceLocator;
use doctor_core::{ExplainSnapshotInput, assemble_explain_snapshot};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CAPTURE: &str = include_str!(
    "../../../../../fixtures/explain/starrocks-3.5.14/capture-047.sanitized.costs.txt"
);

fn main() {
    let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
        rows: CAPTURE
            .split_terminator('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
            .collect(),
        detected_version: "3.5.14-23a56ec".to_owned(),
        default_database: None,
    });

    let lines = snapshot
        .source_view
        .lines
        .iter()
        .map(|line| (line.source_ref.as_str(), line))
        .collect::<BTreeMap<_, _>>();
    let locators = snapshot
        .source_locators
        .iter()
        .filter_map(|locator| match locator {
            ExplainSourceLocator::RowLine {
                source_ref,
                row_no,
                row_line_no,
                global_line_no,
                ..
            } => Some((
                source_ref.as_str(),
                (*row_no, *row_line_no, *global_line_no),
            )),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let plan_source_lines = snapshot
        .graph_view
        .nodes
        .iter()
        .filter_map(|node| {
            let source_ref = node.source_refs.iter().find(|reference| {
                lines.contains_key(reference.as_str()) && locators.contains_key(reference.as_str())
            })?;
            let line = lines[source_ref.as_str()];
            let (row_no, row_line_no, global_line_no) = locators[source_ref.as_str()];
            Some((
                node.node_id.clone(),
                json!({
                    "rowNo": row_no,
                    "rowLineNo": row_line_no,
                    "globalLineNo": global_line_no,
                    "text": line.text,
                }),
            ))
        })
        .collect::<BTreeMap<String, Value>>();

    let source_sha256 = format!("{:x}", Sha256::digest(CAPTURE.as_bytes()));
    let fixture = json!({
        "fixture_metadata": {
            "source_sha256": source_sha256,
            "status": "production_adapter_snapshot",
            "runtime": "absent",
            "version": "3.5.14",
            "production_adapter_verified": true,
            "snapshot_id": snapshot.snapshot_id,
            "facts_hash": snapshot.facts_hash,
        },
        "graph_view": snapshot.graph_view,
        "plan_node_fact_sets": snapshot.plan_node_fact_sets,
        "plan_source_lines": plan_source_lines,
    });
    let rendered = serde_json::to_string_pretty(&fixture).expect("fixture serialization") + "\n";
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .expect("workspace root")
        .to_owned();
    let target = root.join("frontend/src/features/execution/__fixtures__/complex-plan.json");

    if env::args().any(|argument| argument == "--check") {
        let current = fs::read_to_string(&target).expect("checked-in visual fixture");
        assert_eq!(current, rendered, "complex Explain visual fixture drift");
    } else {
        fs::write(&target, rendered).expect("write visual fixture");
    }

    eprintln!(
        "complex Explain fixture: {} nodes / {} edges / {} fact sets; state {:?}",
        snapshot.graph_view.nodes.len(),
        snapshot.graph_view.edges.len(),
        snapshot.plan_node_fact_sets.len(),
        snapshot.graph_view.state
    );
}
