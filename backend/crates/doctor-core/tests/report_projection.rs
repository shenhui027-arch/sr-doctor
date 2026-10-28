use doctor_contracts::{
    BoundedInterval, CanonicalNumber, Fidelity, MetricCell, MetricCellValue, NumberKind,
    ObservedMetricFidelity, RequestedSourceKind, RuleEvaluation, RuleEvaluationStatus,
};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, PublicReportError, adapt_input,
    assemble_public_report, evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};

const LIVE_RUNTIME_PROFILE_TEXT: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/docker_information_schema_columns.profile.txt"
);

fn resolved_output() -> doctor_core::AdapterOutput {
    let context = AdapterContext::new(v01_analysis_receipt()).expect("valid receipt");
    let mut output = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::TextProfile,
            LIVE_RUNTIME_PROFILE_TEXT.as_bytes(),
        ),
        &AdapterLimits::default(),
        &context,
    )
    .expect("runtime fixture adapts");
    let graph_evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, graph_evidence).expect("version and graph resolve");
    evaluate_rules(&mut output).expect("rules evaluate");
    output
}

fn public_metric_cells(report: &doctor_contracts::AnalysisSnapshotReport) -> Vec<&MetricCell> {
    report
        .derived_metric_cells
        .iter()
        .chain(
            report
                .operator_rows
                .iter()
                .flat_map(|row| row.metric_cells.iter()),
        )
        .collect()
}

#[test]
fn adaptive_duration_display_reaches_query_and_operator_cells() {
    let output = resolved_output();
    let report = assemble_public_report(output.clone()).unwrap();
    let cells = public_metric_cells(&report);
    for (metric, canonical, display) in [
        ("QueryExecutionWallTime", "4240000000", "4.24 s"),
        ("OperatorTotalTime", "1237000", "1.237 ms"),
    ] {
        let original = output
            .ir
            .metric_observations
            .iter()
            .find(|m| {
                m.metric_id.as_deref() == Some(metric)
                    && m.canonical_number
                        .as_ref()
                        .is_some_and(|n| n.value == canonical)
            })
            .unwrap();
        let cell = cells
            .iter()
            .find(|cell| {
                cell.metric_id == metric
                    && cell
                        .canonical_number()
                        .is_some_and(|n| n.value == canonical)
            })
            .unwrap();
        assert_eq!(cell.display_text, display);
        assert_ne!(original.display_value, display, "raw IR text is preserved");
        assert_eq!(cell.canonical_number(), original.canonical_number.as_ref());
        assert_eq!(cell.fidelity(), Some(original.fidelity.clone()));
        assert_eq!(cell.bounds(), original.bounds.as_ref());
        assert_eq!(cell.source_refs, original.source_refs);
    }
}

fn without_metric_refs(cells: &[MetricCell]) -> Vec<MetricCell> {
    cells
        .iter()
        .cloned()
        .map(|mut cell| {
            cell.metric_cell_ref.clear();
            // Presentation alone is adaptive; preserve every typed value,
            // bound, state and source field in this IR-to-report comparison.
            if cell.canonical_unit() == Some("ns") {
                cell.display_text.clear();
            }
            cell
        })
        .collect()
}

fn integer(value: &str) -> CanonicalNumber {
    CanonicalNumber {
        kind: NumberKind::Integer,
        value: value.into(),
        scale: None,
    }
}

fn query_metric_indexes(output: &doctor_core::AdapterOutput, metric_id: &str) -> (usize, usize) {
    let observation = output
        .ir
        .metric_observations
        .iter()
        .position(|item| {
            item.subject_ref.as_deref() == Some("query")
                && item.metric_id.as_deref() == Some(metric_id)
        })
        .expect("query observation");
    let cell = output
        .ir
        .derived_metric_cells
        .iter()
        .position(|item| item.subject_ref == "query" && item.metric_id == metric_id)
        .expect("query metric cell");
    (observation, cell)
}

