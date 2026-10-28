use doctor_contracts::{
    AnalysisSnapshotReport, AnalysisState, BoundedFidelity, BoundedInterval, CanonicalNumber,
    Comparison, ExecutionShape, ExecutionShapeState, Finding, GraphState, LosslessFidelity,
    MetricCell, MetricCellValue, MetricDelta, MetricDeltaState, NumberKind, ObservedMetricFidelity,
    PercentageState, Recommendation, RequestedSourceKind,
};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, analysis_attempt,
    assemble_public_report, compare_attempts, evaluate_rules, facts_hash, failed_attempt,
    report_facts, resolve_version_and_graph, v01_analysis_receipt,
};

fn report(state: AnalysisState) -> AnalysisSnapshotReport {
    let mut report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_minimal.report.json"
    ))
    .unwrap();
    report.analysis_state = state;
    report.graph_view.state = GraphState::Complete;
    report.facts_hash = facts_hash(&report_facts(&report));
    report
}

fn number(value: &str) -> CanonicalNumber {
    CanonicalNumber {
        kind: NumberKind::Integer,
        value: value.into(),
        scale: None,
    }
}

fn metric(value: &str, bounds: Option<BoundedInterval>) -> MetricCell {
    MetricCell {
        metric_cell_ref: "metric:operator-0:QuerySpillBytes".into(),
        metric_id: "QuerySpillBytes".into(),
        subject_ref: "operator-0".into(),
        display_text: value.into(),
        order_rank: Some(1),
        source_refs: vec!["json:/query_spill_bytes".into()],
        reasons: vec![],
        match_key: None,
        value: MetricCellValue::Exact {
            canonical_number: number(value),
            canonical_unit: "byte".into(),
            observation: match bounds {
                Some(bounds) => ObservedMetricFidelity::Bounded {
                    fidelity: BoundedFidelity::Interval,
                    bounds,
                },
                None => ObservedMetricFidelity::Lossless {
                    fidelity: LosslessFidelity::Exact,
                    bounds: (),
                },
            },
        },
    }
}

fn with_metric(mut report: AnalysisSnapshotReport, metric: MetricCell) -> AnalysisSnapshotReport {
    report.operator_rows[0].metric_cells = vec![metric];
    report.facts_hash = facts_hash(&report_facts(&report));
    report
}

#[test]
fn public_projection_and_comparator_keep_typed_cells_without_raw_metric_fields() {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let output = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            br#"{"profile_version":"3.5.14","operator_total_time_ns":"500000"}"#,
        ),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let public = assemble_public_report(output).unwrap();
    let public_json = serde_json::to_string(&public).unwrap();
    assert!(!public_json.contains("raw_text"));
    assert_eq!(public.facts_hash, facts_hash(&report_facts(&public)));

    let baseline = with_metric(report(AnalysisState::Complete), metric("0", None));
    let candidate = with_metric(report(AnalysisState::Complete), metric("5", None));
    let comparison = compare_attempts(analysis_attempt(baseline), analysis_attempt(candidate));
    assert_eq!(
        comparison.state,
        doctor_contracts::ComparisonState::Comparable
    );
    assert_eq!(
        comparison.metric_deltas[0].percentage_state,
        PercentageState::UndefinedBaselineZero
    );
    assert!(comparison.metric_deltas[0].percentage_value.is_none());
}

