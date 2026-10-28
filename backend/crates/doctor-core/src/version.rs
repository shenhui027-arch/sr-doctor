use std::collections::BTreeSet;

use doctor_contracts::{
    AnalysisReceipt, Compatibility, CompatibilityCheck, CompatibilityCheckState, CompletenessState,
    Diagnostic, DimensionCompleteness, Fidelity, GraphView, MetricCellState, ProfileContext,
    SupportStatus, VersionContext, VersionEvidence, VersionSource,
};
use starrocks_version_pack_3_5_14::{
    PACK_ID, PRODUCT_PACK_VERSION, SEMANTIC_MANIFEST_HASH, StarRocks3514Pack, UPSTREAM_RELEASE,
    VersionPackError,
    deltas::{DELTAS_HASH, ReleaseProfile, ReleaseResolution, resolve_release},
    load,
};

use crate::{
    AdapterOutput, CancelSignal, Cancelled, GraphEvidence, GraphResolutionError, SourceRef,
    portable_facts, resolve_graph,
};

use crate::report::{GRAPH_COMPONENT_HASH, GRAPH_COMPONENT_ID, GRAPH_COMPONENT_VERSION};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VersionResolutionError {
    Pack(VersionPackError),
    Graph(GraphResolutionError),
    InvalidVersionHint,
    ReceiptComponentInvalid(&'static str),
    GraphProvenanceInvalid,
    /// The caller's `CancelSignal` stopped the resolution; `output` must be
    /// discarded.
    Cancelled,
}

impl From<Cancelled> for VersionResolutionError {
    fn from(_: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// Resolves version evidence only after the adapter has extracted it. No hint may override a report.
pub fn resolve_version_context(
    preliminary: &VersionContext,
    profile_context: &ProfileContext,
) -> Result<VersionContext, VersionResolutionError> {
    let declared = match preliminary.user_declared_version.as_deref() {
        None => None,
        Some(hint) => match resolve_release(hint) {
            ReleaseResolution::Audited(profile) => Some(profile),
            _ => return Err(VersionResolutionError::InvalidVersionHint),
        },
    };
    let pack = load().map_err(VersionResolutionError::Pack)?;
    let reported = preliminary.reported_version.clone();
    let hint = preliminary.user_declared_version.clone();
    let reported_refs = reported
        .iter()
        .map(|evidence| evidence.source_ref.clone())
        .collect::<Vec<_>>();
    let mut checks = Vec::new();
    let mut issues = Vec::new();

    let (effective_version, version_source, selected_pack, mut compatibility, mut support_status) =
        match reported.as_ref() {
            Some(evidence)
                if evidence.value != UPSTREAM_RELEASE
                    && let ReleaseResolution::Audited(profile) =
                        resolve_release(&evidence.value) =>
            {
                checks.push(check(
                    "reported_version",
                    "version",
                    CompatibilityCheckState::Matched,
                    Some(&profile.line),
                    Some(&evidence.value),
                    reported_refs.clone(),
                ));
                checks.push(check(
                    "version_pack",
                    "pack",
                    CompatibilityCheckState::Matched,
                    Some(PACK_ID),
                    Some(&format!("{PACK_ID}+{}", profile.line)),
                    reported_refs.clone(),
                ));
                push_release_delta_checks(&profile, &reported_refs, &mut checks, &mut issues);
                (
                    Some(evidence.value.clone()),
                    VersionSource::Detected,
                    Some(selected_pack_for(&pack, &profile)),
                    Compatibility::Compatible,
                    SupportStatus::Supported,
                )
            }
            Some(evidence)
                if let ReleaseResolution::NotAudited { line } =
                    resolve_release(&evidence.value) =>
            {
                checks.push(check(
                    "version_pack",
                    "pack",
                    CompatibilityCheckState::Conflict,
                    Some(&line),
                    Some(&evidence.value),
                    reported_refs.clone(),
                ));
                issues.push("release_not_audited".into());
                (
                    Some(evidence.value.clone()),
                    VersionSource::Detected,
                    None,
                    Compatibility::Incompatible,
                    SupportStatus::Unsupported,
                )
            }
            Some(evidence) if evidence.value == UPSTREAM_RELEASE => {
                checks.push(check(
                    "reported_version",
                    "version",
                    CompatibilityCheckState::Matched,
                    Some(UPSTREAM_RELEASE),
                    Some(&evidence.value),
                    reported_refs.clone(),
                ));
                checks.push(check(
                    "version_pack",
                    "pack",
                    CompatibilityCheckState::Matched,
                    Some(PACK_ID),
                    Some(PACK_ID),
                    reported_refs.clone(),
                ));
                (
                    Some(evidence.value.clone()),
                    VersionSource::Detected,
                    Some(pack.selected_pack()),
                    Compatibility::Compatible,
                    SupportStatus::Supported,
                )
            }
            Some(evidence) if is_release(&evidence.value) => {
                checks.push(check(
                    "version_pack",
                    "pack",
                    CompatibilityCheckState::Conflict,
                    Some(UPSTREAM_RELEASE),
                    Some(&evidence.value),
                    reported_refs.clone(),
                ));
                issues.push("no_registered_pack".into());
                (
                    Some(evidence.value.clone()),
                    VersionSource::Detected,
                    None,
                    Compatibility::Incompatible,
                    SupportStatus::Unsupported,
                )
            }
            Some(evidence) => {
                checks.push(check(
                    "reported_version",
                    "version",
                    CompatibilityCheckState::Unknown,
                    Some("semantic_release"),
                    Some(&evidence.value),
                    reported_refs.clone(),
                ));
                issues.push("reported_version_unrecognized".into());
                (
                    Some(evidence.value.clone()),
                    VersionSource::Detected,
                    None,
                    Compatibility::Inconclusive,
                    SupportStatus::Unknown,
                )
            }
            None if let Some(profile) = declared.as_ref() => {
                checks.push(check(
                    "reported_version",
                    "version",
                    CompatibilityCheckState::Missing,
                    Some(&profile.release),
                    None,
                    vec![],
                ));
                checks.push(check(
                    "user_declared_version",
                    "request",
                    CompatibilityCheckState::Matched,
                    Some(&profile.release),
                    hint.as_deref(),
                    vec![],
                ));
                issues.push("user_declared_parse_only".into());
                (
                    hint.clone(),
                    VersionSource::UserDeclared,
                    Some(selected_pack_for(&pack, profile)),
                    Compatibility::Inconclusive,
                    SupportStatus::ParseOnly,
                )
            }
            None if hint.is_some() => {
                checks.push(check(
                    "reported_version",
                    "version",
                    CompatibilityCheckState::Missing,
                    Some(UPSTREAM_RELEASE),
                    None,
                    vec![],
                ));
                issues.push("no_registered_pack".into());
                (
                    hint.clone(),
                    VersionSource::UserDeclared,
                    None,
                    Compatibility::Inconclusive,
                    SupportStatus::Unknown,
                )
            }
            None => {
                checks.push(check(
                    "reported_version",
                    "version",
                    CompatibilityCheckState::Missing,
                    Some(UPSTREAM_RELEASE),
                    None,
                    vec![],
                ));
                issues.push("reported_version_missing".into());
                (
                    None,
                    VersionSource::Unknown,
                    None,
                    Compatibility::Inconclusive,
                    SupportStatus::Unknown,
                )
            }
        };

    if let (Some(evidence), Some(hint)) = (reported.as_ref(), hint.as_ref())
        && evidence.value != *hint
    {
        checks.push(check(
            "version_hint_conflict",
            "request",
            CompatibilityCheckState::Conflict,
            Some(&evidence.value),
            Some(hint),
            reported_refs.clone(),
        ));
        issues.push("version_hint_conflicts_with_reported".into());
        if selected_pack.is_some() {
            compatibility = Compatibility::Partial;
            support_status = SupportStatus::ParseOnly;
        }
    }

    if let Some(reason) = profile_incompatibility(profile_context) {
        if selected_pack.is_some() {
            checks.push(check(
                "profile_context",
                "profile",
                CompatibilityCheckState::Conflict,
                Some("level_1+finished+final+merged"),
                Some(reason),
                profile_context.evidence_refs.clone(),
            ));
            issues.push(reason.into());
            compatibility = Compatibility::Partial;
            support_status = SupportStatus::ParseOnly;
        }
    } else if profile_context_is_complete(profile_context) && selected_pack.is_some() {
        checks.push(check(
            "profile_context",
            "profile",
            CompatibilityCheckState::Matched,
            Some("level_1+finished+final+merged"),
            Some("level_1+finished+final+merged"),
            profile_context.evidence_refs.clone(),
        ));
    } else if selected_pack.is_some() {
        checks.push(check(
            "profile_context",
            "profile",
            CompatibilityCheckState::Missing,
            Some("level_1+finished+final+merged"),
            None,
            profile_context.evidence_refs.clone(),
        ));
        issues.push("profile_context_insufficient_for_compatibility".into());
        compatibility = Compatibility::Partial;
        support_status = SupportStatus::ParseOnly;
    }

    let detection_evidence = reported
        .clone()
        .into_iter()
        .collect::<Vec<VersionEvidence>>();
    let pack_match = match selected_pack.as_ref() {
        Some(pack) if pack.release_deltas.is_some() => {
            doctor_contracts::PackMatch::BaselineWithDeltas
        }
        Some(_) if effective_version.as_deref() == Some(UPSTREAM_RELEASE) => {
            doctor_contracts::PackMatch::ExactPack
        }
        _ => doctor_contracts::PackMatch::NoPack,
    };
    Ok(VersionContext {
        reported_version: reported,
        user_declared_version: hint,
        effective_version,
        version_source,
        selected_pack,
        pack_match,
        compatibility,
        support_status,
        detection_evidence,
        compatibility_checks: checks,
        issues,
    })
}

/// Applies release semantics and a proven graph to an adapter-stage IR.
/// `analysis_key` intentionally stays source+receipt+options based.
pub fn resolve_version_and_graph(
    output: &mut AdapterOutput,
    graph_evidence: GraphEvidence,
) -> Result<(), VersionResolutionError> {
    resolve_version_and_graph_cancellable(output, graph_evidence, &CancelSignal::never())
}

/// `resolve_version_and_graph` polling `cancel` per source reference it
/// indexes or checks. On `VersionResolutionError::Cancelled` the output is
/// half-resolved at most and must be discarded.
pub fn resolve_version_and_graph_cancellable(
    output: &mut AdapterOutput,
    graph_evidence: GraphEvidence,
    cancel: &CancelSignal<'_>,
) -> Result<(), VersionResolutionError> {
    validate_resolution_receipt(&output.ir.analysis_receipt)?;
    let version = resolve_version_context(&output.ir.version, &output.ir.profile_context)?;
    let graph = resolve_graph(graph_evidence).map_err(VersionResolutionError::Graph)?;
    cancel.check()?;
    let known = known_source_refs(&output.source_refs, cancel)?;
    validate_version_provenance(&version, &output.ir.profile_context, &known)?;
    validate_graph_provenance(&graph, &known, cancel)?;
    drop(known);
    output.ir.version = version;
    output.ir.graph = graph;
    if output.ir.version.compatibility == Compatibility::Partial {
        let limitation = profile_incompatibility(&output.ir.profile_context)
            .unwrap_or("profile_context_insufficient_for_compatibility");
        if !output
            .ir
            .profile_context
            .limitations
            .iter()
            .any(|existing| existing == limitation)
        {
            output
                .ir
                .profile_context
                .limitations
                .push(limitation.into());
        }
    }
    output.ir.completeness.version = version_completeness(&output.ir.version);
    output.ir.completeness.graph = output.ir.graph.completeness.clone();
    output.ir.diagnostics.retain(|diagnostic| {
        diagnostic.code != "version_selection_deferred"
            && !(diagnostic.code == "graph_unavailable"
                && output.ir.graph.state != doctor_contracts::GraphState::Unavailable)
            && diagnostic.code != "metric_registry_not_selected"
            && diagnostic.code != "metric_registry_candidate_parse_only"
            && diagnostic.code != "version_incompatible"
            && diagnostic.code != "version_unknown"
            && diagnostic.code != "version_user_declared_parse_only"
            && diagnostic.code != "profile_context_partial"
    });
    append_resolution_diagnostics(output);
    cancel.check()?;
    isolate_metrics_without_selected_pack(output);
    if let Some(release) = resolved_release(&output.ir.version) {
        apply_release_deltas(output, &release);
    }
    cancel.check()?;
    output.ir.completeness.summary = DimensionCompleteness {
        state: CompletenessState::Partial,
        present_count: output
            .ir
            .completeness
            .metrics
            .present_count
            .saturating_add(output.ir.completeness.version.present_count)
            .saturating_add(output.ir.completeness.graph.present_count),
        expected_count: None,
        reasons: vec!["adapter_and_resolver_partial".into()],
    };
    let refreshed = portable_facts(
        &output.ir.version,
        output.ir.profile_context.clone(),
        output.ir.analysis_receipt.components.clone(),
        &[],
        &output.ir.graph,
        &output.ir.findings,
        &output.ir.rule_evaluations,
    );
    output.facts.version = refreshed.version;
    output.facts.profile_context = refreshed.profile_context;
    output.facts.receipt = refreshed.receipt;
    output.facts.graph = refreshed.graph;
    Ok(())
}

/// Every source ref id the adapter recorded; both provenance checks use it.
fn known_source_refs<'a>(
    source_refs: &'a [SourceRef],
    cancel: &CancelSignal<'_>,
) -> Result<BTreeSet<&'a str>, Cancelled> {
    source_refs
        .iter()
        .map(|source| cancel.tick().map(|()| source.ref_id.as_str()))
        .collect()
}

fn validate_version_provenance(
    context: &VersionContext,
    profile_context: &ProfileContext,
    known: &BTreeSet<&str>,
) -> Result<(), VersionResolutionError> {
    let is_known = |reference: &str| known.contains(reference);
    if profile_context.evidence_refs.is_empty()
        || context
            .reported_version
            .iter()
            .any(|evidence| !is_known(&evidence.source_ref))
        || context
            .detection_evidence
            .iter()
            .any(|evidence| !is_known(&evidence.source_ref))
        || context
            .compatibility_checks
            .iter()
            .flat_map(|check| check.source_refs.iter())
            .any(|reference| !is_known(reference))
        || profile_context
            .evidence_refs
            .iter()
            .any(|reference| !is_known(reference))
    {
        return Err(VersionResolutionError::GraphProvenanceInvalid);
    }
    Ok(())
}

fn validate_resolution_receipt(receipt: &AnalysisReceipt) -> Result<(), VersionResolutionError> {
    let expected_pack = receipt.components.iter().any(|component| {
        component.id == PACK_ID
            && component.version == PRODUCT_PACK_VERSION
            && component.manifest_hash == SEMANTIC_MANIFEST_HASH
    });
    if !expected_pack {
        return Err(VersionResolutionError::ReceiptComponentInvalid(
            "version_pack",
        ));
    }
    let expected_graph = receipt.components.iter().any(|component| {
        component.id == GRAPH_COMPONENT_ID
            && component.version == GRAPH_COMPONENT_VERSION
            && component.manifest_hash == GRAPH_COMPONENT_HASH
    });
    if !expected_graph {
        return Err(VersionResolutionError::ReceiptComponentInvalid(
            "graph_resolver",
        ));
    }
    Ok(())
}

fn validate_graph_provenance(
    graph: &GraphView,
    known: &BTreeSet<&str>,
    cancel: &CancelSignal<'_>,
) -> Result<(), VersionResolutionError> {
    let references = graph
        .source_refs
        .iter()
        .chain(graph.nodes.iter().flat_map(|node| node.source_refs.iter()))
        .chain(
            graph
                .nodes
                .iter()
                .flat_map(|node| {
                    node.native_operator_refs
                        .iter()
                        .chain(node.subordinate_operator_refs.iter())
                })
                .flat_map(|operator| operator.source_refs.iter()),
        )
        .chain(
            graph
                .edges
                .iter()
                .flat_map(|edge| edge.evidence_refs.iter()),
        )
        .chain(
            graph
                .unresolved_relations
                .iter()
                .flat_map(|relation| relation.source_refs.iter()),
        );
    for reference in references {
        cancel.tick()?;
        if !known.contains(reference.as_str()) {
            return Err(VersionResolutionError::GraphProvenanceInvalid);
        }
    }
    Ok(())
}

fn isolate_metrics_without_selected_pack(output: &mut AdapterOutput) {
    let semantic_state = match output.ir.version.compatibility {
        Compatibility::Incompatible => Some((
            MetricCellState::Incompatible,
            "incompatible",
            "metric_registry_not_selected",
        )),
        Compatibility::Inconclusive if output.ir.version.selected_pack.is_none() => Some((
            MetricCellState::Unavailable,
            "unavailable",
            "metric_registry_not_selected",
        )),
        _ => None,
    };
    if let Some((cell_state, semantic_state, diagnostic_code)) = semantic_state {
        for observation in &mut output.ir.metric_observations {
            observation.semantic_state = semantic_state.into();
            observation.definition_ref = None;
            observation.canonical_number = None;
            observation.canonical_unit = None;
            observation.fidelity = Fidelity::Unknown;
            observation.bounds = None;
            observation.display_value = "—".into();
            observation.strategy_origin = "unknown".into();
            let diagnostic = metric_diagnostic(diagnostic_code, observation);
            observation.diagnostics.push(diagnostic.clone());
            output.ir.diagnostics.push(diagnostic);
        }
        for metric in &mut output.facts.metric_cells {
            metric.state = cell_state.clone();
            metric.canonical_number = None;
            metric.canonical_unit = None;
            metric.fidelity = None;
            metric.bounds = None;
            metric.order_rank = None;
        }
        output.ir.completeness.metrics = DimensionCompleteness {
            state: CompletenessState::Unavailable,
            present_count: 0,
            expected_count: None,
            reasons: vec!["metric_registry_not_selected".into()],
        };
    } else if output.ir.version.version_source == VersionSource::UserDeclared {
        for observation in &mut output.ir.metric_observations {
            let diagnostic = metric_diagnostic("metric_registry_candidate_parse_only", observation);
            observation.diagnostics.push(diagnostic.clone());
            output.ir.diagnostics.push(diagnostic);
        }
    }
}

/// Release deltas that change what the parsed metrics mean: an upstream
/// defect makes merged maxima no evidence, and the release's semantic issues
/// become Profile limitations the report shows.
fn apply_release_deltas(output: &mut AdapterOutput, release: &ReleaseProfile) {
    if release.merged_max_unreliable {
        for observation in &mut output.ir.metric_observations {
            if observation.value_role != "reported_max" || observation.canonical_number.is_none() {
                continue;
            }
            observation.semantic_state = "unavailable".into();
            observation.canonical_number = None;
            observation.canonical_unit = None;
            observation.fidelity = Fidelity::Unknown;
            observation.bounds = None;
            observation.display_value = "—".into();
            let diagnostic = metric_diagnostic("merged_max_upstream_defect", observation);
            observation.diagnostics.push(diagnostic.clone());
            output.ir.diagnostics.push(diagnostic);
        }
    }
    for code in [
        "release_lacks_registered_metrics",
        "merged_max_upstream_defect",
        "output_rows_recipe_not_official",
    ] {
        let raised = output.ir.version.issues.iter().any(|issue| issue == code);
        let listed = output
            .ir
            .profile_context
            .limitations
            .iter()
            .any(|existing| existing == code);
        if raised && !listed {
            output.ir.profile_context.limitations.push(code.into());
        }
    }
}

fn append_resolution_diagnostics(output: &mut AdapterOutput) {
    let fallback = output
        .source_refs
        .last()
        .map(|source| source.ref_id.clone())
        .into_iter()
        .collect::<Vec<_>>();
    let reported_refs = output
        .ir
        .version
        .reported_version
        .iter()
        .map(|evidence| evidence.source_ref.clone())
        .collect::<Vec<_>>();
    let profile_refs = output.ir.profile_context.evidence_refs.clone();
    let diagnostic = match output.ir.version.compatibility {
        Compatibility::Incompatible => Some(("version_incompatible", reported_refs)),
        Compatibility::Inconclusive
            if output.ir.version.version_source == VersionSource::UserDeclared =>
        {
            Some(("version_user_declared_parse_only", fallback))
        }
        Compatibility::Inconclusive if output.ir.version.selected_pack.is_none() => {
            Some(("version_unknown", fallback))
        }
        Compatibility::Partial
            if output.ir.version.version_source == VersionSource::UserDeclared =>
        {
            Some(("version_user_declared_parse_only", fallback))
        }
        Compatibility::Partial => Some(("profile_context_partial", profile_refs)),
        _ => None,
    };
    if let Some((code, source_refs)) = diagnostic {
        output.ir.diagnostics.push(Diagnostic {
            code: code.into(),
            stage: "normalize".into(),
            severity: "warning".into(),
            recoverability: "partial".into(),
            message_id: code.into(),
            source_refs,
            affected_ids: vec![],
            details: std::collections::BTreeMap::new(),
        });
    }
}

fn metric_diagnostic(code: &str, observation: &crate::MetricObservation) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        stage: "normalize".into(),
        severity: "warning".into(),
        recoverability: "partial".into(),
        message_id: code.into(),
        source_refs: observation.source_refs.clone(),
        affected_ids: observation.metric_id.clone().into_iter().collect(),
        details: std::collections::BTreeMap::new(),
    }
}

