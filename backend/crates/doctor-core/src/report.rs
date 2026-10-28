use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use doctor_contracts::{
    AnalysisReceipt, AnalysisSnapshotReport, BoundedFidelity, CanonicalNumber, LosslessFidelity,
    MetricCell, MetricCellValue, NumberKind, ObservedMetricFidelity, OperatorRow, ReceiptComponent,
    RuleEvaluation, canonical_hash,
};

use crate::{
    AdapterOutput, CancelSignal, Cancelled, MetricObservation, NormalizedIr, OperatorGroup,
    ParseState, drop_polled,
};
use starrocks_version_pack_3_5_14::{
    PRODUCT_PACK_VERSION as SR3514_PRODUCT_PACK_VERSION,
    SEMANTIC_MANIFEST_HASH as SR3514_SEMANTIC_MANIFEST_HASH,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicReportError {
    ExactMetricMissingValue,
    MissingMetricObservation { observation_id: String },
    DuplicateMetricObservation { observation_id: String },
    MissingDerivedMetricObservation { metric_cell_ref: String },
    AmbiguousDerivedMetricObservation { metric_cell_ref: String },
    MissingPublicMetricContainer { observation_id: String },
    AmbiguousPublicMetricContainer { observation_id: String },
    MissingPublicMetricReference { metric_cell_ref: String },
    AmbiguousPublicMetricReference { metric_cell_ref: String },
    Cancelled,
}

/// `Cancelled`: the caller's `CancelSignal` stopped the assembly; no report exists.
impl From<Cancelled> for PublicReportError {
    fn from(_: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// The locally assembled receipt is fixed by the checked-in component manifest.
/// The version-pack component deliberately carries its semantic manifest hash.
pub(crate) const GRAPH_COMPONENT_ID: &str = "sr3514-graph-resolver-contract";
pub(crate) const GRAPH_COMPONENT_VERSION: &str = "0.10.0";
pub(crate) const GRAPH_COMPONENT_HASH: &str =
    "adc61d94eab987cf24bd3c3af93eac7e0229ebe389b773af4aba3ab224ac77d5";

pub fn v01_analysis_receipt() -> AnalysisReceipt {
    AnalysisReceipt {
        components: vec![
            receipt(
                "sr3514-runtime-profile-adapter",
                "0.2.3",
                "18681e9af2ba4f02ef2b8437cf11df86cdb778f7aa30cdabd63bfa7067e22eeb",
            ),
            receipt(
                "strict-metric-parser-contract",
                "0.1.0",
                "a05e1deca8a50f4ebaa421c4b1940f1d68815048780ddfe0cf291d3d42d0924a",
            ),
            receipt(
                "sr3514-metric-registry",
                "0.3.2",
                "83eff82b7a6e85a71727ed5d050438766a6fd3c41a44b1244aca6d57d997b55c",
            ),
            receipt(
                GRAPH_COMPONENT_ID,
                GRAPH_COMPONENT_VERSION,
                GRAPH_COMPONENT_HASH,
            ),
            receipt(
                "starrocks-3.5.14",
                SR3514_PRODUCT_PACK_VERSION,
                SR3514_SEMANTIC_MANIFEST_HASH,
            ),
            receipt(
                doctor_rules_v0_1::RULESET_ID,
                doctor_rules_v0_1::PRODUCT_RULESET_VERSION,
                doctor_rules_v0_1::SEMANTIC_MANIFEST_HASH,
            ),
        ],
    }
}

/// Projects only safe, typed IR fields. Raw lexemes and source ranges stay internal.
pub fn assemble_public_report(
    output: AdapterOutput,
) -> Result<AnalysisSnapshotReport, PublicReportError> {
    assemble_public_report_cancellable(output, &CancelSignal::never())
}

/// `assemble_public_report` that polls `cancel` per observation, metric cell
/// and operator row, and returns `PublicReportError::Cancelled` (never a
/// partial report) once it is raised.
pub fn assemble_public_report_cancellable(
    output: AdapterOutput,
    cancel: &CancelSignal<'_>,
) -> Result<AnalysisSnapshotReport, PublicReportError> {
    cancel.check()?;
    let observations = observation_index(&output, cancel)?;
    let derived_observation_refs = derived_observation_refs(&output.ir, &observations, cancel)?;
    cancel.check()?;
    // This map is the report-only bridge from internal observation identity to
    // the generated public cell identity. It never alters IR/rule state.
    let public_metric_refs =
        public_metric_refs(&output.ir, &observations, &derived_observation_refs, cancel)?;
    let mut operator_rows = Vec::with_capacity(output.ir.operator_groups.len());
    for group in &output.ir.operator_groups {
        cancel.tick()?;
        // RuntimeProfile fragment and pipeline containers preserve their own
        // scope internally. `OperatorRow` has no subject-kind discriminator,
        // so publishing either container here would misrepresent it as an
        // operator. Actual RuntimeProfile operators use the `operator:` id.
        if !is_public_operator_group(group) {
            continue;
        }
        let mut metric_cells = Vec::with_capacity(group.metric_observation_refs.len());
        for observation_id in &group.metric_observation_refs {
            cancel.tick()?;
            let observation = observation(&observations, observation_id)?;
            let mut cell = metric_cell(observation)?;
            cell.metric_cell_ref = public_metric_ref(&public_metric_refs, observation_id)?;
            metric_cells.push(cell);
        }
        operator_rows.push(OperatorRow {
            operator_ref: group.operator_group_id.clone(),
            plan_node_id: group.plan_node_id.clone(),
            operator_type: group.canonical_type.clone(),
            metric_cells,
            source_refs: group.source_refs.clone(),
        });
    }
    retain_unique_match_keys(&mut operator_rows, cancel)?;
    cancel.check()?;
    let execution_summary = crate::execution_summary::execution_summary(&output, cancel)?;
    cancel.check()?;
    let release = output.ir.version.effective_version.clone();
    let mut derived_metric_cells = output.ir.derived_metric_cells.clone();
    for (cell, observation_id) in derived_metric_cells
        .iter_mut()
        .zip(derived_observation_refs.iter())
    {
        cell.metric_cell_ref = public_metric_ref(&public_metric_refs, observation_id)?;
        adapt_display(cell);
        // Backend-owned caveat; the browser shows it and never infers it.
        if let Some(code) = crate::metrics::instance_report_sum_limitation(
            release.as_deref(),
            &cell.metric_id,
            &cell.subject_ref,
        ) && !cell.reasons.iter().any(|reason| reason == code)
        {
            cell.reasons.push(code.to_owned());
        }
    }
    let execution_shape = crate::execution_shape::execution_shape(&output, cancel)?;
    cancel.check()?;
    let mut source_view = output.source_view;
    source_view.line_refs = source_line_refs(&output.source_refs);
    let mut report = AnalysisSnapshotReport {
        // `output` is consumed here: fields move instead of being cloned.
        schema_version: output.ir.schema_version,
        analysis_state: output.analysis_state,
        analysis_receipt: output.ir.analysis_receipt,
        analysis_key: output.ir.analysis_key,
        facts_hash: String::new(),
        input: output.ir.input,
        version: output.ir.version,
        profile_context: output.ir.profile_context,
        completeness: output.ir.completeness,
        query_summary: output.ir.query_summary,
        operator_rows,
        graph_view: output.ir.graph,
        execution_summary,
        execution_shape,
        derived_metric_cells,
        findings: public_findings(output.ir.findings, &public_metric_refs)?,
        rule_evaluations: public_rule_evaluations(
            &output.ir.rule_evaluations,
            &public_metric_refs,
        )?,
        diagnostics: output.ir.diagnostics,
        provenance_index: output
            .source_refs
            .into_iter()
            .map(|source| source.ref_id)
            .collect(),
        source_view,
    };
    cancel.check()?;
    verify_public_rule_metric_refs(&report, cancel)?;
    let facts = crate::report_facts_cancellable(&report, cancel)?;
    report.facts_hash = crate::facts_hash_cancellable(&facts, cancel)?;
    // The consumed IR is the largest allocation of the analysis.
    drop_polled(facts.metric_cells, cancel)?;
    drop_polled(public_metric_refs, cancel)?;
    drop_polled(output.ir.metric_observations, cancel)?;
    drop_polled(output.ir.operator_groups, cancel)?;
    drop_polled(output.facts.metric_cells, cancel)?;
    Ok(report)
}

/// One line per single-line text reference, from the typed locator; a
/// multi-line span or a JSON pointer has no single line to name.
fn source_line_refs(sources: &[crate::SourceRef]) -> Vec<doctor_contracts::SourceLineRef> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .filter_map(|source| match source.locator {
            crate::SourceLocator::Text {
                line_start,
                line_end,
                ..
            } if line_start == line_end && seen.insert(source.ref_id.as_str()) => {
                Some(doctor_contracts::SourceLineRef {
                    source_ref: source.ref_id.clone(),
                    line_no: line_start,
                })
            }
            _ => None,
        })
        .collect()
}

/// A match key is published only when it names exactly one cell; colliding
/// keys (e.g. PipelineDriver repeats in an unmerged Profile) are all dropped
/// so Compare never pairs by first match.
fn retain_unique_match_keys(
    rows: &mut [OperatorRow],
    cancel: &CancelSignal<'_>,
) -> Result<(), Cancelled> {
    let mut counts = BTreeMap::<String, usize>::new();
    for cell in rows.iter().flat_map(|row| &row.metric_cells) {
        cancel.tick()?;
        if let Some(key) = cell.match_key.clone() {
            *counts.entry(key).or_default() += 1;
        }
    }
    for cell in rows.iter_mut().flat_map(|row| &mut row.metric_cells) {
        cancel.tick()?;
        if cell.match_key.as_ref().is_some_and(|key| counts[key] != 1) {
            cell.match_key = None;
        }
    }
    Ok(())
}

/// Builds every backend-owned comparable occurrence partition. The input
/// `scope` stays the typed metric scope; it is deliberately separate from the
/// occurrence identity selected below.
fn is_public_operator_group(group: &OperatorGroup) -> bool {
    !group.operator_group_id.starts_with("fragment:")
        && !group.operator_group_id.starts_with("pipeline:")
}

fn observation_index<'a>(
    output: &'a AdapterOutput,
    cancel: &CancelSignal<'_>,
) -> Result<BTreeMap<&'a str, &'a MetricObservation>, PublicReportError> {
    let mut observations = BTreeMap::new();
    for observation in &output.ir.metric_observations {
        cancel.tick()?;
        if observations
            .insert(observation.observation_id.as_str(), observation)
            .is_some()
        {
            return Err(PublicReportError::DuplicateMetricObservation {
                observation_id: observation.observation_id.clone(),
            });
        }
    }
    Ok(observations)
}

fn observation<'a>(
    observations: &'a BTreeMap<&'a str, &'a MetricObservation>,
    observation_id: &str,
) -> Result<&'a MetricObservation, PublicReportError> {
    observations.get(observation_id).copied().ok_or_else(|| {
        PublicReportError::MissingMetricObservation {
            observation_id: observation_id.to_owned(),
        }
    })
}