#[test]
fn comparator_handles_lossy_zero_closure_registry_equivalence_algorithm_and_failed_attempts() {
    let bounds = BoundedInterval {
        lower: number("0"),
        upper: number("5"),
        lower_inclusive: false,
        upper_inclusive: true,
    };
    let baseline = with_metric(report(AnalysisState::Partial), metric("1", Some(bounds)));
    let candidate = with_metric(report(AnalysisState::Partial), metric("8", None));
    let comparison = compare_attempts(analysis_attempt(baseline), analysis_attempt(candidate));
    assert_eq!(
        comparison.state,
        doctor_contracts::ComparisonState::PartiallyComparable
    );
    assert_eq!(
        comparison.metric_deltas[0].percentage_state,
        PercentageState::UndefinedBaselineIntervalContainsZero
    );
    assert!(comparison.reasons.contains(&"bounded_metric_delta".into()));
    assert_eq!(comparison.structure_delta.unresolved, ["partial_inputs"]);

    let mut baseline = with_metric(report(AnalysisState::Complete), metric("1", None));
    let baseline_registry = baseline
        .analysis_receipt
        .components
        .iter_mut()
        .find(|component| component.id == "sr3514-metric-registry")
        .unwrap();
    baseline_registry.version = "0.1.1".into();
    baseline_registry.manifest_hash =
        "850fa6244f1235dbc370b92f8ecf2f53a28789a9214b89f1e41485c7c4098d62".into();
    baseline.facts_hash = facts_hash(&report_facts(&baseline));
    let mut equivalent = with_metric(report(AnalysisState::Complete), metric("2", None));
    let registry = equivalent
        .analysis_receipt
        .components
        .iter_mut()
        .find(|component| component.id == "sr3514-metric-registry")
        .unwrap();
    registry.version = "0.1.0".into();
    registry.manifest_hash =
        "af257364f97f03bca0f0b9b6e4901dd9df871b133e58e623811f0ba210c104a9".into();
    equivalent.facts_hash = facts_hash(&report_facts(&equivalent));
    let comparison = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(equivalent),
    );
    assert_eq!(
        comparison.state,
        doctor_contracts::ComparisonState::Comparable
    );
    assert!(
        comparison
            .compatibility_delta
            .iter()
            .any(|item| item == "registry_equivalence:sr3514.registry.same_metric@1")
    );

    let mut mismatch = baseline.clone();
    mismatch
        .analysis_receipt
        .components
        .iter_mut()
        .find(|component| component.id == "sr3514-metric-registry")
        .unwrap()
        .manifest_hash = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into();
    mismatch.facts_hash = facts_hash(&report_facts(&mismatch));
    let comparison = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(mismatch),
    );
    assert_eq!(
        comparison.state,
        doctor_contracts::ComparisonState::PartiallyComparable
    );
    assert!(
        comparison
            .reasons
            .contains(&"registry_semantic_mismatch".into())
    );
    assert_eq!(
        comparison.metric_deltas[0].state,
        doctor_contracts::MetricDeltaState::NotComparable
    );

    let mut old = baseline.clone();
    let mut new = baseline;
    old.findings = vec![finding("1")];
    new.findings = vec![finding("2")];
    old.facts_hash = facts_hash(&report_facts(&old));
    new.facts_hash = facts_hash(&report_facts(&new));
    let comparison = compare_attempts(analysis_attempt(old), analysis_attempt(new));
    assert_eq!(
        comparison.state,
        doctor_contracts::ComparisonState::PartiallyComparable
    );
    assert_eq!(
        comparison.finding_delta.algorithm_changed,
        ["SR3514.TEST:operator-0:1->2"]
    );

    let mut complete = report(AnalysisState::Complete);
    let mut degraded = report(AnalysisState::Partial);
    complete.findings = vec![finding("1")];
    let mut degraded_finding = finding("1");
    degraded_finding.finding_id = "finding:SR3514.TEST:operator-0:partial".into();
    degraded_finding.evidence_completeness = doctor_contracts::EvidenceCompleteness::Partial;
    degraded_finding.evidence_completeness_reasons = vec!["bounded_typed_inputs".into()];
    degraded.findings = vec![degraded_finding];
    complete.facts_hash = facts_hash(&report_facts(&complete));
    degraded.facts_hash = facts_hash(&report_facts(&degraded));
    let comparison = compare_attempts(analysis_attempt(complete), analysis_attempt(degraded));
    assert_eq!(comparison.finding_delta.changed, ["SR3514.TEST:operator-0"]);
    assert!(comparison.finding_delta.persisting.is_empty());

    let comparison = compare_attempts(
        failed_attempt(
            "parse_failed",
            "parse",
            "no_trusted_facts",
            "change_input",
            "baseline",
        ),
        analysis_attempt(report(AnalysisState::Partial)),
    );
    assert_eq!(
        comparison.state,
        doctor_contracts::ComparisonState::NotComparable
    );
    assert_eq!(comparison.reasons, ["baseline_failed"]);
}

fn finding(version: &str) -> Finding {
    Finding {
        finding_id: "finding:SR3514.TEST:operator-0".into(),
        rule_id: "SR3514.TEST".into(),
        rule_version: version.into(),
        category: "test".into(),
        severity: "moderate".into(),
        confidence: doctor_contracts::FindingConfidence::High,
        confidence_reasons: vec![],
        evidence_completeness: doctor_contracts::EvidenceCompleteness::Complete,
        evidence_completeness_reasons: vec!["exact_typed_inputs".into()],
        subject_ref: Some("operator-0".into()),
        evidence: vec![],
        limitations: vec![],
        recommendation: Recommendation {
            risk: "low".into(),
            expected_direction: "reduce".into(),
            validation_steps: vec![],
            recommendation_id: "rec.test.reduce@v1".into(),
            kind: doctor_contracts::RecommendationKind::Investigation,
            title: "reduce".into(),
            next_step: "reduce".into(),
            risk_level: doctor_contracts::RecommendationRiskLevel::Low,
            result_effect: doctor_contracts::RecommendationResultEffect::Unchanged,
            steps: vec![],
            expected_changes: vec![],
            limitations: vec![],
        },
        related_finding_refs: vec![],
        rule_trace_ref: "rule:SR3514.TEST".into(),
        measurements: vec![],
    }
}

fn retained_complex_report() -> AnalysisSnapshotReport {
    serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/docker_runtime_corpus_complex.report.json"
    ))
    .unwrap()
}