fn version_completeness(context: &VersionContext) -> DimensionCompleteness {
    let has_evidence =
        context.reported_version.is_some() || context.user_declared_version.is_some();
    let state = match context.compatibility {
        Compatibility::Compatible => CompletenessState::Complete,
        Compatibility::Partial | Compatibility::Incompatible | Compatibility::Inconclusive
            if has_evidence =>
        {
            CompletenessState::Partial
        }
        Compatibility::Partial | Compatibility::Incompatible | Compatibility::Inconclusive => {
            CompletenessState::Unavailable
        }
    };
    DimensionCompleteness {
        state,
        present_count: u32::from(has_evidence),
        expected_count: Some(1),
        reasons: context.issues.clone(),
    }
}

fn profile_context_is_complete(profile: &ProfileContext) -> bool {
    matches!(
        profile.profile_level,
        doctor_contracts::ProfileLevel::Level1
    ) && matches!(profile.query_state, doctor_contracts::QueryState::Finished)
        && matches!(profile.snapshot_kind, doctor_contracts::SnapshotKind::Final)
        && matches!(profile.merge_state, doctor_contracts::MergeState::Merged)
        && !profile.evidence_refs.is_empty()
}

fn profile_incompatibility(profile: &ProfileContext) -> Option<&'static str> {
    if matches!(
        profile.profile_level,
        doctor_contracts::ProfileLevel::Level2
    ) {
        Some("profile_level_2")
    } else if matches!(profile.query_state, doctor_contracts::QueryState::Running) {
        Some("profile_running_snapshot")
    } else if matches!(profile.query_state, doctor_contracts::QueryState::Error) {
        Some("profile_error")
    } else if matches!(
        profile.snapshot_kind,
        doctor_contracts::SnapshotKind::Runtime
    ) {
        Some("profile_runtime_snapshot")
    } else if matches!(profile.merge_state, doctor_contracts::MergeState::Unmerged) {
        Some("profile_unmerged")
    } else {
        None
    }
}

