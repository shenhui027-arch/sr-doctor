//! Fragment / instance counts from the stored Profile's fragment sections.
//!
//! Source semantics (StarRocks 3.5.14
//! `fe/fe-core/src/main/java/com/starrocks/qe/scheduler/QueryRuntimeProfile.java`,
//! `buildQueryProfile`): each merged `Fragment N` profile gets
//! `InstanceNum = instanceProfiles.size()`; a fragment without instance
//! children is skipped before the counter is written. Only an exact,
//! registry-backed `InstanceNum` is counted. Anything else stays missing and
//! never becomes 0.

use std::collections::BTreeMap;

use doctor_contracts::{
    EXECUTION_SHAPE_RECIPE_ID, ExecutionShape, ExecutionShapeState, FragmentInstanceCount,
    NumberKind,
};

use crate::{AdapterOutput, CancelSignal, Cancelled, MetricObservation, ParseState};

const FRAGMENT_GROUP_PREFIX: &str = "fragment:";

/// Polls `cancel` per observation it indexes.
pub(crate) fn execution_shape(
    output: &AdapterOutput,
    cancel: &CancelSignal<'_>,
) -> Result<ExecutionShape, Cancelled> {
    let ir = &output.ir;
    if ir
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "runtime_fragment_ref_missing")
    {
        // A fragment header without an id was skipped by the adapter, so the
        // fragment list itself is incomplete.
        return Ok(ExecutionShape::unavailable("runtime_fragment_ref_missing"));
    }
    let fragments = ir
        .operator_groups
        .iter()
        .filter(|group| group.operator_group_id.starts_with(FRAGMENT_GROUP_PREFIX))
        .collect::<Vec<_>>();
    if fragments.is_empty() {
        return Ok(ExecutionShape::unavailable("runtime_fragments_unavailable"));
    }
    let mut ids = BTreeMap::<&str, usize>::new();
    for group in &fragments {
        let Some(fragment_id) = group.fragment_ref.as_deref() else {
            return Ok(ExecutionShape::unavailable("runtime_fragment_ref_missing"));
        };
        *ids.entry(fragment_id).or_default() += 1;
    }
    if ids.values().any(|count| *count != 1) {
        // e.g. an unmerged or concatenated Profile repeating `Fragment N:`.
        return Ok(ExecutionShape::unavailable("fragment_identity_ambiguous"));
    }
    let Ok(fragment_count) = u32::try_from(fragments.len()) else {
        return Ok(ExecutionShape::unavailable("fragment_count_overflow"));
    };
    // Inserted one by one: `collect` into a map sorts all keys in one step.
    let mut observations = BTreeMap::new();
    for observation in &ir.metric_observations {
        cancel.tick()?;
        observations.insert(observation.observation_id.as_str(), observation);
    }
    let fragments = fragments
        .into_iter()
        .map(|group| {
            let candidates = group
                .metric_observation_refs
                .iter()
                .filter_map(|id| observations.get(id.as_str()).copied())
                .filter(|observation| {
                    observation.metric_id.as_deref() == Some("InstanceNum")
                        && observation.scope == "fragment"
                })
                .collect::<Vec<_>>();
            let (instance_num, reasons, source_refs) = match candidates.as_slice() {
                [] => (
                    None,
                    vec!["fragment_instance_num_missing".to_owned()],
                    vec![],
                ),
                [observation] => match exact_positive_count(observation) {
                    Some(value) => (Some(value), vec![], observation.source_refs.clone()),
                    None => (
                        None,
                        vec!["fragment_instance_num_not_exact".to_owned()],
                        observation.source_refs.clone(),
                    ),
                },
                several => (
                    None,
                    vec!["fragment_instance_num_ambiguous".to_owned()],
                    several
                        .iter()
                        .flat_map(|observation| observation.source_refs.iter().cloned())
                        .collect(),
                ),
            };
            FragmentInstanceCount {
                fragment_id: group.fragment_ref.clone().unwrap_or_default(),
                instance_num,
                reasons,
                source_refs: group
                    .source_refs
                    .iter()
                    .cloned()
                    .chain(source_refs)
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    let instance_count = fragments.iter().try_fold(0_u32, |total, fragment| {
        fragment
            .instance_num
            .and_then(|value| total.checked_add(value))
    });
    let (state, reasons) = if instance_count.is_some() {
        (ExecutionShapeState::Complete, vec![])
    } else {
        (
            ExecutionShapeState::Partial,
            vec!["fragment_instance_num_unavailable".to_owned()],
        )
    };
    Ok(ExecutionShape {
        state,
        recipe_id: EXECUTION_SHAPE_RECIPE_ID.to_owned(),
        fragment_count: Some(fragment_count),
        instance_count,
        fragments,
        reasons,
    })
}

/// FE writes `InstanceNum` only for a fragment with at least one instance, so
/// an exact 0 is not a valid reading.
fn exact_positive_count(observation: &MetricObservation) -> Option<u32> {
    if observation.parse_state != ParseState::Exact
        || observation.semantic_state != "known"
        || observation.canonical_unit.as_deref() != Some("item")
    {
        return None;
    }
    let number = observation.canonical_number.as_ref()?;
    if number.kind != NumberKind::Integer || number.scale.is_some() {
        return None;
    }
    number.value.parse::<u32>().ok().filter(|value| *value > 0)
}