#[test]
fn audit_v1_02_absent_findings_require_a_comparable_rule_evaluation() {
    use doctor_contracts::{EvidenceCompleteness, RuleEvaluation, RuleEvaluationStatus};
    let mut baseline = report(AnalysisState::Complete);
    baseline.findings = vec![finding("1")];
    let mut candidate = baseline.clone();
    candidate.findings.clear();
    candidate.rule_evaluations.clear();
    let compare = |b: AnalysisSnapshotReport, c: AnalysisSnapshotReport| {
        compare_attempts(analysis_attempt(b), analysis_attempt(c))
    };
    assert_eq!(
        compare(baseline.clone(), candidate.clone())
            .finding_delta
            .incomparable,
        ["SR3514.TEST:operator-0"]
    );
    let eval = RuleEvaluation {
        rule_id: "SR3514.TEST".into(),
        rule_version: "1".into(),
        status: RuleEvaluationStatus::NotTriggered,
        evidence_completeness: EvidenceCompleteness::Complete,
        subject_ref: Some("operator-0".into()),
        evidence_completeness_reasons: vec![],
        applicability_checks: vec![],
        input_observation_refs: vec!["metric:test".into()],
        comparisons: vec![],
        threshold_receipts: vec![],
        missing_prerequisites: vec![],
        evidence_refs: vec![],
        limitations: vec![],
        finding_id: None,
    };
    for status in [
        RuleEvaluationStatus::NotApplicable,
        RuleEvaluationStatus::Suppressed,
        RuleEvaluationStatus::Suspected,
    ] {
        candidate.rule_evaluations = vec![RuleEvaluation {
            status,
            ..eval.clone()
        }];
        assert!(
            compare(baseline.clone(), candidate.clone())
                .finding_delta
                .resolved
                .is_empty()
        );
    }
    candidate.rule_evaluations = vec![RuleEvaluation {
        subject_ref: Some("other".into()),
        ..eval.clone()
    }];
    assert!(
        compare(baseline.clone(), candidate.clone())
            .finding_delta
            .resolved
            .is_empty()
    );
    candidate.rule_evaluations = vec![RuleEvaluation {
        rule_version: "2".into(),
        ..eval.clone()
    }];
    assert!(
        compare(baseline.clone(), candidate.clone())
            .finding_delta
            .resolved
            .is_empty()
    );
    candidate.rule_evaluations = vec![RuleEvaluation {
        missing_prerequisites: vec!["input".into()],
        ..eval.clone()
    }];
    assert!(
        compare(baseline.clone(), candidate.clone())
            .finding_delta
            .resolved
            .is_empty()
    );
    candidate.rule_evaluations = vec![eval];
    assert_eq!(
        compare(baseline.clone(), candidate.clone())
            .finding_delta
            .resolved,
        ["SR3514.TEST:operator-0"]
    );
    candidate.rule_evaluations.clear();
    assert_eq!(
        compare(candidate, baseline).finding_delta.incomparable,
        ["SR3514.TEST:operator-0"]
    );
}

#[test]
fn audit_v1_02_final_to_running_profile_is_incomparable_in_both_directions() {
    let analyze = |text: &str| {
        let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
        let mut output = adapt_input(
            InputEnvelope::new(RequestedSourceKind::TextProfile, text.as_bytes()),
            &AdapterLimits::default(),
            &context,
        )
        .unwrap();
        let evidence = output.graph_evidence.clone();
        resolve_version_and_graph(&mut output, evidence).unwrap();
        evaluate_rules(&mut output).unwrap();
        assemble_public_report(output).unwrap()
    };
    let baseline = analyze(include_str!(
        "../../../../fixtures/runtime/3.5.14/profile-states/s04_running_query_final.profile.txt"
    ));
    let candidate = analyze(include_str!(
        "../../../../fixtures/runtime/3.5.14/profile-states/s03_running_snapshot.profile.txt"
    ));
    assert!(!baseline.findings.is_empty());
    let count = baseline.findings.len();
    let comparison = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(candidate.clone()),
    );
    assert!(comparison.finding_delta.resolved.is_empty());
    assert_eq!(comparison.finding_delta.incomparable.len(), count);
    let reverse = compare_attempts(analysis_attempt(candidate), analysis_attempt(baseline));
    assert!(reverse.finding_delta.added.is_empty());
    assert_eq!(reverse.finding_delta.incomparable.len(), count);
}

// Task 21 uses the retained official captures through the public analysis chain.
// It never changes a rule result to manufacture a resolved finding.
fn task21_official_pairs() -> Vec<(String, AnalysisSnapshotReport, AnalysisSnapshotReport)> {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/runtime/3.5.14");
    let mut paths = std::fs::read_dir(root.join("oracle-corpus"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".profile.txt"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_str().unwrap().to_owned();
            let single = analyze(&std::fs::read_to_string(&path).unwrap());
            let multi = analyze(
                &std::fs::read_to_string(root.join("oracle-corpus-multi-be").join(&name)).unwrap(),
            );
            (name, single, multi)
        })
        .collect()
}

