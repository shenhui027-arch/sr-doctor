use std::collections::BTreeSet;

use doctor_contracts::{
    ExplainRuntimeState, ExplainSourceDisplayState, ExplainSourceLocator, GraphState, PackMatch,
};
use doctor_core::{ExplainSnapshotInput, assemble_explain_snapshot};

fn registered(rows: Vec<String>) -> doctor_contracts::ExplainSnapshot {
    assemble_explain_snapshot(ExplainSnapshotInput {
        rows,
        detected_version: "3.5.14-23a56ec".to_owned(),
        default_database: Some("secret_database".to_owned()),
    })
}

fn fixture_rows() -> Vec<String> {
    include_str!("../../../../fixtures/explain/starrocks-3.5.14/select-multijoin.costs.txt")
        .split_terminator('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}

fn closed_refs(snapshot: &doctor_contracts::ExplainSnapshot) -> BTreeSet<String> {
    snapshot
        .source_locators
        .iter()
        .map(|locator| match locator {
            ExplainSourceLocator::Result { source_ref, .. }
            | ExplainSourceLocator::RowLine { source_ref, .. }
            | ExplainSourceLocator::ConnectionMetadata { source_ref, .. } => source_ref.clone(),
        })
        .collect()
}

fn assert_closed(snapshot: &doctor_contracts::ExplainSnapshot) {
    let locators = closed_refs(snapshot);
    let mut refs = snapshot.source_refs.clone();
    refs.extend(snapshot.version.source_refs.clone());
    refs.extend(snapshot.graph_view.source_refs.clone());
    refs.extend(
        snapshot
            .graph_view
            .nodes
            .iter()
            .flat_map(|node| node.source_refs.clone()),
    );
    refs.extend(
        snapshot
            .graph_view
            .edges
            .iter()
            .flat_map(|edge| edge.evidence_refs.clone()),
    );
    refs.extend(
        snapshot
            .graph_view
            .unresolved_relations
            .iter()
            .flat_map(|relation| relation.source_refs.clone()),
    );
    refs.extend(snapshot.plan_node_fact_sets.iter().flat_map(|set| {
        let mut values = set.source_refs.clone();
        values.extend(
            set.card_slots
                .iter()
                .flat_map(|slot| slot.source_refs.clone()),
        );
        values.extend(set.facts.iter().flat_map(|fact| match fact {
            doctor_contracts::ExplainNodeFact::Available { source_refs, .. }
            | doctor_contracts::ExplainNodeFact::Unavailable { source_refs, .. } => {
                source_refs.clone()
            }
        }));
        values
    }));
    refs.extend(
        snapshot
            .source_view
            .lines
            .iter()
            .map(|line| line.source_ref.clone()),
    );
    refs.extend(
        snapshot
            .diagnostics
            .iter()
            .flat_map(|diagnostic| diagnostic.source_refs.clone()),
    );
    assert!(refs.iter().all(|source_ref| locators.contains(source_ref)));
}

#[test]
fn real_3514_costs_fixture_projects_its_19_source_evidenced_nodes() {
    let snapshot = registered(fixture_rows());
    assert_eq!(
        snapshot.explain_schema_version,
        doctor_contracts::ExplainSchemaVersion::V0_4
    );
    assert_eq!(snapshot.version.pack_match, PackMatch::ExactPack);
    assert_eq!(snapshot.graph_view.state, GraphState::Complete);
    assert_eq!(snapshot.graph_view.nodes.len(), 19);
    assert_eq!(snapshot.runtime_state, ExplainRuntimeState::Unavailable);
    assert!(
        snapshot
            .graph_view
            .nodes
            .iter()
            .all(|node| node.metric_observation_refs.is_empty() && node.pipeline_id.is_none())
    );
    assert!(snapshot.receipt.version_pack.is_some());
    assert_eq!(snapshot.plan_node_fact_sets.len(), 19);
    assert_eq!(snapshot.facts_hash.len(), 64);
    assert!(snapshot.receipt.card_facts_recipe.manifest_hash.len() == 64);
    assert_closed(&snapshot);
}

#[test]
fn sanitized_45_node_capture_projects_typed_plan_facts_without_runtime_values() {
    let rows = include_str!(
        "../../../../fixtures/explain/starrocks-3.5.14/capture-047.sanitized.costs.txt"
    )
    .split_terminator('\n')
    .map(str::to_owned)
    .collect();
    let snapshot = registered(rows);
    assert_eq!(snapshot.graph_view.nodes.len(), 45);
    assert_eq!(snapshot.graph_view.edges.len(), 44);
    assert_eq!(snapshot.plan_node_fact_sets.len(), 45);
    assert!(snapshot.plan_node_fact_sets.iter().all(|set| {
        set.facts.iter().all(|fact| {
            matches!(
                fact,
                doctor_contracts::ExplainNodeFact::Available { .. }
                    | doctor_contracts::ExplainNodeFact::Unavailable { .. }
            )
        })
    }));
    assert_eq!(snapshot.runtime_state, ExplainRuntimeState::Unavailable);
    assert!(snapshot.plan_node_fact_sets.iter().all(|set| {
        set.card_slots.iter().all(|slot| {
            slot.expected_fact_kind != Some(doctor_contracts::ExplainFactKind::PlanChildCount)
                || (slot.state == doctor_contracts::ExplainCardFactSlotState::Unavailable
                    && slot.reason_codes == ["plan_child_count_incomplete"])
        })
    }));
    assert_closed(&snapshot);
}

#[test]
fn unknown_version_does_not_apply_a_pack_or_parse_a_graph() {
    let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
        rows: fixture_rows(),
        detected_version: "untrusted-secret-version".to_owned(),
        default_database: None,
    });
    assert_eq!(snapshot.version.pack_match, PackMatch::NoPack);
    assert_eq!(snapshot.graph_view.state, GraphState::Unavailable);
    assert!(snapshot.graph_view.nodes.is_empty());
    assert!(snapshot.version.detected_release.is_none());
    assert!(snapshot.receipt.version_pack.is_none());
    assert_eq!(
        snapshot.source_view.display_state,
        ExplainSourceDisplayState::RawText
    );
    assert_eq!(
        snapshot
            .source_view
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        fixture_rows()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert!(
        !snapshot
            .source_view
            .limitations
            .contains(&"raw_explain_attributes_restricted".to_owned())
    );
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("untrusted-secret-version")
    );
    assert_closed(&snapshot);
}