/// The baseline pack read with a release's audited deltas.
fn selected_pack_for(
    pack: &StarRocks3514Pack,
    profile: &ReleaseProfile,
) -> doctor_contracts::SelectedVersionPack {
    let mut selected = pack.selected_pack();
    if !profile.is_baseline() {
        selected.release_deltas = Some(doctor_contracts::ReleaseDeltas {
            release: profile.release.clone(),
            line: profile.line.clone(),
            delta_ids: profile.delta_ids.clone(),
            deltas_hash: DELTAS_HASH.to_owned(),
        });
    }
    selected
}

/// What a release changes, as checks and issues the report shows: registered
/// metrics it never emits, and semantics it distrusts for it.
fn push_release_delta_checks(
    profile: &ReleaseProfile,
    refs: &[String],
    checks: &mut Vec<CompatibilityCheck>,
    issues: &mut Vec<String>,
) {
    if !profile.absent_metrics.is_empty() {
        checks.push(check(
            "release_registered_metrics",
            "metrics",
            CompatibilityCheckState::Missing,
            Some("all_registered"),
            Some(&profile.absent_metrics.join(",")),
            refs.to_vec(),
        ));
        issues.push("release_lacks_registered_metrics".into());
    }
    if profile.merged_max_unreliable {
        issues.push("merged_max_upstream_defect".into());
    }
    if !profile.output_rows_recipe_available {
        issues.push("output_rows_recipe_not_official".into());
    }
}

/// The audited release behind a resolved version context, if any.
pub(crate) fn resolved_release(context: &VersionContext) -> Option<ReleaseProfile> {
    let pack = context.selected_pack.as_ref()?;
    let release = pack
        .release_deltas
        .as_ref()
        .map_or(UPSTREAM_RELEASE, |deltas| deltas.release.as_str());
    match resolve_release(release) {
        ReleaseResolution::Audited(profile) => Some(profile),
        _ => None,
    }
}

fn is_release(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!((parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if !major.is_empty() && !minor.is_empty() && !patch.is_empty()
            && [major, minor, patch].into_iter().all(|part| part.bytes().all(|byte| byte.is_ascii_digit())))
}

fn check(
    check_id: &str,
    domain: &str,
    state: CompatibilityCheckState,
    expected: Option<&str>,
    observed: Option<&str>,
    source_refs: Vec<String>,
) -> CompatibilityCheck {
    CompatibilityCheck {
        check_id: check_id.into(),
        domain: domain.into(),
        state,
        expected: expected.map(str::to_owned),
        observed: observed.map(str::to_owned),
        source_refs,
    }
}