fn task21_official_absences(partial: bool) {
    use doctor_contracts::{EvidenceCompleteness, RuleEvaluationStatus};
    let mut checked = 0;
    for (name, single, multi) in task21_official_pairs() {
        for (present, absent) in [(&single, &multi), (&multi, &single)] {
            let forward = compare_attempts(
                analysis_attempt(present.clone()),
                analysis_attempt(absent.clone()),
            );
            let reverse = compare_attempts(
                analysis_attempt(absent.clone()),
                analysis_attempt(present.clone()),
            );
            for finding in &present.findings {
                if absent.findings.iter().any(|other| {
                    other.rule_id == finding.rule_id && other.subject_ref == finding.subject_ref
                }) {
                    continue;
                }
                let matches = absent
                    .rule_evaluations
                    .iter()
                    .filter(|evaluation| {
                        evaluation.rule_id == finding.rule_id
                            && evaluation.subject_ref == finding.subject_ref
                            && evaluation.rule_version == finding.rule_version
                    })
                    .collect::<Vec<_>>();
                let [evaluation] = matches.as_slice() else {
                    continue;
                };
                let wanted = if partial {
                    evaluation.status == RuleEvaluationStatus::NotTriggered
                        && evaluation.evidence_completeness == EvidenceCompleteness::Partial
                        && evaluation.missing_prerequisites.is_empty()
                } else {
                    evaluation.status == RuleEvaluationStatus::Suppressed
                };
                if !wanted {
                    continue;
                }
                let entry = format!(
                    "{}:{}",
                    finding.rule_id,
                    finding.subject_ref.as_deref().unwrap()
                );
                if partial {
                    assert!(
                        forward.finding_delta.resolved.contains(&entry),
                        "{name}: {entry}"
                    );
                    assert!(
                        reverse.finding_delta.added.contains(&entry),
                        "{name}: {entry}"
                    );
                    for comparison in [&forward, &reverse] {
                        let wire = serde_json::to_value(&comparison.finding_delta).unwrap();
                        assert!(
                            wire["partial_evidence"]
                                .as_array()
                                .unwrap()
                                .contains(&serde_json::json!(entry))
                        );
                        assert!(!comparison.finding_delta.incomparable.contains(&entry));
                    }
                } else {
                    assert!(
                        forward.finding_delta.incomparable.contains(&entry),
                        "{name}: {entry}"
                    );
                    assert!(
                        reverse.finding_delta.incomparable.contains(&entry),
                        "{name}: {entry}"
                    );
                    assert!(!forward.finding_delta.resolved.contains(&entry));
                    assert!(!reverse.finding_delta.added.contains(&entry));
                }
                checked += 1;
            }
        }
    }
    assert!(
        checked > 0,
        "the official corpus must exercise this boundary"
    );
    println!("task21 official partial={partial}: {checked} finding cases, both directions");
}

#[test]
fn task21_official_partial_not_triggered_resolves_and_adds_with_explicit_evidence() {
    task21_official_absences(true);
}

#[test]
fn task21_official_suppressed_stays_incomparable_in_both_directions() {
    task21_official_absences(false);
}

fn task21_synthetic_reports() -> (AnalysisSnapshotReport, AnalysisSnapshotReport) {
    let mut baseline = report(AnalysisState::Partial);
    baseline.findings = vec![finding("1")];
    baseline.findings[0].evidence_completeness = doctor_contracts::EvidenceCompleteness::Partial;
    let mut candidate = baseline.clone();
    candidate.findings.clear();
    candidate.rule_evaluations = vec![
        serde_json::from_value(serde_json::json!({
            "rule_id":"SR3514.TEST", "rule_version":"1", "subject_ref":"operator-0",
            "status":"not_triggered", "evidence_completeness":"complete",
            "evidence_completeness_reasons":[], "applicability_checks":[],
            "input_observation_refs":[], "comparisons":[], "threshold_receipts":[],
            "missing_prerequisites":[], "evidence_refs":[], "limitations":[], "finding_id":null
        }))
        .unwrap(),
    ];
    (baseline, candidate)
}

#[test]
fn task21_partial_finding_with_complete_negative_evaluation_is_labelled() {
    use doctor_contracts::EvidenceCompleteness::{Complete, Partial};
    for finding_evidence in [Complete, Partial] {
        for negative_evidence in [Complete, Partial] {
            let (mut present, mut absent) = task21_synthetic_reports();
            present.findings[0].evidence_completeness = finding_evidence.clone();
            absent.rule_evaluations[0].evidence_completeness = negative_evidence.clone();
            let partial = finding_evidence == Partial || negative_evidence == Partial;
            for (before, after, field) in [
                (&present, &absent, "resolved"),
                (&absent, &present, "added"),
            ] {
                let comparison = compare_attempts(
                    analysis_attempt(before.clone()),
                    analysis_attempt(after.clone()),
                );
                let delta = serde_json::to_value(comparison.finding_delta).unwrap();
                assert_eq!(delta[field], serde_json::json!(["SR3514.TEST:operator-0"]));
                assert_eq!(
                    delta["partial_evidence"],
                    if partial {
                        serde_json::json!(["SR3514.TEST:operator-0"])
                    } else {
                        serde_json::json!([])
                    }
                );
            }
        }
    }
}