fn derived_observation_refs(
    ir: &NormalizedIr,
    observations: &BTreeMap<&str, &MetricObservation>,
    cancel: &CancelSignal<'_>,
) -> Result<Vec<String>, PublicReportError> {
    let grouped = ir
        .operator_groups
        .iter()
        .flat_map(|group| group.metric_observation_refs.iter())
        .map(|observation_id| cancel.tick().map(|()| observation_id.as_str()))
        .collect::<Result<HashSet<_>, _>>()?;
    let mut candidates = Vec::new();
    for observation in observations.values().copied() {
        cancel.tick()?;
        if !grouped.contains(observation.observation_id.as_str()) {
            candidates.push(observation);
        }
    }
    let mut matched = BTreeSet::new();
    let mut refs = Vec::with_capacity(ir.derived_metric_cells.len());
    for cell in &ir.derived_metric_cells {
        let matches = candidates
            .iter()
            .copied()
            .map(|observation| {
                derived_metric_cell_matches(observation, cell).map(|matches| (observation, matches))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(observation, matches)| matches.then_some(observation))
            .collect::<Vec<_>>();
        let observation = match matches.as_slice() {
            [] => {
                return Err(PublicReportError::MissingDerivedMetricObservation {
                    metric_cell_ref: cell.metric_cell_ref.clone(),
                });
            }
            [observation] => *observation,
            _ => {
                return Err(PublicReportError::AmbiguousDerivedMetricObservation {
                    metric_cell_ref: cell.metric_cell_ref.clone(),
                });
            }
        };
        if !matched.insert(observation.observation_id.clone()) {
            return Err(PublicReportError::AmbiguousPublicMetricContainer {
                observation_id: observation.observation_id.clone(),
            });
        }
        refs.push(observation.observation_id.clone());
    }
    if let Some(observation) = candidates
        .into_iter()
        .find(|observation| !matched.contains(&observation.observation_id))
    {
        return Err(PublicReportError::MissingPublicMetricContainer {
            observation_id: observation.observation_id.clone(),
        });
    }
    Ok(refs)
}

fn derived_metric_cell_matches(
    observation: &MetricObservation,
    cell: &MetricCell,
) -> Result<bool, PublicReportError> {
    // Validate the original IR projection before changing presentation text.
    let mut projected = raw_metric_cell(observation)?;
    projected.metric_cell_ref = cell.metric_cell_ref.clone();
    Ok(projected == *cell)
}

fn public_metric_refs(
    ir: &NormalizedIr,
    observations: &BTreeMap<&str, &MetricObservation>,
    derived_observation_refs: &[String],
    cancel: &CancelSignal<'_>,
) -> Result<HashMap<String, String>, PublicReportError> {
    let observation_ids = ir
        .operator_groups
        .iter()
        .filter(|group| is_public_operator_group(group))
        .flat_map(|group| group.metric_observation_refs.iter())
        .chain(derived_observation_refs)
        .map(String::as_str);

    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for observation_id in observation_ids {
        cancel.tick()?;
        let observation = observation(observations, observation_id)?;
        if !seen.insert(observation_id) {
            return Err(PublicReportError::AmbiguousPublicMetricContainer {
                observation_id: observation_id.to_owned(),
            });
        }
        entries.push((
            observation_id,
            observation,
            metric_cell_ref_base(observation),
        ));
    }
    let mut base_counts = HashMap::<&str, usize>::new();
    for (_, _, base) in &entries {
        *base_counts.entry(base.as_str()).or_default() += 1;
    }

    // Each colliding observation is hashed once; the suffix length depends on
    // the whole group, so refs are formatted after every hash is known.
    let mut group_hashes = HashMap::<&str, Vec<String>>::new();
    let hash_indexes = entries
        .iter()
        .map(|(_, observation, base)| {
            cancel.tick()?;
            Ok((base_counts[base.as_str()] > 1).then(|| {
                let hashes = group_hashes.entry(base.as_str()).or_default();
                hashes.push(canonical_hash(&observation.observation_id));
                hashes.len() - 1
            }))
        })
        .collect::<Result<Vec<_>, Cancelled>>()?;
    let suffix_lens = group_hashes
        .iter()
        .map(|(base, hashes)| (*base, metric_ref_suffix_len(hashes)))
        .collect::<HashMap<_, _>>();
    Ok(entries
        .iter()
        .zip(hash_indexes)
        .map(|((observation_id, _, base), hash_index)| {
            cancel.tick()?;
            let metric_cell_ref = match hash_index {
                None => base.clone(),
                Some(index) => {
                    let hash = &group_hashes[base.as_str()][index];
                    format!("{base}:{}", &hash[..suffix_lens[base.as_str()]])
                }
            };
            Ok(((*observation_id).to_owned(), metric_cell_ref))
        })
        .collect::<Result<_, Cancelled>>()?)
}

const METRIC_REF_SUFFIX_HEX: usize = 12;

/// The suffix only disambiguates observations sharing one subject and metric.
/// A 12-hex prefix of the observation hash is enough; a group whose prefixes
/// collide keeps the full hash, so refs stay unique and deterministic.
fn metric_ref_suffix_len(hashes: &[String]) -> usize {
    let prefixes = hashes
        .iter()
        .map(|hash| &hash[..METRIC_REF_SUFFIX_HEX])
        .collect::<BTreeSet<_>>();
    if prefixes.len() == hashes.len() {
        METRIC_REF_SUFFIX_HEX
    } else {
        hashes.iter().map(String::len).min().unwrap_or(0)
    }
}

fn metric_cell_ref_base(observation: &MetricObservation) -> String {
    format!(
        "metric:{}:{}",
        observation.subject_ref.as_deref().unwrap_or("unknown"),
        observation.metric_id.as_deref().unwrap_or("unknown")
    )
}

fn public_metric_ref(
    public_metric_refs: &HashMap<String, String>,
    observation_id: &str,
) -> Result<String, PublicReportError> {
    public_metric_refs
        .get(observation_id)
        .cloned()
        .ok_or_else(|| PublicReportError::MissingPublicMetricContainer {
            observation_id: observation_id.to_owned(),
        })
}

/// A finding's measurements name their inputs by the same public metric cell
/// refs as its rule evaluation.
fn public_findings(
    mut findings: Vec<doctor_contracts::Finding>,
    public_metric_refs: &HashMap<String, String>,
) -> Result<Vec<doctor_contracts::Finding>, PublicReportError> {
    for measurement in findings
        .iter_mut()
        .flat_map(|finding| finding.measurements.iter_mut())
    {
        measurement.input_refs = measurement
            .input_refs
            .iter()
            .map(|observation_id| public_metric_ref(public_metric_refs, observation_id))
            .collect::<Result<_, _>>()?;
    }
    // Recommendation steps cite the same public cells.
    for step in findings
        .iter_mut()
        .flat_map(|finding| finding.recommendation.steps.iter_mut())
    {
        step.evidence_refs = step
            .evidence_refs
            .iter()
            .map(|observation_id| public_metric_ref(public_metric_refs, observation_id))
            .collect::<Result<_, _>>()?;
    }
    Ok(findings)
}

fn public_rule_evaluations(
    rule_evaluations: &[RuleEvaluation],
    public_metric_refs: &HashMap<String, String>,
) -> Result<Vec<RuleEvaluation>, PublicReportError> {
    rule_evaluations
        .iter()
        .cloned()
        .map(|mut evaluation| {
            evaluation.input_observation_refs = evaluation
                .input_observation_refs
                .iter()
                .map(|observation_id| public_metric_ref(public_metric_refs, observation_id))
                .collect::<Result<_, _>>()?;
            Ok(evaluation)
        })
        .collect()
}

fn verify_public_rule_metric_refs(
    report: &AnalysisSnapshotReport,
    cancel: &CancelSignal<'_>,
) -> Result<(), PublicReportError> {
    let mut cells = BTreeMap::<&str, usize>::new();
    for cell in report.derived_metric_cells.iter().chain(
        report
            .operator_rows
            .iter()
            .flat_map(|row| row.metric_cells.iter()),
    ) {
        cancel.tick()?;
        *cells.entry(&cell.metric_cell_ref).or_default() += 1;
    }
    let rule_refs = report
        .rule_evaluations
        .iter()
        .flat_map(|evaluation| evaluation.input_observation_refs.iter())
        .chain(report.findings.iter().flat_map(|finding| {
            finding
                .recommendation
                .steps
                .iter()
                .flat_map(|step| step.evidence_refs.iter())
        }));
    for metric_cell_ref in rule_refs {
        cancel.tick()?;
        match cells
            .get(metric_cell_ref.as_str())
            .copied()
            .unwrap_or_default()
        {
            0 => {
                return Err(PublicReportError::MissingPublicMetricReference {
                    metric_cell_ref: metric_cell_ref.clone(),
                });
            }
            1 => {}
            _ => {
                return Err(PublicReportError::AmbiguousPublicMetricReference {
                    metric_cell_ref: metric_cell_ref.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Closes the reading DTO against the exact graph already carried by the
/// report. `None` is accepted only for retained pre-V2 payloads; assembly
/// above always emits `Some` and is checked here before returning it.
fn receipt(id: &str, version: &str, manifest_hash: &str) -> ReceiptComponent {
    ReceiptComponent {
        id: id.into(),
        version: version.into(),
        manifest_hash: manifest_hash.into(),
    }
}

fn metric_cell(observation: &MetricObservation) -> Result<MetricCell, PublicReportError> {
    let mut cell = raw_metric_cell(observation)?;
    adapt_display(&mut cell);
    Ok(cell)
}

/// One display form per unit: durations adapt their unit, and exact counts are
/// grouped by thousands like the execution summary, so the FE's dual
/// `1.000M (1000000)` form never reaches the reader. Rounded counts keep the
/// source text, which is the only honest statement of their precision.
fn adapt_display(cell: &mut MetricCell) {
    let text = match cell.canonical_unit() {
        Some("ns") => cell.canonical_number().and_then(adaptive_duration_ns),
        // Bytes: StarRocks prints three decimals of a binary KB / MB / GB, so
        // re-rendering the nearest whole byte under the same policy shows the
        // same digits (only trailing zeros go); precision stays in fidelity and
        // bounds. Exact integers (e.g. JSON Profiles, `1007.000 B`) adapt too.
        Some("byte")
            if matches!(
                cell.fidelity(),
                Some(doctor_contracts::Fidelity::Exact | doctor_contracts::Fidelity::Rounded)
            ) =>
        {
            cell.canonical_number().and_then(adaptive_bytes)
        }
        Some("item") if cell.fidelity() == Some(doctor_contracts::Fidelity::Exact) => {
            cell.canonical_number().and_then(exact_count)
        }
        _ => None,
    };
    if let Some(text) = text {
        cell.display_text = text;
    }
}

fn exact_count(number: &CanonicalNumber) -> Option<String> {
    if number.kind != NumberKind::Integer || number.scale.is_some() {
        return None;
    }
    let value = number
        .value
        .parse::<i128>()
        .ok()
        .filter(|value| *value >= 0)?;
    Some(crate::execution_summary::group_thousands(value))
}

/// Presentation of a typed, normalized integer only; never parses Profile units.
/// Source precision and statistic roles remain untouched. Integer arithmetic keeps
/// values above 2^53 exact and half-up rounding never turns a positive ns into zero.
pub(crate) fn adaptive_duration_ns(number: &CanonicalNumber) -> Option<String> {
    exact_integer(number).map(crate::display::duration_ns)
}

/// Bytes in the product's unit policy, from an integer or the nearest whole
/// byte of a non-negative decimal (half-up).
fn adaptive_bytes(number: &CanonicalNumber) -> Option<String> {
    let whole = match number.kind {
        NumberKind::Integer => exact_integer(number)?,
        NumberKind::Decimal => {
            let (integer, fraction) = number.value.split_once('.').unwrap_or((&number.value, ""));
            let integer = integer.parse::<i128>().ok().filter(|value| *value >= 0)?;
            if !fraction.chars().all(|digit| digit.is_ascii_digit()) {
                return None;
            }
            integer
                + i128::from(
                    fraction
                        .as_bytes()
                        .first()
                        .is_some_and(|digit| *digit >= b'5'),
                )
        }
    };
    Some(crate::display::bytes(whole))
}

fn exact_integer(number: &CanonicalNumber) -> Option<i128> {
    if number.kind != NumberKind::Integer || number.scale.is_some() {
        return None;
    }
    number.value.parse::<i128>().ok()
}

fn raw_metric_cell(observation: &MetricObservation) -> Result<MetricCell, PublicReportError> {
    let value = match observation.semantic_state.as_str() {
        "incompatible" => MetricCellValue::Incompatible {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        "unavailable" => MetricCellValue::Unavailable {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        "unknown_metric" => MetricCellValue::Unknown {
            canonical_number: (),
            canonical_unit: (),
            fidelity: (),
            bounds: (),
        },
        _ => match observation.parse_state {
            ParseState::Missing => MetricCellValue::Missing {
                canonical_number: (),
                canonical_unit: (),
                fidelity: (),
                bounds: (),
            },
            ParseState::Invalid => MetricCellValue::Invalid {
                canonical_number: (),
                canonical_unit: (),
                fidelity: (),
                bounds: (),
            },
            ParseState::Partial => MetricCellValue::Partial {
                canonical_number: (),
                canonical_unit: (),
                fidelity: (),
                bounds: (),
            },
            ParseState::Overflow => MetricCellValue::Overflow {
                canonical_number: (),
                canonical_unit: (),
                fidelity: (),
                bounds: (),
            },
            ParseState::Exact => MetricCellValue::Exact {
                canonical_number: observation
                    .canonical_number
                    .clone()
                    .ok_or(PublicReportError::ExactMetricMissingValue)?,
                canonical_unit: observation
                    .canonical_unit
                    .clone()
                    .ok_or(PublicReportError::ExactMetricMissingValue)?,
                observation: match (&observation.fidelity, observation.bounds.clone()) {
                    (doctor_contracts::Fidelity::Exact, None) => ObservedMetricFidelity::Lossless {
                        fidelity: LosslessFidelity::Exact,
                        bounds: (),
                    },
                    (doctor_contracts::Fidelity::Rounded, Some(bounds)) => {
                        ObservedMetricFidelity::Bounded {
                            fidelity: BoundedFidelity::Rounded,
                            bounds,
                        }
                    }
                    (doctor_contracts::Fidelity::Truncated, Some(bounds)) => {
                        ObservedMetricFidelity::Bounded {
                            fidelity: BoundedFidelity::Truncated,
                            bounds,
                        }
                    }
                    (doctor_contracts::Fidelity::Interval, Some(bounds)) => {
                        ObservedMetricFidelity::Bounded {
                            fidelity: BoundedFidelity::Interval,
                            bounds,
                        }
                    }
                    _ => ObservedMetricFidelity::Unknown {
                        fidelity: doctor_contracts::UnknownFidelity::Unknown,
                        bounds: (),
                    },
                },
            },
        },
    };
    Ok(MetricCell {
        match_key: observation.match_key.clone(),
        metric_cell_ref: format!(
            "metric:{}:{}",
            observation.subject_ref.as_deref().unwrap_or("unknown"),
            observation.metric_id.as_deref().unwrap_or("unknown")
        ),
        metric_id: observation
            .metric_id
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        subject_ref: observation
            .subject_ref
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        display_text: observation.display_value.clone(),
        // The adapter ordinals are zero-based; the public one-cell projection is ranked from one.
        order_rank: observation
            .source_ordinal
            .and_then(|ordinal| ordinal.checked_add(1)),
        source_refs: observation.source_refs.clone(),
        reasons: observation
            .diagnostics
            .iter()
            // A user-declared pack is parse-only context for the whole
            // observation. It does not change the parsed metric cell and is
            // already carried by diagnostics/version limitations.
            .filter(|diagnostic| diagnostic.code != "metric_registry_candidate_parse_only")
            .map(|diagnostic| diagnostic.code.clone())
            .collect(),
        value,
    })
}

#[cfg(test)]
mod metric_ref_suffix_tests {
    use super::*;

    #[test]
    fn distinct_prefixes_use_the_short_suffix() {
        let hashes = ["a".repeat(64), "b".repeat(64), canonical_hash(&"c")];
        assert_eq!(metric_ref_suffix_len(&hashes), METRIC_REF_SUFFIX_HEX);
    }

    #[test]
    fn a_prefix_collision_keeps_the_full_hash_for_the_whole_group() {
        let shared = "0123456789ab";
        let hashes = [
            format!("{shared}{}", "1".repeat(52)),
            format!("{shared}{}", "2".repeat(52)),
            "f".repeat(64),
        ];
        let len = metric_ref_suffix_len(&hashes);
        assert_eq!(len, 64);
        let suffixes = hashes
            .iter()
            .map(|hash| &hash[..len])
            .collect::<BTreeSet<_>>();
        assert_eq!(suffixes.len(), hashes.len());
    }
}

#[cfg(test)]
mod duration_display_tests {
    use super::*;

    fn integer(value: &str) -> CanonicalNumber {
        CanonicalNumber {
            kind: NumberKind::Integer,
            value: value.into(),
            scale: None,
        }
    }

    #[test]
    fn adaptive_duration_thresholds_and_half_up_carry_are_integer_exact() {
        for (raw, expected) in [
            ("0", "0 ns"),
            ("1", "1 ns"),
            ("999", "999 ns"),
            ("1000", "1 μs"),
            ("1001", "1.001 μs"),
            ("999999", "999.999 μs"),
            ("1000000", "1 ms"),
            ("1000499", "1 ms"),
            ("1000500", "1.001 ms"),
            ("999999499", "999.999 ms"),
            ("999999500", "1 s"),
            ("1000000000", "1 s"),
            ("1000499999", "1 s"),
            ("1000500000", "1.001 s"),
            ("619380", "619.38 μs"),
            ("29545", "29.545 μs"),
            ("1206000", "1.206 ms"),
            ("31880", "31.88 μs"),
            ("-1000500", "−1.001 ms"),
            ("9007199254740993", "2502h 00m"),
            ("9223372036854775807", "2562047h 47m"),
            (
                "170141183460469231731687303715884105727",
                "47261439850130342147690917h 42m",
            ),
            (
                "-170141183460469231731687303715884105728",
                "−47261439850130342147690917h 42m",
            ),
        ] {
            assert_eq!(
                adaptive_duration_ns(&integer(raw)).as_deref(),
                Some(expected),
                "{raw}"
            );
        }
    }

    #[test]
    fn adaptive_duration_is_not_a_raw_metric_parser() {
        for raw in [
            "",
            "missing",
            "1ms",
            "1.5",
            "1e9",
            "170141183460469231731687303715884105728",
        ] {
            assert_eq!(adaptive_duration_ns(&integer(raw)), None, "{raw}");
        }
        let mut scaled = integer("1000");
        scaled.scale = Some(3);
        assert_eq!(adaptive_duration_ns(&scaled), None);
        scaled.scale = None;
        scaled.kind = NumberKind::Decimal;
        assert_eq!(adaptive_duration_ns(&scaled), None);
    }

    #[test]
    fn non_exact_metric_cells_keep_their_state_and_display() {
        for state in [
            "missing",
            "invalid",
            "partial",
            "overflow",
            "unknown",
            "unavailable",
            "incompatible",
        ] {
            let mut cell: MetricCell = serde_json::from_value(serde_json::json!({
                "metric_cell_ref":"metric:sample", "metric_id":"OperatorTotalTime",
                "subject_ref":"sample", "display_text":"source display", "order_rank":null,
                "source_refs":["text:1"], "reasons":["test-state"], "state":state,
                "canonical_number":null,"canonical_unit":null,"fidelity":null,"bounds":null
            }))
            .unwrap();
            let before = cell.clone();
            adapt_display(&mut cell);
            assert_eq!(cell, before, "{state}");
        }
    }

    #[test]
    fn exact_counts_drop_the_fe_dual_form_and_rounded_counts_keep_source_text() {
        let cell = |display: &str, value: &str, fidelity: &str, bounds: serde_json::Value| {
            serde_json::from_value::<MetricCell>(serde_json::json!({
                "metric_cell_ref":"metric:sample", "metric_id":"PullRowNum",
                "subject_ref":"sample", "display_text":display, "order_rank":null,
                "source_refs":["text:1"], "reasons":[], "state":"exact",
                "canonical_number":{"kind":"integer","value":value,"scale":null},
                "canonical_unit":"item","fidelity":fidelity,"bounds":bounds
            }))
            .unwrap()
        };
        let mut exact = cell(
            "1.000M (1000000)",
            "1000000",
            "exact",
            serde_json::Value::Null,
        );
        adapt_display(&mut exact);
        assert_eq!(exact.display_text, "1,000,000");
        let mut small = cell("22", "22", "exact", serde_json::Value::Null);
        adapt_display(&mut small);
        assert_eq!(small.display_text, "22");
        let bounds = serde_json::json!({
            "lower":{"kind":"integer","value":"59500","scale":null},
            "upper":{"kind":"integer","value":"60500","scale":null},
            "lower_inclusive":true,"upper_inclusive":false
        });
        let mut rounded = cell("60.000K", "60000", "rounded", bounds);
        let before = rounded.clone();
        adapt_display(&mut rounded);
        assert_eq!(rounded, before);
    }
}