#[test]
fn real_fixture_rule_inputs_resolve_to_one_public_metric_cell() {
    let mut internal = resolved_output();
    let grouped = internal
        .ir
        .operator_groups
        .iter()
        .flat_map(|group| group.metric_observation_refs.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let mut input_observation_refs = internal
        .ir
        .operator_groups
        .iter()
        .filter(|group| {
            !group.operator_group_id.starts_with("fragment:")
                && !group.operator_group_id.starts_with("pipeline:")
        })
        .flat_map(|group| group.metric_observation_refs.iter().cloned())
        .collect::<Vec<_>>();
    input_observation_refs.extend(
        internal
            .ir
            .metric_observations
            .iter()
            .filter(|observation| !grouped.contains(&observation.observation_id))
            .map(|observation| observation.observation_id.clone()),
    );
    assert!(
        !input_observation_refs.is_empty(),
        "fixture must expose public observations"
    );
    internal.ir.rule_evaluations.push(RuleEvaluation {
        rule_id: "BE16.PUBLIC_EVIDENCE_PROJECTION".into(),
        rule_version: "1".into(),
        status: RuleEvaluationStatus::NotApplicable,
        evidence_completeness: doctor_contracts::EvidenceCompleteness::Unavailable,
        evidence_completeness_reasons: vec!["fixture_not_applicable".into()],
        subject_ref: None,
        applicability_checks: vec![],
        input_observation_refs,
        comparisons: vec![],
        threshold_receipts: vec![],
        missing_prerequisites: vec![],
        evidence_refs: vec![],
        limitations: vec![],
        finding_id: None,
    });

    let report = assemble_public_report(internal).expect("public projection");
    let cells = public_metric_cells(&report);
    for evaluation in &report.rule_evaluations {
        for metric_cell_ref in &evaluation.input_observation_refs {
            assert!(
                !metric_cell_ref.starts_with("observation:"),
                "internal observation leaked from {}",
                evaluation.rule_id
            );
            assert_eq!(
                cells
                    .iter()
                    .filter(|cell| cell.metric_cell_ref == *metric_cell_ref)
                    .count(),
                1,
                "{} must resolve exactly once for {}",
                metric_cell_ref,
                evaluation.rule_id
            );
        }
    }
}

#[test]
fn projection_keeps_query_evidence_and_disambiguates_duplicate_operator_cells() {
    let mut output = resolved_output();
    let (spill_observation, spill_cell) = query_metric_indexes(&output, "QuerySpillBytes");
    output.ir.metric_observations[spill_observation].parse_state =
        doctor_contracts::ParseState::Missing;
    output.ir.metric_observations[spill_observation].canonical_number = None;
    output.ir.metric_observations[spill_observation].canonical_unit = None;
    output.ir.metric_observations[spill_observation].bounds = None;
    output.ir.metric_observations[spill_observation].fidelity = Fidelity::Unknown;
    output.ir.metric_observations[spill_observation].display_value = "—".into();
    output.ir.derived_metric_cells[spill_cell].display_text = "—".into();
    output.ir.derived_metric_cells[spill_cell].value = MetricCellValue::Missing {
        canonical_number: (),
        canonical_unit: (),
        fidelity: (),
        bounds: (),
    };

    let (denominator_observation, denominator_cell) =
        query_metric_indexes(&output, "QueryCumulativeOperatorTime");
    let bounds = BoundedInterval {
        lower: integer("4231000000"),
        upper: integer("4231000001"),
        lower_inclusive: true,
        upper_inclusive: true,
    };
    output.ir.metric_observations[denominator_observation].fidelity = Fidelity::Interval;
    output.ir.metric_observations[denominator_observation].bounds = Some(bounds.clone());
    let MetricCellValue::Exact { observation, .. } =
        &mut output.ir.derived_metric_cells[denominator_cell].value
    else {
        panic!("query denominator must start exact");
    };
    *observation = ObservedMetricFidelity::Bounded {
        fidelity: doctor_contracts::BoundedFidelity::Interval,
        bounds,
    };
    evaluate_rules(&mut output).expect("rules re-evaluate after typed evidence changes");

    let (group_index, original_observation_id) = output
        .ir
        .operator_groups
        .iter()
        .enumerate()
        .filter(|(_, group)| {
            !group.operator_group_id.starts_with("fragment:")
                && !group.operator_group_id.starts_with("pipeline:")
        })
        .find_map(|(group_index, group)| {
            group
                .metric_observation_refs
                .iter()
                .find_map(|observation_id| {
                    output
                        .ir
                        .metric_observations
                        .iter()
                        .find(|item| {
                            item.observation_id == *observation_id
                                && item.metric_id.as_deref() == Some("OperatorTotalTime")
                        })
                        .map(|_| (group_index, observation_id.clone()))
                })
        })
        .expect("operator total-time observation");
    let mut duplicate = output
        .ir
        .metric_observations
        .iter()
        .find(|item| item.observation_id == original_observation_id)
        .cloned()
        .expect("source observation");
    duplicate.observation_id = "observation:duplicate-subordinate-ordinal-0".into();
    duplicate.operator_ref = Some("subordinate:duplicate".into());
    let duplicate_ordinal = output.ir.operator_groups[group_index]
        .subordinate_operator_refs
        .iter()
        .map(|(_, ordinal)| *ordinal)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    duplicate.source_ordinal = Some(duplicate_ordinal);
    let duplicate_id = duplicate.observation_id.clone();
    output.ir.metric_observations.push(duplicate);
    output.ir.operator_groups[group_index]
        .subordinate_operator_refs
        .push(("subordinate:duplicate".into(), duplicate_ordinal));
    output.ir.operator_groups[group_index]
        .metric_observation_refs
        .push(duplicate_id);

    let internal = output.ir.clone();
    // Projection only appends the registry's instance-report-sum caveat
    // (QueryCumulativeCpuTime / QuerySpillBytes); every typed value stays.
    let mut expected_derived = without_metric_refs(&internal.derived_metric_cells);
    for cell in &mut expected_derived {
        if let Some(code) =
            doctor_core::sr3514_instance_report_sum_limitation(&cell.metric_id, &cell.subject_ref)
        {
            cell.reasons.push(code.into());
        }
    }
    assert_eq!(
        expected_derived
            .iter()
            .filter(|cell| !cell.reasons.is_empty())
            .map(|cell| cell.metric_id.as_str())
            .collect::<Vec<_>>(),
        ["QueryCumulativeCpuTime", "QuerySpillBytes"]
    );
    let first = assemble_public_report(output.clone()).expect("first public projection");
    let second = assemble_public_report(output.clone()).expect("second public projection");
    assert_eq!(first, second, "public projection must be deterministic");
    assert_eq!(
        output.ir, internal,
        "projection must not mutate internal IR"
    );
    assert_eq!(
        without_metric_refs(&first.derived_metric_cells),
        expected_derived,
        "query denominator/spill missing/interval evidence and provenance stay intact"
    );

    let duplicate_cells = first
        .operator_rows
        .iter()
        .find(|row| row.operator_ref == internal.operator_groups[group_index].operator_group_id)
        .expect("projected operator row")
        .metric_cells
        .iter()
        .filter(|cell| {
            cell.metric_id == "OperatorTotalTime"
                && cell.subject_ref == internal.operator_groups[group_index].operator_group_id
        })
        .collect::<Vec<_>>();
    assert!(
        duplicate_cells.len() >= 2,
        "duplicate cells are both projected"
    );
    let refs = duplicate_cells
        .iter()
        .map(|cell| cell.metric_cell_ref.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        refs.len(),
        duplicate_cells.len(),
        "duplicate cells cannot crosswire"
    );
    let base = format!(
        "metric:{}:OperatorTotalTime:",
        internal.operator_groups[group_index].operator_group_id
    );
    assert!(
        refs.iter().all(|reference| reference.starts_with(&base)),
        "colliding metric bases receive opaque suffixes"
    );
}

#[test]
fn projection_rejects_an_uncontainerized_rule_input() {
    let mut output = resolved_output();
    let (observation, cell) = query_metric_indexes(&output, "QuerySpillBytes");
    let observation_id = output.ir.metric_observations[observation]
        .observation_id
        .clone();
    output.ir.derived_metric_cells.remove(cell);

    assert_eq!(
        assemble_public_report(output),
        Err(PublicReportError::MissingPublicMetricContainer { observation_id })
    );
}