#[test]
fn task21_partial_evidence_preserves_absence_prerequisite_and_version_gates() {
    use doctor_contracts::{Compatibility, EvidenceCompleteness, RuleEvaluationStatus};
    for case in [
        "suppressed",
        "not_applicable",
        "suspected",
        "unavailable",
        "missing",
        "no_evaluation",
        "ambiguous",
        "rule_version",
        "subject",
        "rule_id",
        "engine",
        "pack",
        "registry",
        "incompatible",
        "finding_unavailable",
    ] {
        let (mut present, mut absent) = task21_synthetic_reports();
        absent.rule_evaluations[0].evidence_completeness = EvidenceCompleteness::Partial;
        match case {
            "suppressed" => absent.rule_evaluations[0].status = RuleEvaluationStatus::Suppressed,
            "not_applicable" => {
                absent.rule_evaluations[0].status = RuleEvaluationStatus::NotApplicable
            }
            "suspected" => absent.rule_evaluations[0].status = RuleEvaluationStatus::Suspected,
            "unavailable" => {
                absent.rule_evaluations[0].evidence_completeness = EvidenceCompleteness::Unavailable
            }
            "missing" => absent.rule_evaluations[0]
                .missing_prerequisites
                .push("input".into()),
            "no_evaluation" => absent.rule_evaluations.clear(),
            "ambiguous" => absent
                .rule_evaluations
                .push(absent.rule_evaluations[0].clone()),
            "rule_version" => absent.rule_evaluations[0].rule_version = "other".into(),
            "subject" => absent.rule_evaluations[0].subject_ref = Some("other".into()),
            "rule_id" => absent.rule_evaluations[0].rule_id = "other".into(),
            "engine" => absent.version.effective_version = Some("3.5.15".into()),
            "pack" => absent.version.selected_pack = None,
            "registry" => {
                absent
                    .analysis_receipt
                    .components
                    .iter_mut()
                    .find(|component| component.id == "sr3514-metric-registry")
                    .unwrap()
                    .manifest_hash = "0".repeat(64)
            }
            "incompatible" => absent.version.compatibility = Compatibility::Incompatible,
            "finding_unavailable" => {
                present.findings[0].evidence_completeness = EvidenceCompleteness::Unavailable
            }
            _ => unreachable!(),
        }
        for (before, after) in [(&present, &absent), (&absent, &present)] {
            let delta = compare_attempts(
                analysis_attempt(before.clone()),
                analysis_attempt(after.clone()),
            )
            .finding_delta;
            assert!(
                delta.resolved.is_empty() && delta.added.is_empty(),
                "{case}"
            );
            assert_eq!(delta.incomparable, ["SR3514.TEST:operator-0"], "{case}");
            assert!(delta.partial_evidence.is_empty(), "{case}");
        }
    }
}

#[test]
fn task21_level2_profiles_do_not_resolve_or_add_findings() {
    let final_report = analyze(include_str!(
        "../../../../fixtures/runtime/3.5.14/profile-states/s04_running_query_final.profile.txt"
    ));
    assert!(!final_report.findings.is_empty());
    for text in [
        include_str!(
            "../../../../fixtures/runtime/3.5.14/profile-states/s01_level2_shuffle_join.profile.txt"
        ),
        include_str!(
            "../../../../fixtures/runtime/3.5.14/profile-states/s02_level2_multi_instance_agg.profile.txt"
        ),
    ] {
        let level2 = analyze(text);
        for (before, after) in [(&final_report, &level2), (&level2, &final_report)] {
            let delta = compare_attempts(
                analysis_attempt(before.clone()),
                analysis_attempt(after.clone()),
            )
            .finding_delta;
            assert!(delta.resolved.is_empty() && delta.added.is_empty());
            assert!(!delta.incomparable.is_empty());
            assert!(delta.partial_evidence.is_empty());
        }
    }
}

#[test]
fn audit_v1_03_edge_semantics_and_node_types_survive_stable_ids() {
    let baseline = retained_complex_report();
    for field in ["from", "to", "input_role", "source_ordinal", "relation"] {
        let mut value = serde_json::to_value(&baseline).unwrap();
        let edge = &mut value["graph_view"]["edges"][0];
        let replacement = match field {
            "from" => serde_json::json!("changed-from"),
            "to" => serde_json::json!("changed-to"),
            "input_role" => serde_json::json!(if edge[field] == "build" {
                "probe"
            } else {
                "build"
            }),
            "source_ordinal" => serde_json::json!(999),
            _ => serde_json::json!(if edge[field] == "plan_child" {
                "fragment_sink"
            } else {
                "plan_child"
            }),
        };
        edge[field] = replacement;
        let candidate: AnalysisSnapshotReport = serde_json::from_value(value).unwrap();
        let comparison = compare_attempts(
            analysis_attempt(baseline.clone()),
            analysis_attempt(candidate),
        );
        assert!(!comparison.structure_delta.edge_delta.is_empty(), "{field}");
    }
    let mut candidate = baseline.clone();
    let id = candidate.graph_view.nodes[0].node_id.clone();
    candidate.graph_view.nodes[0].plan_node_type = Some("CHANGED_OPERATOR".into());
    let comparison = compare_attempts(analysis_attempt(baseline), analysis_attempt(candidate));
    assert!(comparison.structure_delta.changed.contains(&id));
}

#[test]
fn self_comparison_pairs_every_physical_operator_counter_without_ambiguity() {
    let report = retained_complex_report();
    let comparison = compare_attempts(analysis_attempt(report.clone()), analysis_attempt(report));
    assert!(
        !comparison
            .reasons
            .iter()
            .any(|reason| reason == "duplicate_metric_match")
    );
    let unique = comparison
        .reasons
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        comparison.reasons.len(),
        "reasons are deduplicated"
    );
    for delta in &comparison.metric_deltas {
        assert!(
            delta.baseline_metric_cell_ref.is_some() && delta.candidate_metric_cell_ref.is_some(),
            "{} {:?} paired on both sides",
            delta.metric_id,
            delta.subject_match
        );
        if delta.state == doctor_contracts::MetricDeltaState::Comparable {
            assert_eq!(
                delta.baseline_metric_cell_ref,
                delta.candidate_metric_cell_ref
            );
        }
    }
    let cells = |report: &AnalysisSnapshotReport| {
        report
            .operator_rows
            .iter()
            .map(|row| row.metric_cells.len())
            .sum::<usize>()
            + report.derived_metric_cells.len()
    };
    assert_eq!(
        comparison.metric_deltas.len(),
        cells(&retained_complex_report())
    );
}

