//! Shared evidence gates for official 3.5.14 stored Runtime Profiles.
//!
//! These helpers deliberately keep the adapter's public observation merge state
//! conservative.  A caller must hold the returned proof before using the
//! official final-profile counter recipe.

use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    Compatibility, MergeState, PackMatch, ProfileLevel, QueryState, RuntimeSourceRole,
    SnapshotKind, SupportStatus, VersionSource,
};

use crate::{MetricObservation, NormalizedIr};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OfficialStoredProfileSourceStageProof {
    pub(crate) evidence_refs: Vec<String>,
}

/// Proves that the official 3.5.14 Level-1 stored-profile builder completed
/// source-counter processing before it emitted this pretty-printed profile.
/// It does not mutate `MetricObservation::source_merge_state`.
pub(crate) fn official_stored_profile_source_stage_proof(
    ir: &NormalizedIr,
    known_refs: &BTreeSet<String>,
    verified_formatter_shape: bool,
) -> Option<OfficialStoredProfileSourceStageProof> {
    // In 3.5.14, Level-1 buildQueryProfile(true) first applies
    // RuntimeProfile.mergeIsomorphicProfiles (including each Counter strategy)
    // and then derives QueryCumulativeOperatorTime from that processed tree.
    if !verified_formatter_shape
        || !final_supported_release(ir)
        || ir.profile_context.limitations.iter().any(|limitation| {
            matches!(
                limitation.as_str(),
                "runtime_profile_not_identical"
                    | "runtime_profile_missing_instances"
                    | "runtime_operator_running"
                    | "runtime_profile_incomplete_marker_malformed"
            )
        })
        || ir.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code.as_str(),
                "runtime_profile_not_identical"
                    | "runtime_profile_missing_instances"
                    | "runtime_operator_running"
                    | "runtime_profile_incomplete_marker_malformed"
            )
        })
        || !unique_nonempty(&ir.profile_context.evidence_refs)
        || !refs_valid(&ir.profile_context.evidence_refs, known_refs)
        || ir.pipeline_containers.is_empty()
        || ir.physical_operator_memberships.is_empty()
        || ir.pipeline_containers.iter().any(|container| {
            container.source_refs.is_empty() || !refs_valid(&container.source_refs, known_refs)
        })
        || ir.physical_operator_memberships.iter().any(|membership| {
            membership.source_refs.is_empty() || !refs_valid(&membership.source_refs, known_refs)
        })
    {
        return None;
    }
    let mut evidence_refs = ir.profile_context.evidence_refs.clone();
    let known_container_refs = ir
        .pipeline_containers
        .iter()
        .map(|container| container.container_ref.as_str())
        .collect::<BTreeSet<_>>();
    if known_container_refs.len() != ir.pipeline_containers.len() {
        return None;
    }
    for group in &ir.operator_groups {
        if group.plan_node_id.is_none() {
            continue;
        }
        let memberships = ir
            .physical_operator_memberships
            .iter()
            .filter(|membership| {
                membership.plan_node_id == group.plan_node_id
                    && membership.fragment_ref == group.fragment_ref
            })
            .collect::<Vec<_>>();
        if memberships.is_empty() {
            continue;
        }
        let mut roles = BTreeMap::new();
        // ExplainAnalyzer.ProfileNodeParser keeps every unmarked operator of a
        // plan node as native, in print order, however many share one
        // pipeline (LIMIT + EXCHANGE_SOURCE after a LIMIT push-down). What the
        // Level-1 merge never prints is the same operator twice in one
        // pipeline: a repeated native type there is an unmerged duplicate.
        let mut native_types_by_container = BTreeSet::<(&str, &str)>::new();
        for membership in memberships {
            let container_ref = membership.container_ref.as_deref()?;
            if !known_container_refs.contains(container_ref)
                || !matches!(
                    membership.source_role,
                    RuntimeSourceRole::Native | RuntimeSourceRole::Subordinate
                )
                || roles
                    .insert(
                        membership.physical_operator_ref.clone(),
                        membership.source_role.clone(),
                    )
                    .is_some()
            {
                return None;
            }
            if membership.source_role == RuntimeSourceRole::Native
                && !native_types_by_container
                    .insert((container_ref, membership.canonical_type.as_str()))
            {
                return None;
            }
        }
        if !roles
            .values()
            .any(|role| *role == RuntimeSourceRole::Native)
        {
            return None;
        }
    }
    evidence_refs.extend(
        ir.pipeline_containers
            .iter()
            .flat_map(|container| container.source_refs.iter().cloned()),
    );
    evidence_refs.extend(
        ir.physical_operator_memberships
            .iter()
            .flat_map(|membership| membership.source_refs.iter().cloned()),
    );
    Some(OfficialStoredProfileSourceStageProof {
        evidence_refs: ordered_unique(evidence_refs),
    })
}

pub(crate) fn official_stored_profile_metric_shape(
    observation: &MetricObservation,
    metric_id: &str,
) -> bool {
    if observation.source_merge_state != "unknown"
        || observation.strategy_origin != "registry"
        || observation.counter_strategy.is_some()
    {
        return false;
    }
    match metric_id {
        // Both are derived by QueryRuntimeProfile.buildQueryProfile after the Level-1 merge
        // (QuerySpillBytes: the sum of per-instance reports, QueryRuntimeProfile.java:469-473).
        "QueryCumulativeOperatorTime" | "QuerySpillBytes" => {
            observation.value_role == "reported_base"
                && observation.source_name == metric_id
                && observation.scope == "query"
                && observation.operator_ref.is_none()
                && observation.runtime_occurrence_ref.is_none()
        }
        "OperatorTotalTime" | "ScanTime" | "NetworkTime" | "PushRowNum" | "PullRowNum"
        | "BytesSent" => {
            matches!(
                observation.value_role.as_str(),
                "reported_base" | "reported_min" | "reported_max"
            ) && observation.source_name
                == match observation.value_role.as_str() {
                    "reported_min" => format!("__MIN_OF_{metric_id}"),
                    "reported_max" => format!("__MAX_OF_{metric_id}"),
                    _ => metric_id.to_owned(),
                }
                && observation.scope == "operator"
                && observation.runtime_occurrence_ref.is_some()
        }
        _ => false,
    }
}

/// A final, merged Level 1 Profile of an audited release: the 3.5.14 baseline
/// or the baseline read with a release's audited deltas. The applicability
/// check id `version_profile:final_supported_3514` keeps its name.
pub(crate) fn final_supported_release(ir: &NormalizedIr) -> bool {
    ir.version.effective_version.is_some()
        && crate::version::resolved_release(&ir.version).is_some()
        && ir.version.version_source == VersionSource::Detected
        && matches!(
            ir.version.pack_match,
            PackMatch::ExactPack | PackMatch::BaselineWithDeltas
        )
        && ir.version.compatibility == Compatibility::Compatible
        && ir.version.support_status == SupportStatus::Supported
        && matches!(ir.profile_context.profile_level, ProfileLevel::Level1)
        && matches!(ir.profile_context.query_state, QueryState::Finished)
        && matches!(ir.profile_context.snapshot_kind, SnapshotKind::Final)
        && matches!(ir.profile_context.merge_state, MergeState::Merged)
}

fn refs_valid(refs: &[String], known_refs: &BTreeSet<String>) -> bool {
    !refs.is_empty()
        && refs
            .iter()
            .all(|reference| !reference.trim().is_empty() && known_refs.contains(reference))
}

fn unique_nonempty(refs: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    refs.iter()
        .all(|reference| !reference.trim().is_empty() && unique.insert(reference.as_str()))
}

fn ordered_unique(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}
