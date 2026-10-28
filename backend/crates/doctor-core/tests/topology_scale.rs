//! Scale guards for the runtime topology path (v0.6 audit F9).
//!
//! * A synthetic Level 1 Profile with 3,000 plan nodes and 80,000 operators
//!   must parse, resolve, evaluate and assemble with linear cost: a quarter of
//!   the Profile must take about a quarter of the time. The ratio check is
//!   independent of machine speed; a node x operator or node x edge loop makes
//!   it about 16. A wall-clock bound backs it up (generous, for debug builds).
//!   Before v0.6 the full Profile took 508 s in a debug build.
//! * The FE `Topology` line of a plan with more than about 3,100 nodes exceeds
//!   the 256 KiB per-line limit; that one line is exempt (product decision),
//!   every other line still is not.
//!
//! * Release builds only: the largest operator-heavy and metric-dense
//!   Profiles the default budget admits finish inside the API's 10 s
//!   analysis wall (v0.6-03).
//!
//! The large Profile exceeds the default text-line budget (250,000 lines) on
//! purpose: the test measures algorithmic cost, so it raises only that budget
//! and the metric budget.

#[path = "support/scale_profile.rs"]
mod scale_profile;

use scale_profile::{BULK_PIPELINES, Shape, metric_dense};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use doctor_contracts::{AnalysisSnapshotReport, GraphState, RequestedSourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

/// Linear cost gives 4 for a 4x larger Profile; quadratic gives 16.
const MAX_SCALE_RATIO: f64 = 8.0;
const BOUND: Duration = Duration::from_secs(300);

/// Tests in this file run in parallel; analyses take turns so each timing
/// measures one analysis, as the ratio and wall-clock guards assume.
static ONE_ANALYSIS_AT_A_TIME: Mutex<()> = Mutex::new(());

fn analyze(text: String, limits: &AdapterLimits) -> (AnalysisSnapshotReport, Duration) {
    let _turn = ONE_ANALYSIS_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let started = Instant::now();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, text.into_bytes()),
        limits,
        &context,
    )
    .expect("synthetic Profile adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("synthetic Profile resolves");
    evaluate_rules(&mut output).expect("rules evaluate");
    let report = assemble_public_report(output).expect("report assembles");
    (report, started.elapsed())
}

fn assert_closed_and_attributed(report: &AnalysisSnapshotReport, shape: &Shape) {
    let graph = &report.graph_view;
    assert_eq!(graph.nodes.len(), shape.plan_nodes());
    assert_eq!(
        graph.state,
        GraphState::Complete,
        "{:?}",
        graph.unresolved_relations
    );
    assert!(graph.unconnected_node_ids.is_empty());
    for node in &graph.nodes {
        let id: usize = node.node_id.parse().expect("numeric plan node id");
        // Fragment f owns 3f..=3f+2: its receiving EXCHANGE stays with the
        // consumer, and the last fragment's OLAP_SCAN takes the free id 3f+2.
        let expected = id / 3;
        assert_eq!(
            node.fragment_id.as_deref(),
            Some(expected.to_string().as_str()),
            "fragment of plan node {id}"
        );
    }
}

#[test]
fn three_thousand_nodes_and_eighty_thousand_operators_scale_linearly() {
    let limits = AdapterLimits {
        max_text_lines: 1_000_000,
        max_normalized_metrics: 1_000_000,
        ..AdapterLimits::default()
    };
    let small = Shape {
        fragments: 250,
        bulk_pipelines: BULK_PIPELINES,
    };
    let large = Shape {
        fragments: 1_000,
        bulk_pipelines: BULK_PIPELINES,
    };

    let (text, _, _) = small.profile();
    let (report, small_time) = analyze(text, &limits);
    assert_closed_and_attributed(&report, &small);

    let (text, operators, _) = large.profile();
    assert!(
        large.plan_nodes() >= 3_000 && operators >= 80_000,
        "{} nodes / {operators} operators",
        large.plan_nodes()
    );
    let (report, large_time) = analyze(text, &limits);
    assert_closed_and_attributed(&report, &large);

    let ratio = large_time.as_secs_f64() / small_time.as_secs_f64().max(1e-3);
    eprintln!(
        "topology_scale: 1/4 size {small_time:?}, full size {large_time:?}, ratio {ratio:.2}"
    );
    assert!(
        ratio < MAX_SCALE_RATIO,
        "4x larger Profile took {ratio:.2}x as long ({small_time:?} -> {large_time:?})"
    );
    assert!(
        large_time < BOUND,
        "parse + assemble took {large_time:?} (bound {BOUND:?})"
    );
}

#[test]
fn topology_line_is_exempt_from_the_per_line_limit_and_other_lines_are_not() {
    let limits = AdapterLimits::default();
    let shape = Shape {
        fragments: 1_200,
        bulk_pipelines: 0,
    };
    let (text, _, topology_bytes) = shape.profile();
    assert!(
        topology_bytes > limits.max_text_line_bytes,
        "{topology_bytes} bytes must exceed the per-line limit"
    );
    let (report, _) = analyze(text, &limits);
    assert_closed_and_attributed(&report, &shape);

    let other = format!(
        "Query:\n  Summary:\n     - Query Type: Query\n     - Sql Statement: {}\n",
        "x".repeat(limits.max_text_line_bytes + 1)
    );
    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let error = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, other.into_bytes()),
        &limits,
        &context,
    )
    .expect_err("an ordinary long line is still rejected");
    assert_eq!(error.code, "line_too_long");
}

/// The largest Profiles of both shapes that the default budget (250,000 text
/// lines, 250,000 metrics, 16 MiB; doctor-api `ResourceBudgets::default`)
/// admits must finish inside the API's 10 s `analysis_wall`. Fragment counts
/// are the maximum for each shape: one more fragment exceeds the line budget.
/// Release builds only; debug timings say nothing about the API budget.
#[test]
#[cfg_attr(debug_assertions, ignore = "timing guard for release builds")]
fn default_budget_maximum_profiles_finish_within_the_analysis_wall() {
    const ANALYSIS_WALL: Duration = Duration::from_secs(10);
    let limits = AdapterLimits::default();
    let build = |fragments, dense: bool| {
        let (text, _, _) = Shape {
            fragments,
            bulk_pipelines: BULK_PIPELINES,
        }
        .profile();
        if dense { metric_dense(&text) } else { text }
    };
    for (name, fragments, dense) in [("operator-heavy", 590, false), ("metric-dense", 170, true)] {
        let build = |fragments| build(fragments, dense);
        assert!(
            build(fragments + 1).lines().count() > limits.max_text_lines,
            "{name}: {fragments} fragments is not the maximum"
        );
        let text = build(fragments);
        assert!(text.lines().count() <= limits.max_text_lines);
        let (report, elapsed) = analyze(text, &limits);
        assert_closed_and_attributed(
            &report,
            &Shape {
                fragments,
                bulk_pipelines: BULK_PIPELINES,
            },
        );
        eprintln!("default budget {name}: {fragments} fragments in {elapsed:?}");
        assert!(
            elapsed < ANALYSIS_WALL,
            "{name}: {elapsed:?} exceeds the {ANALYSIS_WALL:?} analysis wall"
        );
    }
}