#[test]
fn colliding_or_missing_match_keys_keep_the_group_ambiguous() {
    let mut report = retained_complex_report();
    let row = report
        .operator_rows
        .iter_mut()
        .find(|row| row.operator_ref == "operator-group:1:26")
        .unwrap();
    let keyed = row
        .metric_cells
        .iter()
        .filter(|cell| cell.metric_id == "OperatorTotalTime")
        .count();
    assert!(
        keyed > 1,
        "fixture keeps several OperatorTotalTime counters per plan node"
    );
    for cell in row
        .metric_cells
        .iter_mut()
        .filter(|cell| cell.metric_id == "OperatorTotalTime")
        .take(1)
    {
        cell.match_key = None;
    }
    let comparison = compare_attempts(
        analysis_attempt(retained_complex_report()),
        analysis_attempt(report),
    );
    let group = comparison
        .metric_deltas
        .iter()
        .filter(|delta| {
            delta.metric_id == "OperatorTotalTime"
                && delta.subject_match.as_deref() == Some("operator-group:1:26")
        })
        .collect::<Vec<_>>();
    assert_eq!(group.len(), 1);
    assert_eq!(
        group[0].limitations,
        vec!["duplicate_metric_match".to_owned()]
    );
    assert!(
        comparison
            .reasons
            .iter()
            .any(|reason| reason == "duplicate_metric_match")
    );
}

#[test]
fn driver_repeats_drop_their_match_keys_instead_of_pairing_by_order() {
    let report: AnalysisSnapshotReport = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/p04/runtime-driver-occurrences.report.json"
    ))
    .unwrap();
    let cells = report
        .operator_rows
        .iter()
        .flat_map(|row| &row.metric_cells)
        .collect::<Vec<_>>();
    let keys = cells
        .iter()
        .filter_map(|cell| cell.match_key.as_deref())
        .collect::<Vec<_>>();
    assert!(
        keys.len() < cells.len(),
        "repeated driver counters lose their key"
    );
    let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), keys.len());
}

// --- Parallelism gate (P1-3) -------------------------------------------------
// Real 3.5.14 Profiles from the pinned image (oracle corpus). c03 has five
// fragments, each with `InstanceNum: 1`; c01 has two.

const C01: &str =
    include_str!("../../../../fixtures/runtime/3.5.14/oracle-corpus/c01_scan_filter.profile.txt");
const C03: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/oracle-corpus/c03_shuffle_join_agg.profile.txt"
);
const C03_FIRST_INSTANCE_NUM: &str = "       - InstanceNum: 1\n";
const CPU_LIMITATION: &str = "query_cpu_summed_over_instance_reports_not_wall_or_operator_time";
const SPILL_LIMITATION: &str = "query_spill_summed_over_instance_reports";

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

fn c03_with_first_instance_num(line: &str) -> String {
    let text = C03.replace("\r\n", "\n");
    assert!(text.contains(C03_FIRST_INSTANCE_NUM));
    text.replacen(C03_FIRST_INSTANCE_NUM, line, 1)
}

fn query_cell<'a>(report: &'a AnalysisSnapshotReport, metric_id: &str) -> &'a MetricCell {
    let cells = report
        .derived_metric_cells
        .iter()
        .filter(|cell| cell.metric_id == metric_id && cell.subject_ref == "query")
        .collect::<Vec<_>>();
    assert_eq!(cells.len(), 1, "{metric_id} query cell");
    cells[0]
}

fn query_delta<'a>(comparison: &'a Comparison, metric_id: &str) -> &'a MetricDelta {
    comparison
        .metric_deltas
        .iter()
        .find(|delta| {
            delta.metric_id == metric_id && delta.subject_match.as_deref() == Some("query")
        })
        .unwrap_or_else(|| panic!("{metric_id} query delta"))
}

fn assert_gated(delta: &MetricDelta, limitation: &str) {
    assert_eq!(
        delta.state,
        MetricDeltaState::NotComparable,
        "{}",
        delta.metric_id
    );
    assert_eq!(delta.limitations, [limitation]);
    assert!(delta.absolute_delta.is_none() && delta.bounds.is_none());
    assert!(delta.percentage_value.is_none() && delta.percentage_bounds.is_none());
    assert!(delta.baseline_metric_cell_ref.is_some() && delta.candidate_metric_cell_ref.is_some());
}

#[test]
fn real_profile_publishes_fragment_instance_counts_and_the_cpu_sum_limitation() {
    let report = analyze(C03);
    let shape = &report.execution_shape;
    assert_eq!(shape.state, ExecutionShapeState::Complete);
    assert_eq!(
        shape.recipe_id,
        "sr3514.query_runtime_profile.instance_num.v1"
    );
    assert_eq!(shape.fragment_count, Some(5));
    assert_eq!(shape.instance_count, Some(5));
    assert_eq!(
        shape
            .fragments
            .iter()
            .map(|fragment| (fragment.fragment_id.as_str(), fragment.instance_num))
            .collect::<Vec<_>>(),
        [
            ("0", Some(1)),
            ("1", Some(1)),
            ("2", Some(1)),
            ("3", Some(1)),
            ("4", Some(1))
        ]
    );
    assert!(
        shape
            .fragments
            .iter()
            .all(|fragment| fragment.source_refs.len() == 2)
    );

    let cpu = query_cell(&report, "QueryCumulativeCpuTime");
    assert_eq!(cpu.reasons, [CPU_LIMITATION]);
    assert_eq!(cpu.state(), doctor_contracts::MetricCellState::Exact);
    assert_eq!(
        query_cell(&report, "QuerySpillBytes").reasons,
        [SPILL_LIMITATION]
    );
    for metric_id in [
        "QueryExecutionWallTime",
        "QueryCumulativeOperatorTime",
        "QueryPeakMemoryUsagePerNode",
    ] {
        assert!(
            query_cell(&report, metric_id).reasons.is_empty(),
            "{metric_id}"
        );
    }
}