#[test]
fn original_rows_hash_in_order_and_embedded_newlines_keep_original_locations() {
    let rows = vec![
        "PLAN FRAGMENT 0(F0)\r\n  RESULT SINK\n  0:Project\n|  secret_attribute: should_not_escape"
            .to_owned(),
        "".to_owned(),
    ];
    let snapshot = registered(rows.clone());
    let reversed = registered(rows.into_iter().rev().collect());
    assert_ne!(snapshot.plan_hash, reversed.plan_hash);
    assert!(snapshot.source_locators.iter().any(|locator| matches!(
        locator,
        ExplainSourceLocator::RowLine {
            row_no: 1,
            row_line_no: 2,
            ..
        }
    )));
    assert!(snapshot.source_locators.iter().any(|locator| matches!(
        locator,
        ExplainSourceLocator::RowLine {
            row_no: 2,
            byte_start: 0,
            byte_end: 0,
            ..
        }
    )));
    assert_eq!(
        snapshot.source_view.display_state,
        ExplainSourceDisplayState::RawText
    );
    assert_eq!(
        snapshot
            .source_view
            .lines
            .iter()
            .map(|line| (line.line_no, line.source_ref.as_str(), line.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (1, "explain:row:1:line:1", "PLAN FRAGMENT 0(F0)"),
            (2, "explain:row:1:line:2", "  RESULT SINK"),
            (3, "explain:row:1:line:3", "  0:Project"),
            (
                4,
                "explain:row:1:line:4",
                "|  secret_attribute: should_not_escape"
            ),
            (5, "explain:row:2:line:1", ""),
        ]
    );
    assert_closed(&snapshot);
}

#[test]
fn raw_explain_attributes_are_confined_to_the_raw_source_view() {
    let restricted = [
        "password=OpenSesame!",
        "550e8400-e29b-41d4-a716-446655440000",
        "db.internal.example:9030",
        "C:\\private\\profile.sql",
        "/srv/private/profile.sql",
        "SELECT secret_column FROM private_table",
        "WHERE secret_column = 'predicate-literal-92'",
    ];
    let mut rows = vec![
        "PLAN FRAGMENT 0(F0)".to_owned(),
        "  RESULT SINK".to_owned(),
        "".to_owned(),
        "  0:Project".to_owned(),
    ];
    rows.extend(
        restricted
            .iter()
            .map(|value| format!("  |  unknown attribute: {value}")),
    );
    let snapshot = registered(rows);
    assert_eq!(
        snapshot.source_view.display_state,
        ExplainSourceDisplayState::RawText
    );
    for secret in restricted {
        assert!(
            snapshot
                .source_view
                .lines
                .iter()
                .any(|line| line.text.contains(secret)),
            "raw source view omitted: {secret}"
        );
    }
    assert!(
        !snapshot
            .source_view
            .limitations
            .contains(&"raw_explain_attributes_restricted".to_owned())
    );
    let mut without_raw_source = serde_json::to_value(&snapshot).expect("snapshot JSON");
    without_raw_source["source_view"]["lines"] = serde_json::json!([]);
    let serialized =
        serde_json::to_string(&without_raw_source).expect("snapshot without raw source");
    for secret in restricted {
        assert!(
            !serialized.contains(secret),
            "raw attribute escaped outside source_view: {secret}"
        );
    }
}

#[test]
fn receipt_components_bind_snapshot_identity_without_changing_plan_hash() {
    let first = registered(fixture_rows());
    let second = registered(fixture_rows());
    assert_eq!(first.plan_hash, second.plan_hash);
    assert_eq!(first.snapshot_id, second.snapshot_id);
    assert!(first.receipt.schema.manifest_hash.len() == 64);
    assert!(first.receipt.adapter.manifest_hash.len() == 64);
    assert!(first.receipt.exchange_recipe.manifest_hash.len() == 64);
}

#[test]
fn input_budget_stops_before_joining_or_projecting_row_lines() {
    let snapshot = registered(vec!["x".repeat(16 * 1024 * 1024 + 1)]);
    assert_eq!(snapshot.graph_view.state, GraphState::Unavailable);
    assert_eq!(
        snapshot.source_view.display_state,
        ExplainSourceDisplayState::SafeStructureOnly
    );
    assert!(snapshot.source_view.lines.is_empty());
    assert!(
        snapshot
            .source_view
            .limitations
            .contains(&"safe_structure_unavailable_for_input_budget".to_owned())
    );
    assert!(
        snapshot
            .source_view
            .limitations
            .contains(&"raw_explain_attributes_restricted".to_owned())
    );
    assert!(
        snapshot
            .limitations
            .contains(&"input_bytes_exceeded".to_owned())
    );
    assert!(
        snapshot
            .source_locators
            .iter()
            .all(|locator| !matches!(locator, ExplainSourceLocator::RowLine { .. }))
    );
    assert_closed(&snapshot);
}
