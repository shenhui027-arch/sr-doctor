use doctor_contracts::{
    ExplainLevel, ExplainMode, ExplainRuntimeState, ExplainSourceDisplayState, ExplainSourceKind,
    ExplainSourceLocator, ExplainSourceTruncationState, GraphState, PackMatch, TopologySource,
};
use doctor_core::{
    ExplainSnapshotInput, assemble_explain_snapshot, assemble_explain_snapshot_for_mode,
};

fn input(rows: Vec<String>) -> ExplainSnapshotInput {
    ExplainSnapshotInput {
        rows,
        detected_version: "3.5.14-23a56ec".to_owned(),
        default_database: Some("private_default_database".to_owned()),
    }
}

fn rows_with_embedded_lines() -> Vec<String> {
    vec![
        "PLAN FRAGMENT 0(F0)\r\n  0:Project".to_owned(),
        "  | raw attribute: preserve-this-text".to_owned(),
        String::new(),
    ]
}

#[test]
fn same_rows_bind_distinct_modes_and_all_modes_expose_bounded_raw_lines() {
    let rows = rows_with_embedded_lines();
    let logical = assemble_explain_snapshot_for_mode(input(rows.clone()), ExplainMode::Logical);
    let verbose = assemble_explain_snapshot_for_mode(input(rows.clone()), ExplainMode::Verbose);
    let costs = assemble_explain_snapshot(input(rows));

    assert_eq!(logical.plan_hash, verbose.plan_hash);
    assert_eq!(logical.plan_hash, costs.plan_hash);
    assert_ne!(logical.snapshot_id, verbose.snapshot_id);
    assert_ne!(logical.snapshot_id, costs.snapshot_id);
    assert_ne!(verbose.snapshot_id, costs.snapshot_id);

    assert_eq!(logical.explain_level, ExplainLevel::Logical);
    assert_eq!(
        logical.source_view.source_kind,
        ExplainSourceKind::ExplainLogical
    );
    assert_eq!(
        logical.graph_view.topology_source,
        TopologySource::ExplainLogical
    );
    assert_eq!(verbose.explain_level, ExplainLevel::Verbose);
    assert_eq!(
        verbose.source_view.source_kind,
        ExplainSourceKind::ExplainVerbose
    );
    assert_eq!(
        verbose.graph_view.topology_source,
        TopologySource::ExplainVerbose
    );
    assert_eq!(costs.explain_level, ExplainLevel::Costs);
    assert_eq!(
        costs.source_view.source_kind,
        ExplainSourceKind::ExplainCosts
    );
    assert_eq!(
        costs.graph_view.topology_source,
        TopologySource::ExplainCosts
    );
    assert_eq!(
        costs.source_view.display_state,
        ExplainSourceDisplayState::RawText
    );
    assert_eq!(
        costs
            .source_view
            .lines
            .iter()
            .map(|line| (line.line_no, line.source_ref.as_str(), line.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (1, "explain:row:1:line:1", "PLAN FRAGMENT 0(F0)"),
            (2, "explain:row:1:line:2", "  0:Project"),
            (
                3,
                "explain:row:2:line:1",
                "  | raw attribute: preserve-this-text"
            ),
            (4, "explain:row:3:line:1", ""),
        ]
    );
    assert!(
        !costs
            .source_view
            .limitations
            .contains(&"raw_explain_attributes_restricted".to_owned())
    );
    assert!(costs.source_view.lines.iter().all(|line| {
        costs.source_locators.iter().any(|locator| {
            matches!(
                locator,
                ExplainSourceLocator::RowLine {
                    source_ref,
                    global_line_no,
                    ..
                } if source_ref == &line.source_ref && *global_line_no == line.line_no
            )
        })
    }));

    for snapshot in [&logical, &verbose] {
        assert_eq!(snapshot.graph_view.state, GraphState::Unavailable);
        assert!(snapshot.graph_view.nodes.is_empty());
        assert!(snapshot.graph_view.edges.is_empty());
        assert!(snapshot.plan_node_fact_sets.is_empty());
        assert_eq!(snapshot.runtime_state, ExplainRuntimeState::Unavailable);
        assert!(
            snapshot
                .limitations
                .contains(&"structure_parser_unavailable".to_owned())
        );
        assert!(
            snapshot
                .limitations
                .contains(&"costs_recipes_not_applied_for_raw_text_mode".to_owned())
        );
        assert!(
            snapshot
                .source_view
                .limitations
                .contains(&"costs_recipes_not_applied_for_raw_text_mode".to_owned())
        );
        assert_eq!(
            snapshot.source_view.display_state,
            ExplainSourceDisplayState::RawText
        );
        assert_eq!(
            snapshot.source_view.truncation_state,
            ExplainSourceTruncationState::NotTruncated
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
                (2, "explain:row:1:line:2", "  0:Project"),
                (
                    3,
                    "explain:row:2:line:1",
                    "  | raw attribute: preserve-this-text"
                ),
                (4, "explain:row:3:line:1", ""),
            ]
        );
        assert_eq!(
            snapshot
                .source_locators
                .iter()
                .filter_map(|locator| match locator {
                    ExplainSourceLocator::RowLine {
                        source_ref,
                        row_no,
                        row_line_no,
                        global_line_no,
                        byte_start,
                        byte_end,
                    } => Some((
                        source_ref.as_str(),
                        *row_no,
                        *row_line_no,
                        *global_line_no,
                        *byte_start,
                        *byte_end,
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "explain:row:1:line:1",
                    1,
                    1,
                    1,
                    0,
                    "PLAN FRAGMENT 0(F0)".len() as u32,
                ),
                (
                    "explain:row:1:line:2",
                    1,
                    2,
                    2,
                    ("PLAN FRAGMENT 0(F0)\r\n".len()) as u32,
                    ("PLAN FRAGMENT 0(F0)\r\n  0:Project".len()) as u32,
                ),
                (
                    "explain:row:2:line:1",
                    2,
                    1,
                    3,
                    0,
                    "  | raw attribute: preserve-this-text".len() as u32,
                ),
                ("explain:row:3:line:1", 3, 1, 4, 0, 0),
            ]
        );
    }
}

#[test]
fn unregistered_logical_mode_keeps_no_pack_and_no_invented_structure() {
    let snapshot = assemble_explain_snapshot_for_mode(
        ExplainSnapshotInput {
            rows: vec!["- Output => 0:Project".to_owned()],
            detected_version: "unregistered-version-text".to_owned(),
            default_database: None,
        },
        ExplainMode::Logical,
    );

    assert_eq!(snapshot.version.pack_match, PackMatch::NoPack);
    assert!(snapshot.version.selected_pack.is_none());
    assert!(snapshot.receipt.version_pack.is_none());
    assert!(snapshot.version.detected_release.is_none());
    assert_eq!(snapshot.graph_view.state, GraphState::Unavailable);
    assert_eq!(
        snapshot.graph_view.topology_source,
        TopologySource::ExplainLogical
    );
    assert!(snapshot.plan_node_fact_sets.is_empty());
    assert!(
        snapshot
            .limitations
            .contains(&"detected_version_not_registered_for_explain_logical".to_owned())
    );
    assert_eq!(
        snapshot
            .source_view
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["- Output => 0:Project"]
    );
}

#[test]
fn raw_text_modes_stop_before_emitting_text_when_the_input_budget_is_exceeded() {
    let snapshot = assemble_explain_snapshot_for_mode(
        input(vec!["x".repeat(16 * 1024 * 1024 + 1)]),
        ExplainMode::Verbose,
    );

    assert_eq!(snapshot.graph_view.state, GraphState::Unavailable);
    assert_eq!(
        snapshot.graph_view.topology_source,
        TopologySource::ExplainVerbose
    );
    assert!(snapshot.source_view.lines.is_empty());
    assert_eq!(
        snapshot.source_view.truncation_state,
        ExplainSourceTruncationState::InputLimited
    );
    assert!(
        snapshot
            .limitations
            .contains(&"input_bytes_exceeded".to_owned())
    );
    assert!(
        snapshot
            .source_view
            .limitations
            .contains(&"raw_text_unavailable_for_input_budget".to_owned())
    );
    assert!(
        snapshot
            .source_locators
            .iter()
            .all(|locator| !matches!(locator, ExplainSourceLocator::RowLine { .. }))
    );
}

#[test]
fn input_and_raw_text_snapshot_debug_views_redact_original_text_and_metadata() {
    let secret_row = "raw-secret-row";
    let secret_version = "raw-secret-version";
    let secret_database = "raw-secret-database";
    let input = ExplainSnapshotInput {
        rows: vec![secret_row.to_owned()],
        detected_version: secret_version.to_owned(),
        default_database: Some(secret_database.to_owned()),
    };

    let input_debug = format!("{input:?}");
    for secret in [secret_row, secret_version, secret_database] {
        assert!(!input_debug.contains(secret));
    }

    let logical = assemble_explain_snapshot_for_mode(input, ExplainMode::Logical);
    assert!(
        serde_json::to_string(&logical)
            .expect("logical raw text snapshot JSON")
            .contains(secret_row)
    );
    let logical_debug = format!("{logical:?}");
    for secret in [secret_row, secret_version, secret_database] {
        assert!(!logical_debug.contains(secret));
    }

    let costs = assemble_explain_snapshot_for_mode(
        ExplainSnapshotInput {
            rows: vec![secret_row.to_owned()],
            detected_version: "3.5.14-23a56ec".to_owned(),
            default_database: Some(secret_database.to_owned()),
        },
        ExplainMode::Costs,
    );
    assert!(
        serde_json::to_string(&costs)
            .expect("costs raw text snapshot JSON")
            .contains(secret_row)
    );
    let costs_debug = format!("{costs:?}");
    for secret in [secret_row, secret_database] {
        assert!(!costs_debug.contains(secret));
    }
}