#[test]
fn different_instance_or_fragment_counts_make_instance_summed_counters_not_comparable() {
    let baseline = analyze(C03);
    let more_instances = analyze(&c03_with_first_instance_num("       - InstanceNum: 2\n"));
    assert_eq!(more_instances.execution_shape.fragment_count, Some(5));
    assert_eq!(more_instances.execution_shape.instance_count, Some(6));
    let comparison = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(more_instances),
    );
    let cpu = query_delta(&comparison, "QueryCumulativeCpuTime");
    assert_gated(cpu, "parallelism_shape_mismatch");
    assert_eq!(cpu.percentage_state, PercentageState::SemanticMismatch);
    assert_gated(
        query_delta(&comparison, "QuerySpillBytes"),
        "parallelism_shape_mismatch",
    );
    assert!(
        comparison
            .reasons
            .contains(&"parallelism_shape_mismatch".into())
    );
    // Max-based query counters are not instance sums and still compare.
    for metric_id in ["QueryExecutionWallTime", "QueryCumulativeOperatorTime"] {
        assert_eq!(
            query_delta(&comparison, metric_id).state,
            MetricDeltaState::Comparable
        );
    }

    // Two fragments against five: fragment count alone also gates.
    let fewer_fragments = analyze(C01);
    assert_eq!(fewer_fragments.execution_shape.fragment_count, Some(2));
    let comparison = compare_attempts(
        analysis_attempt(fewer_fragments),
        analysis_attempt(baseline),
    );
    assert_gated(
        query_delta(&comparison, "QueryCumulativeCpuTime"),
        "parallelism_shape_mismatch",
    );
}

#[test]
fn equal_fragment_and_instance_counts_compare_instance_summed_counters_as_usual() {
    let baseline = analyze(C03);
    let text = C03.replace("\r\n", "\n");
    let cpu_line = "     - QueryCumulativeCpuTime: 49.594ms\n";
    assert!(text.contains(cpu_line));
    let candidate =
        analyze(&text.replacen(cpu_line, "     - QueryCumulativeCpuTime: 60.000ms\n", 1));
    let comparison = compare_attempts(analysis_attempt(baseline), analysis_attempt(candidate));
    assert!(
        !comparison
            .reasons
            .iter()
            .any(|reason| reason.starts_with("parallelism_shape"))
    );
    let cpu = query_delta(&comparison, "QueryCumulativeCpuTime");
    assert_eq!(cpu.state, MetricDeltaState::Comparable);
    // Printed to the microsecond: the change is the truncation interval.
    let bounds = cpu
        .bounds
        .as_ref()
        .expect("truncated display keeps an interval");
    assert_eq!(bounds.lower.value, "10405000");
    assert_eq!(bounds.upper.value, "10407000");
    assert_eq!(cpu.percentage_state, PercentageState::Approximate);
    assert_eq!(cpu.limitations, ["bounded_metric_delta"]);
    let spill = query_delta(&comparison, "QuerySpillBytes");
    assert_eq!(spill.state, MetricDeltaState::Comparable);
}

#[test]
fn a_missing_instance_count_never_defaults_to_comparable() {
    let baseline = analyze(C03);
    let missing = analyze(&c03_with_first_instance_num(""));
    let shape = &missing.execution_shape;
    assert_eq!(shape.state, ExecutionShapeState::Partial);
    assert_eq!(shape.fragment_count, Some(5));
    assert_eq!(shape.instance_count, None);
    assert_eq!(shape.fragments[0].instance_num, None);
    assert_eq!(
        shape.fragments[0].reasons,
        ["fragment_instance_num_missing"]
    );
    let comparison = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(missing),
    );
    let cpu = query_delta(&comparison, "QueryCumulativeCpuTime");
    assert_gated(cpu, "parallelism_shape_unavailable");
    assert_eq!(cpu.percentage_state, PercentageState::Missing);
    assert!(
        comparison
            .reasons
            .contains(&"parallelism_shape_unavailable".into())
    );

    // A retained report assembled before the shape existed is unavailable too.
    let mut retained = baseline.clone();
    retained.execution_shape = ExecutionShape::default();
    assert_eq!(
        retained.execution_shape.state,
        ExecutionShapeState::Unavailable
    );
    let comparison = compare_attempts(analysis_attempt(retained), analysis_attempt(baseline));
    assert_gated(
        query_delta(&comparison, "QueryCumulativeCpuTime"),
        "parallelism_shape_unavailable",
    );
}

// --- Finding measurement changes (v0.6-07) -----------------------------------
// c17 (broadcast 576.647 MB) is the baseline of c21 (the same join, 384.519 MB).

const C17: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/oracle-corpus-multi-be/c17_large_broadcast.profile.txt"
);
const C21: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/oracle-corpus-multi-be/c21_mbe_broadcast_default_passthrough.profile.txt"
);

#[test]
fn finding_changes_carry_both_sides_rule_measurements_without_inventing_values() {
    let baseline = analyze(C17);
    let candidate = analyze(C21);
    let comparison = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(candidate.clone()),
    );
    let delta = &comparison.finding_delta;
    let broadcast = delta
        .measurement_changes
        .iter()
        .find(|change| {
            change
                .finding_entry
                .starts_with("SR3514.BROADCAST_EXCHANGE_BYTES:")
                && change.measurement_id == "broadcast_bytes_sent"
        })
        .expect("broadcast bytes paired");
    assert!(delta.changed.contains(&broadcast.finding_entry));
    assert_eq!(
        broadcast.baseline_display_text.as_deref(),
        Some("576.647 MB")
    );
    assert_eq!(
        broadcast.candidate_display_text.as_deref(),
        Some("384.519 MB")
    );
    assert_eq!(
        broadcast.baseline_threshold_text.as_deref(),
        Some("> 100 MB")
    );
    assert_eq!(
        broadcast.candidate_threshold_text.as_deref(),
        Some("> 100 MB")
    );
    // EXCHANGE #2 network time share fires on a lower tier before than after.
    let tiers = delta.measurement_changes.iter().find(|change| {
        change
            .finding_entry
            .starts_with("SR3514.EXCHANGE_NETWORK_TIME_SHARE:")
            && change.baseline_threshold_text != change.candidate_threshold_text
            && change.baseline_threshold_text.is_some()
            && change.candidate_threshold_text.is_some()
    });
    assert!(
        tiers.is_some(),
        "a tier change keeps both conditions: {:#?}",
        delta.measurement_changes
    );

    let entry_of = |finding: &Finding| {
        format!(
            "{}:{}",
            finding.rule_id,
            finding.subject_ref.as_deref().unwrap_or_default()
        )
    };
    let side = |report: &AnalysisSnapshotReport, entry: &str, id: &str| {
        report
            .findings
            .iter()
            .find(|finding| entry_of(finding) == entry)
            .and_then(|finding| {
                finding
                    .measurements
                    .iter()
                    .find(|measurement| measurement.measurement_id == id)
            })
            .map(|measurement| measurement.display_text.clone())
    };
    let named = [
        &delta.added,
        &delta.resolved,
        &delta.persisting,
        &delta.changed,
        &delta.incomparable,
    ];
    for change in &delta.measurement_changes {
        assert!(
            named
                .iter()
                .any(|entries| entries.contains(&change.finding_entry)),
            "{} is a named finding entry",
            change.finding_entry
        );
        // Each side is copied from that side's finding, or absent; never a default.
        assert_eq!(
            change.baseline_display_text,
            side(&baseline, &change.finding_entry, &change.measurement_id)
        );
        assert_eq!(
            change.candidate_display_text,
            side(&candidate, &change.finding_entry, &change.measurement_id)
        );
    }
    // A resolved finding has no candidate value at all.
    for entry in &delta.resolved {
        assert!(
            delta
                .measurement_changes
                .iter()
                .filter(|change| &change.finding_entry == entry)
                .all(|change| change.candidate_display_text.is_none())
        );
    }
    // Every published measurement of every paired finding is listed once.
    let listed = delta
        .measurement_changes
        .iter()
        .map(|change| (change.finding_entry.clone(), change.measurement_id.clone()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(listed.len(), delta.measurement_changes.len());
    for report in [&baseline, &candidate] {
        for finding in &report.findings {
            for measurement in &finding.measurements {
                assert!(listed.contains(&(entry_of(finding), measurement.measurement_id.clone())));
            }
        }
    }

    // Self-comparison: every finding persists with identical text on both sides.
    let same = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(baseline),
    );
    assert!(same.finding_delta.changed.is_empty());
    assert!(!same.finding_delta.measurement_changes.is_empty());
    assert!(
        same.finding_delta
            .measurement_changes
            .iter()
            .all(|change| change.baseline_display_text.is_some()
                && change.baseline_display_text == change.candidate_display_text)
    );
}

// Comparator 0.4.0: a stable-id node whose printed plan facts differ is
// "changed"; reordering the same facts is not a change.
#[test]
fn structure_delta_names_matched_nodes_whose_plan_facts_differ() {
    let baseline = retained_complex_report();
    let identical = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(baseline.clone()),
    );
    assert!(identical.structure_delta.changed.is_empty());

    let target = baseline.graph_view.nodes[0].node_id.clone();
    let predicate = doctor_contracts::GraphSqlFact {
        kind: doctor_contracts::GraphSqlFactKind::Predicate,
        value: "order_date >= '2026-10-01'".into(),
        source_refs: vec![],
    };
    let mut candidate = baseline.clone();
    let node = &mut candidate.graph_view.nodes[0];
    let mut facts = node.sql_facts.clone().unwrap_or_default();
    facts.push(predicate);
    node.sql_facts = Some(facts.clone());
    let changed = compare_attempts(
        analysis_attempt(baseline.clone()),
        analysis_attempt(candidate),
    );
    assert_eq!(changed.structure_delta.changed, vec![target]);

    let mut reordered = baseline.clone();
    if let Some(list) = reordered.graph_view.nodes[0].sql_facts.as_mut() {
        list.reverse();
    }
    let same = compare_attempts(analysis_attempt(baseline), analysis_attempt(reordered));
    assert!(same.structure_delta.changed.is_empty());
}
