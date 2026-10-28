//! Explain snapshot assembly sits between the frozen formatter normalizer and
//! the public connected API.  It never feeds Explain into the Profile graph path.

use std::{collections::BTreeSet, fmt};

use doctor_contracts::{
    CompletenessState, Diagnostic, DimensionCompleteness, DirectionSemantics, EdgeSource,
    ExplainAggregatePhase, ExplainCardFactSlot, ExplainCardFactSlotState, ExplainCardSlotId,
    ExplainCardinalityEstimate, ExplainCardinalityNumberKind, ExplainDistribution,
    ExplainFactEvidenceKind, ExplainFactKind, ExplainFactValue, ExplainJoinStrategy, ExplainLevel,
    ExplainMetadataAvailability, ExplainMode, ExplainNodeFact, ExplainNodeFactSet, ExplainReceipt,
    ExplainRuntimeState, ExplainSchemaVersion, ExplainSnapshot, ExplainSourceDisplayState,
    ExplainSourceKind, ExplainSourceLocator, ExplainSourceTruncationState, ExplainSourceView,
    ExplainVersionContext, GraphEdge, GraphEntityKind, GraphNode, GraphRelation, GraphState,
    GraphView, InputOrigin, InputRole, PackMatch, ReceiptComponent, RecipeRef, SelectedVersionPack,
    SourceLine, SourceQuality, TopologySource, UnresolvedRelation, canonical_hash,
    explain_facts_hash_v1,
};
use starrocks_version_pack_3_5_14::{
    PACK_ID, PRODUCT_PACK_VERSION, SEMANTIC_MANIFEST_HASH, UPSTREAM_COMMIT, UPSTREAM_RELEASE,
};

use crate::{
    ExplainLimits, ExplainNodeKind, ExplainPlanAggregatePhase, ExplainPlanDistribution,
    ExplainPlanFactKind, ExplainPlanFactValue, ExplainPlanFacts, ExplainPlanJoinStrategy,
    ExplainPlanNodeFact, ExplainPlanNodeFactSet, ExplainSourceSpan, ExplainStructureState,
    normalize_explain_costs,
};

const RESULT_REF: &str = "explain:result";
const DETECTED_VERSION_REF: &str = "explain:connection:detected_version";
const DEFAULT_DATABASE_REF: &str = "explain:connection:default_database";
const COSTS_RECIPES_NOT_APPLIED_FOR_RAW_TEXT_MODE: &str =
    "costs_recipes_not_applied_for_raw_text_mode";
const EXPLAIN_SCHEMA_MANIFEST: &str =
    include_str!("../../../../contracts/manifests/doctor-explain-schema-v0.4.json");

#[derive(Clone, Eq, PartialEq)]
pub struct ExplainSnapshotInput {
    pub rows: Vec<String>,
    pub detected_version: String,
    pub default_database: Option<String>,
}

impl fmt::Debug for ExplainSnapshotInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExplainSnapshotInput { rows: <redacted>, detected_version: <redacted>, default_database: <redacted> }")
    }
}

/// Builds one self-contained plan-only snapshot.  The original ordered cells
/// are hashed before the grammar view is constructed, so row boundaries remain
/// part of `plan_hash` even where the grammar sees a newline.
pub fn assemble_explain_snapshot(input: ExplainSnapshotInput) -> ExplainSnapshot {
    assemble_explain_snapshot_for_mode(input, ExplainMode::Costs)
}

/// Builds a mode-bound plan-only snapshot.  COSTS alone has a version-pinned
/// structural parser; LOGICAL and VERBOSE retain their bounded original lines
/// while explicitly keeping every typed structural dimension unavailable.
pub fn assemble_explain_snapshot_for_mode(
    input: ExplainSnapshotInput,
    mode: ExplainMode,
) -> ExplainSnapshot {
    let plan_hash = canonical_hash(&input.rows);
    let limits = ExplainLimits::default();
    let row_index = RowIndex::new(&input.rows, limits.max_input_bytes, limits.max_lines);
    let detected_evidence_hash =
        (!input.detected_version.is_empty()).then(|| canonical_hash(&input.detected_version));
    let database_evidence_hash = input.default_database.as_ref().map(canonical_hash);
    let selected_pack = selected_pack(&input.detected_version);
    let is_registered = selected_pack.is_some();
    let version = ExplainVersionContext {
        detected_release: is_registered.then(|| UPSTREAM_RELEASE.to_owned()),
        selected_pack: selected_pack.clone(),
        pack_match: if is_registered {
            PackMatch::ExactPack
        } else {
            PackMatch::NoPack
        },
        source_refs: vec![
            DETECTED_VERSION_REF.to_owned(),
            DEFAULT_DATABASE_REF.to_owned(),
        ],
        limitations: version_limitations(&mode, is_registered),
    };
    let receipt = explain_receipt(selected_pack.clone());
    let mut source_locators = vec![ExplainSourceLocator::Result {
        source_ref: RESULT_REF.to_owned(),
        row_count: input.rows.len().try_into().unwrap_or(u32::MAX),
        byte_length: input
            .rows
            .iter()
            .map(String::len)
            .sum::<usize>()
            .try_into()
            .unwrap_or(u32::MAX),
    }];
    source_locators.push(ExplainSourceLocator::ConnectionMetadata {
        source_ref: DETECTED_VERSION_REF.to_owned(),
        field: "detected_version".to_owned(),
        availability: if detected_evidence_hash.is_some() {
            ExplainMetadataAvailability::Available
        } else {
            ExplainMetadataAvailability::Unavailable
        },
        evidence_hash: detected_evidence_hash.clone(),
    });
    source_locators.push(ExplainSourceLocator::ConnectionMetadata {
        source_ref: DEFAULT_DATABASE_REF.to_owned(),
        field: "default_database".to_owned(),
        availability: if database_evidence_hash.is_some() {
            ExplainMetadataAvailability::Available
        } else {
            ExplainMetadataAvailability::Unavailable
        },
        evidence_hash: database_evidence_hash.clone(),
    });
    source_locators.extend(row_index.locators.clone());

    let (graph_view, plan_node_fact_sets, source_view, diagnostics, limitations) = match &mode {
        ExplainMode::Costs if is_registered && !row_index.input_limited => {
            let facts = normalize_explain_costs(&row_index.grammar, &limits);
            let graph_view = graph_view(&facts, &row_index);
            let plan_node_fact_sets = plan_node_fact_sets(&facts, &row_index, &receipt);
            let diagnostics = diagnostics(&facts, &row_index);
            let mut limitations = facts
                .limitations
                .iter()
                .map(|limitation| limitation.code.clone())
                .collect::<Vec<_>>();
            limitations.push("explain_cannot_prove_runtime_improvement".to_owned());
            let source_view = raw_text_source_view(&mode, &row_index, &limitations);
            (
                graph_view,
                plan_node_fact_sets,
                source_view,
                diagnostics,
                limitations,
            )
        }
        ExplainMode::Costs => unavailable_costs_snapshot(&row_index),
        ExplainMode::Logical | ExplainMode::Verbose => {
            unavailable_raw_text_snapshot(&mode, &row_index, is_registered)
        }
    };

    let facts_hash = explain_facts_hash_v1(&graph_view, &plan_node_fact_sets, &receipt);

    let snapshot_id = explain_snapshot_id(
        &mode,
        &plan_hash,
        &facts_hash,
        &receipt,
        detected_evidence_hash.as_deref(),
        database_evidence_hash.as_deref(),
    );
    ExplainSnapshot {
        snapshot_id,
        explain_schema_version: ExplainSchemaVersion::V0_4,
        explain_level: explain_level(&mode),
        input_origin: InputOrigin::ConnectedExplain,
        plan_hash,
        facts_hash,
        version,
        receipt,
        graph_view,
        plan_node_fact_sets,
        runtime_state: ExplainRuntimeState::Unavailable,
        source_locators,
        source_view,
        diagnostics,
        source_refs: vec![
            RESULT_REF.to_owned(),
            DETECTED_VERSION_REF.to_owned(),
            DEFAULT_DATABASE_REF.to_owned(),
        ],
        limitations,
    }
}

fn unavailable_costs_snapshot(
    rows: &RowIndex,
) -> (
    GraphView,
    Vec<ExplainNodeFactSet>,
    ExplainSourceView,
    Vec<Diagnostic>,
    Vec<String>,
) {
    let unavailable_code = if rows.input_limited {
        rows.limit_code.unwrap_or("input_bytes_exceeded")
    } else {
        "detected_version_not_registered_for_explain_costs"
    };
    let limitations = vec![
        unavailable_code.to_owned(),
        "explain_cannot_prove_runtime_improvement".to_owned(),
    ];
    let graph_view = unavailable_graph(TopologySource::ExplainCosts, &limitations);
    let source_view = if rows.input_limited {
        ExplainSourceView {
            source_kind: ExplainSourceKind::ExplainCosts,
            lines: Vec::new(),
            display_state: ExplainSourceDisplayState::SafeStructureOnly,
            truncation_state: ExplainSourceTruncationState::InputLimited,
            limitations: vec![
                "safe_structure_unavailable_for_input_budget".to_owned(),
                "raw_explain_attributes_restricted".to_owned(),
            ],
        }
    } else {
        raw_text_source_view(&ExplainMode::Costs, rows, &limitations)
    };
    let diagnostics = vec![Diagnostic {
        code: unavailable_code.to_owned(),
        stage: "explain_costs_assembly".to_owned(),
        severity: "warning".to_owned(),
        recoverability: "change_input".to_owned(),
        message_id: format!("explain.{unavailable_code}"),
        source_refs: vec![if rows.input_limited {
            RESULT_REF.to_owned()
        } else {
            DETECTED_VERSION_REF.to_owned()
        }],
        affected_ids: Vec::new(),
        details: Default::default(),
    }];
    (
        graph_view,
        Vec::new(),
        source_view,
        diagnostics,
        limitations,
    )
}

fn unavailable_raw_text_snapshot(
    mode: &ExplainMode,
    rows: &RowIndex,
    is_registered: bool,
) -> (
    GraphView,
    Vec<ExplainNodeFactSet>,
    ExplainSourceView,
    Vec<Diagnostic>,
    Vec<String>,
) {
    let mut limitations = vec![
        "structure_parser_unavailable".to_owned(),
        COSTS_RECIPES_NOT_APPLIED_FOR_RAW_TEXT_MODE.to_owned(),
    ];
    if rows.input_limited {
        limitations.push(rows.limit_code.unwrap_or("input_bytes_exceeded").to_owned());
    }
    if !is_registered {
        limitations.push(unregistered_version_limitation(mode).to_owned());
    }
    limitations.push("explain_cannot_prove_runtime_improvement".to_owned());

    let diagnostics = vec![Diagnostic {
        code: if rows.input_limited {
            rows.limit_code.unwrap_or("input_bytes_exceeded").to_owned()
        } else {
            "structure_parser_unavailable".to_owned()
        },
        stage: format!("explain_{}_assembly", mode_name(mode)),
        severity: "warning".to_owned(),
        recoverability: "change_input".to_owned(),
        message_id: if rows.input_limited {
            format!(
                "explain.{}",
                rows.limit_code.unwrap_or("input_bytes_exceeded")
            )
        } else {
            "explain.structure_parser_unavailable".to_owned()
        },
        source_refs: vec![RESULT_REF.to_owned()],
        affected_ids: Vec::new(),
        details: Default::default(),
    }];

    (
        unavailable_graph(topology_source(mode), &limitations),
        Vec::new(),
        raw_text_source_view(mode, rows, &limitations),
        diagnostics,
        limitations,
    )
}

fn explain_level(mode: &ExplainMode) -> ExplainLevel {
    match mode {
        ExplainMode::Logical => ExplainLevel::Logical,
        ExplainMode::Verbose => ExplainLevel::Verbose,
        ExplainMode::Costs => ExplainLevel::Costs,
    }
}

fn source_kind(mode: &ExplainMode) -> ExplainSourceKind {
    match mode {
        ExplainMode::Logical => ExplainSourceKind::ExplainLogical,
        ExplainMode::Verbose => ExplainSourceKind::ExplainVerbose,
        ExplainMode::Costs => ExplainSourceKind::ExplainCosts,
    }
}

fn topology_source(mode: &ExplainMode) -> TopologySource {
    match mode {
        ExplainMode::Logical => TopologySource::ExplainLogical,
        ExplainMode::Verbose => TopologySource::ExplainVerbose,
        ExplainMode::Costs => TopologySource::ExplainCosts,
    }
}

fn mode_name(mode: &ExplainMode) -> &'static str {
    match mode {
        ExplainMode::Logical => "logical",
        ExplainMode::Verbose => "verbose",
        ExplainMode::Costs => "costs",
    }
}

fn unregistered_version_limitation(mode: &ExplainMode) -> &'static str {
    match mode {
        ExplainMode::Logical => "detected_version_not_registered_for_explain_logical",
        ExplainMode::Verbose => "detected_version_not_registered_for_explain_verbose",
        ExplainMode::Costs => "detected_version_not_registered_for_explain_costs",
    }
}

fn raw_text_source_view(
    mode: &ExplainMode,
    rows: &RowIndex,
    snapshot_limitations: &[String],
) -> ExplainSourceView {
    let mut limitations = snapshot_limitations
        .iter()
        .filter(|limitation| limitation.as_str() != "explain_cannot_prove_runtime_improvement")
        .cloned()
        .collect::<Vec<_>>();
    if rows.input_limited {
        limitations.push("raw_text_unavailable_for_input_budget".to_owned());
    }
    ExplainSourceView {
        source_kind: source_kind(mode),
        lines: if rows.input_limited {
            Vec::new()
        } else {
            rows.raw_lines.clone()
        },
        display_state: ExplainSourceDisplayState::RawText,
        truncation_state: if rows.input_limited {
            ExplainSourceTruncationState::InputLimited
        } else {
            ExplainSourceTruncationState::NotTruncated
        },
        limitations,
    }
}

fn selected_pack(detected_version: &str) -> Option<SelectedVersionPack> {
    let short_commit = &UPSTREAM_COMMIT[..7];
    let registered = detected_version == UPSTREAM_RELEASE
        || detected_version == format!("{UPSTREAM_RELEASE}-{short_commit}")
        || detected_version == format!("{UPSTREAM_RELEASE}-{UPSTREAM_COMMIT}");
    registered.then(|| SelectedVersionPack {
        pack_id: PACK_ID.to_owned(),
        product_pack_version: PRODUCT_PACK_VERSION.to_owned(),
        upstream_release: UPSTREAM_RELEASE.to_owned(),
        upstream_commit: UPSTREAM_COMMIT.to_owned(),
        manifest_hash: SEMANTIC_MANIFEST_HASH.to_owned(),
        release_deltas: None,
    })
}

fn version_limitations(mode: &ExplainMode, is_registered: bool) -> Vec<String> {
    let mut limitations = vec!["default_database_fingerprint_not_comparability_proof".to_owned()];
    if !is_registered {
        limitations.push(unregistered_version_limitation(mode).to_owned());
    }
    limitations
}

fn explain_receipt(selected_pack: Option<SelectedVersionPack>) -> ExplainReceipt {
    explain_receipt_with_schema_component(
        embedded_manifest_component(EXPLAIN_SCHEMA_MANIFEST),
        selected_pack,
    )
}

fn explain_receipt_with_schema_component(
    schema: ReceiptComponent,
    selected_pack: Option<SelectedVersionPack>,
) -> ExplainReceipt {
    ExplainReceipt {
        schema,
        adapter: embedded_manifest_component(include_str!(
            "../../../../contracts/manifests/sr3514-explain-costs-adapter.json"
        )),
        exchange_recipe: embedded_manifest_component(include_str!(
            "../../../../contracts/manifests/sr3514-explain-costs-exchange-recipe.json"
        )),
        card_facts_recipe: embedded_manifest_component(include_str!(
            "../../../../contracts/manifests/sr3514-explain-costs-card-facts-recipe.json"
        )),
        version_pack: selected_pack.map(|pack| ReceiptComponent {
            id: pack.pack_id,
            version: pack.product_pack_version,
            manifest_hash: pack.manifest_hash,
        }),
    }
}

fn explain_snapshot_id(
    mode: &ExplainMode,
    plan_hash: &str,
    facts_hash: &str,
    receipt: &ExplainReceipt,
    detected_version_evidence_hash: Option<&str>,
    default_database_evidence_hash: Option<&str>,
) -> String {
    canonical_hash(&serde_json::json!({
        "mode": mode,
        "plan_hash": plan_hash,
        "facts_hash": facts_hash,
        "receipt": receipt,
        "detected_version_evidence_hash": detected_version_evidence_hash,
        "default_database_evidence_hash": default_database_evidence_hash,
    }))
}

fn embedded_manifest_component(text: &str) -> ReceiptComponent {
    let manifest: serde_json::Value =
        serde_json::from_str(text).expect("embedded Explain manifest");
    ReceiptComponent {
        id: manifest["id"]
            .as_str()
            .expect("Explain manifest ID")
            .to_owned(),
        version: manifest["version"]
            .as_str()
            .expect("Explain manifest version")
            .to_owned(),
        manifest_hash: canonical_hash(&manifest),
    }
}

fn plan_node_fact_sets(
    facts: &ExplainPlanFacts,
    rows: &RowIndex,
    receipt: &ExplainReceipt,
) -> Vec<ExplainNodeFactSet> {
    facts
        .plan_node_facts
        .iter()
        .map(|set| {
            let public_facts = set
                .facts
                .iter()
                .map(|fact| plan_node_fact(set, fact, rows))
                .collect::<Vec<_>>();
            let source_refs = rows.refs_for_spans(&set.source);
            let node_kind = facts
                .nodes
                .iter()
                .find(|node| node.node_ref == set.node_ref)
                .map(|node| &node.kind);
            let recipe = RecipeRef {
                id: receipt.card_facts_recipe.id.clone(),
                version: receipt.card_facts_recipe.version.clone(),
            };
            let slots = card_slots(node_kind, &public_facts, &source_refs, recipe);
            let reason_codes = set
                .facts
                .iter()
                .flat_map(|fact| match fact {
                    ExplainPlanNodeFact::Unavailable { reason_codes, .. } => reason_codes.clone(),
                    ExplainPlanNodeFact::Available { .. } => Vec::new(),
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            ExplainNodeFactSet {
                node_ref: set.node_ref.clone(),
                facts: public_facts,
                card_slots: slots,
                completeness: if set.partial {
                    CompletenessState::Partial
                } else {
                    CompletenessState::Complete
                },
                reason_codes,
                source_refs,
            }
        })
        .collect()
}

fn plan_node_fact(
    set: &ExplainPlanNodeFactSet,
    fact: &ExplainPlanNodeFact,
    rows: &RowIndex,
) -> ExplainNodeFact {
    let (kind, source) = match fact {
        ExplainPlanNodeFact::Available { kind, source, .. }
        | ExplainPlanNodeFact::Unavailable { kind, source, .. } => (kind, source),
    };
    let fact_kind = public_fact_kind(kind);
    let fact_ref = format!("explain_fact:{}:{}", set.node_ref, fact_kind_name(kind));
    let source_refs = rows.refs_for_spans(source);
    match fact {
        ExplainPlanNodeFact::Available { value, .. } => ExplainNodeFact::Available {
            fact_ref,
            node_ref: set.node_ref.clone(),
            fact_kind,
            evidence_kind: if matches!(kind, ExplainPlanFactKind::PlanChildCount) {
                ExplainFactEvidenceKind::VerifiedDerived
            } else {
                ExplainFactEvidenceKind::Explicit
            },
            value: public_fact_value(value),
            source_refs,
        },
        ExplainPlanNodeFact::Unavailable { reason_codes, .. } => ExplainNodeFact::Unavailable {
            fact_ref,
            node_ref: set.node_ref.clone(),
            fact_kind,
            source_refs,
            reason_codes: reason_codes.clone(),
        },
    }
}

fn public_fact_kind(kind: &ExplainPlanFactKind) -> ExplainFactKind {
    match kind {
        ExplainPlanFactKind::CardinalityEstimate => ExplainFactKind::CardinalityEstimate,
        ExplainPlanFactKind::Distribution => ExplainFactKind::Distribution,
        ExplainPlanFactKind::JoinStrategy => ExplainFactKind::JoinStrategy,
        ExplainPlanFactKind::AggregatePhase => ExplainFactKind::AggregatePhase,
        ExplainPlanFactKind::PlanChildCount => ExplainFactKind::PlanChildCount,
    }
}

fn fact_kind_name(kind: &ExplainPlanFactKind) -> &'static str {
    match kind {
        ExplainPlanFactKind::CardinalityEstimate => "cardinality_estimate",
        ExplainPlanFactKind::Distribution => "distribution",
        ExplainPlanFactKind::JoinStrategy => "join_strategy",
        ExplainPlanFactKind::AggregatePhase => "aggregate_phase",
        ExplainPlanFactKind::PlanChildCount => "plan_child_count",
    }
}

fn public_fact_value(value: &ExplainPlanFactValue) -> ExplainFactValue {
    match value {
        ExplainPlanFactValue::CardinalityEstimate(value) => {
            ExplainFactValue::CardinalityEstimate(ExplainCardinalityEstimate {
                kind: ExplainCardinalityNumberKind::Integer,
                value: value.clone(),
                scale: (),
            })
        }
        ExplainPlanFactValue::Distribution(value) => ExplainFactValue::Distribution(match value {
            ExplainPlanDistribution::Broadcast => ExplainDistribution::Broadcast,
            ExplainPlanDistribution::Gather => ExplainDistribution::Gather,
            ExplainPlanDistribution::Shuffle => ExplainDistribution::Shuffle,
        }),
        ExplainPlanFactValue::JoinStrategy(value) => ExplainFactValue::JoinStrategy(match value {
            ExplainPlanJoinStrategy::InnerBroadcast => ExplainJoinStrategy::InnerBroadcast,
            ExplainPlanJoinStrategy::InnerPartitioned => ExplainJoinStrategy::InnerPartitioned,
            ExplainPlanJoinStrategy::RightOuterBucketShuffle => {
                ExplainJoinStrategy::RightOuterBucketShuffle
            }
            ExplainPlanJoinStrategy::RightOuterPartitioned => {
                ExplainJoinStrategy::RightOuterPartitioned
            }
        }),
        ExplainPlanFactValue::AggregatePhase(value) => {
            ExplainFactValue::AggregatePhase(match value {
                ExplainPlanAggregatePhase::UpdateSerialize => {
                    ExplainAggregatePhase::UpdateSerialize
                }
                ExplainPlanAggregatePhase::UpdateFinalize => ExplainAggregatePhase::UpdateFinalize,
                ExplainPlanAggregatePhase::MergeFinalize => ExplainAggregatePhase::MergeFinalize,
            })
        }
        ExplainPlanFactValue::PlanChildCount(value) => ExplainFactValue::PlanChildCount(*value),
    }
}

fn card_slots(
    node_kind: Option<&ExplainNodeKind>,
    facts: &[ExplainNodeFact],
    source_refs: &[String],
    recipe: RecipeRef,
) -> [ExplainCardFactSlot; 3] {
    let expected = match node_kind {
        Some(ExplainNodeKind::HashJoin) => [
            Some(ExplainFactKind::CardinalityEstimate),
            Some(ExplainFactKind::JoinStrategy),
            Some(ExplainFactKind::PlanChildCount),
        ],
        Some(ExplainNodeKind::Exchange | ExplainNodeKind::MergingExchange) => [
            Some(ExplainFactKind::CardinalityEstimate),
            Some(ExplainFactKind::Distribution),
            Some(ExplainFactKind::PlanChildCount),
        ],
        Some(ExplainNodeKind::Aggregate) => [
            Some(ExplainFactKind::CardinalityEstimate),
            Some(ExplainFactKind::AggregatePhase),
            Some(ExplainFactKind::PlanChildCount),
        ],
        _ => [
            Some(ExplainFactKind::CardinalityEstimate),
            Some(ExplainFactKind::PlanChildCount),
            None,
        ],
    };
    let slots = [
        ExplainCardSlotId::Primary,
        ExplainCardSlotId::Secondary,
        ExplainCardSlotId::Tertiary,
    ];
    std::array::from_fn(|index| {
        let expected_kind = expected[index].clone();
        let matching = expected_kind
            .as_ref()
            .and_then(|kind| facts.iter().find(|fact| fact.kind() == *kind));
        match (expected_kind, matching) {
            (
                Some(expected_fact_kind),
                Some(ExplainNodeFact::Available {
                    fact_ref,
                    source_refs,
                    ..
                }),
            ) => ExplainCardFactSlot {
                slot_id: slots[index].clone(),
                expected_fact_kind: Some(expected_fact_kind),
                selection_recipe: recipe.clone(),
                state: ExplainCardFactSlotState::Selected,
                fact_ref: Some(fact_ref.clone()),
                reason_codes: Vec::new(),
                source_refs: source_refs.clone(),
            },
            (
                Some(expected_fact_kind),
                Some(ExplainNodeFact::Unavailable {
                    reason_codes,
                    source_refs,
                    ..
                }),
            ) => ExplainCardFactSlot {
                slot_id: slots[index].clone(),
                expected_fact_kind: Some(expected_fact_kind),
                selection_recipe: recipe.clone(),
                state: ExplainCardFactSlotState::Unavailable,
                fact_ref: None,
                reason_codes: reason_codes.clone(),
                source_refs: source_refs.clone(),
            },
            (Some(expected_fact_kind), None) => ExplainCardFactSlot {
                slot_id: slots[index].clone(),
                expected_fact_kind: Some(expected_fact_kind),
                selection_recipe: recipe.clone(),
                state: ExplainCardFactSlotState::Unavailable,
                fact_ref: None,
                reason_codes: vec!["fact_missing".into()],
                source_refs: source_refs.to_vec(),
            },
            (None, _) => ExplainCardFactSlot {
                slot_id: slots[index].clone(),
                expected_fact_kind: None,
                selection_recipe: recipe.clone(),
                state: ExplainCardFactSlotState::NotApplicable,
                fact_ref: None,
                reason_codes: vec!["fact_not_applicable".into()],
                source_refs: source_refs.to_vec(),
            },
        }
    })
}

fn graph_view(facts: &ExplainPlanFacts, rows: &RowIndex) -> GraphView {
    let state = graph_state(&facts.state);
    let quality = source_quality(&facts.state);
    let mut nodes = facts
        .nodes
        .iter()
        .map(|node| GraphNode {
            sql_location: None,
            sql_facts: None,
            node_id: node.node_ref.clone(),
            entity_kind: GraphEntityKind::PlanNode,
            plan_node_id: Some(node.plan_node_id.to_string()),
            fragment_id: Some(node.fragment_ref.clone()),
            pipeline_id: None,
            plan_node_type: Some(node_kind_name(&node.kind).to_owned()),
            display_name: Some(node_kind_name(&node.kind).to_owned()),
            source_ordinal: Some(node.source_ordinal),
            native_operator_refs: Vec::new(),
            subordinate_operator_refs: Vec::new(),
            metric_observation_refs: Vec::new(),
            execution_state: None,
            source_quality: quality.clone(),
            source_refs: rows.refs_for_span(node.source),
        })
        .collect::<Vec<_>>();
    let mut edges = facts
        .plan_children
        .iter()
        .map(|relation| GraphEdge {
            edge_id: relation.relation_ref.clone(),
            from: relation.parent_node_ref.clone(),
            to: relation.child_node_ref.clone(),
            relation: GraphRelation::PlanChild,
            direction_semantics: DirectionSemantics::ParentToChild,
            source: EdgeSource::Explicit,
            quality: quality.clone(),
            source_ordinal: None,
            input_role: input_role(&relation.input_role),
            derivation_recipe: None,
            restates_edge_id: None,
            evidence_refs: rows.refs_for_spans(&relation.evidence),
        })
        .chain(
            facts
                .verified_exchange_links
                .iter()
                .map(|relation| GraphEdge {
                    edge_id: relation.relation_ref.clone(),
                    from: relation.source_node_ref.clone(),
                    to: relation.target_node_ref.clone(),
                    relation: GraphRelation::ExchangeLink,
                    direction_semantics: DirectionSemantics::UpstreamToDownstream,
                    source: EdgeSource::VerifiedDerived,
                    quality: quality.clone(),
                    source_ordinal: None,
                    input_role: None,
                    derivation_recipe: Some(RecipeRef {
                        id: relation.recipe_id.clone(),
                        version: relation.recipe_version.clone(),
                    }),
                    restates_edge_id: None,
                    evidence_refs: rows.refs_for_spans(&relation.evidence),
                }),
        )
        .collect::<Vec<_>>();
    // A child relation appears when its branch is accepted, not when its parent
    // happens to occur.  Keep that fact order (R>A, A>B, R>C) and assign the
    // public ordinal after the complete explicit/derived relation sequence is
    // assembled.  Parent-line evidence would incorrectly sort R>C before A>B.
    for (ordinal, edge) in edges.iter_mut().enumerate() {
        edge.source_ordinal = Some(ordinal.try_into().unwrap_or(u32::MAX));
    }
    let connected = edges
        .iter()
        .flat_map(|edge| [&edge.from, &edge.to])
        .collect::<BTreeSet<_>>();
    let unconnected_node_ids = nodes
        .iter()
        .filter(|node| !connected.contains(&node.node_id))
        .map(|node| node.node_id.clone())
        .collect::<Vec<_>>();
    let linked_sources = facts
        .verified_exchange_links
        .iter()
        .map(|relation| relation.source_node_ref.as_str())
        .collect::<BTreeSet<_>>();
    let unresolved_relations = facts
        .exchange_candidates
        .iter()
        .filter(|candidate| {
            candidate
                .source_root_node_ref
                .as_deref()
                .is_none_or(|root| !linked_sources.contains(root))
        })
        .map(|candidate| UnresolvedRelation {
            relation_ref: candidate.candidate_ref.clone(),
            reason: "exchange_candidate_unverified".to_owned(),
            source_refs: rows.refs_for_spans(&[candidate.fragment_source, candidate.sink_source]),
        })
        .collect::<Vec<_>>();
    let mut source_refs = vec![RESULT_REF.to_owned()];
    for node in &nodes {
        extend_unique(&mut source_refs, &node.source_refs);
    }
    for edge in &edges {
        extend_unique(&mut source_refs, &edge.evidence_refs);
    }
    for unresolved in &unresolved_relations {
        extend_unique(&mut source_refs, &unresolved.source_refs);
    }
    GraphView {
        state,
        topology_source: TopologySource::ExplainCosts,
        root_node_ids: facts.result_root_node_refs.clone(),
        nodes: std::mem::take(&mut nodes),
        edges,
        unconnected_node_ids,
        unresolved_relations,
        source_refs,
        completeness: DimensionCompleteness {
            state: completeness_state(&facts.state),
            present_count: facts.nodes.len().try_into().unwrap_or(u32::MAX),
            expected_count: None,
            reasons: facts
                .limitations
                .iter()
                .map(|limitation| limitation.code.clone())
                .collect(),
        },
    }
}

fn unavailable_graph(topology_source: TopologySource, limitations: &[String]) -> GraphView {
    GraphView {
        state: GraphState::Unavailable,
        topology_source,
        root_node_ids: Vec::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        unconnected_node_ids: Vec::new(),
        unresolved_relations: Vec::new(),
        source_refs: vec![RESULT_REF.to_owned()],
        completeness: DimensionCompleteness {
            state: CompletenessState::Unavailable,
            present_count: 0,
            expected_count: None,
            reasons: limitations.to_vec(),
        },
    }
}

fn diagnostics(facts: &ExplainPlanFacts, rows: &RowIndex) -> Vec<Diagnostic> {
    facts
        .limitations
        .iter()
        .map(|limitation| Diagnostic {
            code: limitation.code.clone(),
            stage: "explain_costs_normalization".to_owned(),
            severity: "warning".to_owned(),
            recoverability: "inspect_source".to_owned(),
            message_id: format!("explain.{}", limitation.code),
            source_refs: limitation
                .source
                .map(|span| rows.refs_for_span(span))
                .unwrap_or_else(|| vec![RESULT_REF.to_owned()]),
            affected_ids: Vec::new(),
            details: Default::default(),
        })
        .collect()
}

fn graph_state(state: &ExplainStructureState) -> GraphState {
    match state {
        ExplainStructureState::Complete => GraphState::Complete,
        ExplainStructureState::Partial => GraphState::Partial,
        ExplainStructureState::Unavailable => GraphState::Unavailable,
    }
}

fn completeness_state(state: &ExplainStructureState) -> CompletenessState {
    match state {
        ExplainStructureState::Complete => CompletenessState::Complete,
        ExplainStructureState::Partial => CompletenessState::Partial,
        ExplainStructureState::Unavailable => CompletenessState::Unavailable,
    }
}

fn source_quality(state: &ExplainStructureState) -> SourceQuality {
    match state {
        ExplainStructureState::Complete => SourceQuality::Exact,
        ExplainStructureState::Partial | ExplainStructureState::Unavailable => {
            SourceQuality::Partial
        }
    }
}

fn input_role(value: &str) -> Option<InputRole> {
    match value {
        "probe" => Some(InputRole::Probe),
        "build" => Some(InputRole::Build),
        "child" => Some(InputRole::Child),
        "sink" => Some(InputRole::Sink),
        "source" => Some(InputRole::Source),
        _ => None,
    }
}

fn node_kind_name(kind: &ExplainNodeKind) -> &'static str {
    match kind {
        ExplainNodeKind::Aggregate => "AGGREGATE",
        ExplainNodeKind::Analytic => "ANALYTIC",
        ExplainNodeKind::AssertNumberOfRows => "ASSERT_NUMBER_OF_ROWS",
        ExplainNodeKind::Decode => "DECODE",
        ExplainNodeKind::Except => "EXCEPT",
        ExplainNodeKind::Exchange => "EXCHANGE",
        ExplainNodeKind::FileScan => "FILE_SCAN",
        ExplainNodeKind::HashJoin => "HASH_JOIN",
        ExplainNodeKind::Intersect => "INTERSECT",
        ExplainNodeKind::MergingExchange => "MERGING_EXCHANGE",
        ExplainNodeKind::NestLoopJoin => "NEST_LOOP_JOIN",
        ExplainNodeKind::OlapScan => "OLAP_SCAN",
        ExplainNodeKind::PartitionTopN => "PARTITION_TOP_N",
        ExplainNodeKind::Project => "PROJECT",
        ExplainNodeKind::Repeat => "REPEAT",
        ExplainNodeKind::ScanMysql => "SCAN_MYSQL",
        ExplainNodeKind::Select => "SELECT",
        ExplainNodeKind::Sort => "SORT",
        ExplainNodeKind::TableValueFunction => "TABLE_VALUE_FUNCTION",
        ExplainNodeKind::TopN => "TOP_N",
        ExplainNodeKind::Union => "UNION",
    }
}

fn extend_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        if !target.contains(value) {
            target.push(value.clone());
        }
    }
}

#[derive(Clone, Debug)]
struct RowIndex {
    grammar: String,
    locators: Vec<ExplainSourceLocator>,
    line_refs: Vec<String>,
    raw_lines: Vec<SourceLine>,
    input_limited: bool,
    limit_code: Option<&'static str>,
}

impl RowIndex {
    fn new(rows: &[String], max_input_bytes: usize, max_lines: usize) -> Self {
        let grammar_bytes = rows.iter().enumerate().fold(0usize, |total, (index, row)| {
            total
                .saturating_add(row.len())
                .saturating_add(usize::from(index > 0))
        });
        if grammar_bytes > max_input_bytes {
            return Self {
                grammar: String::new(),
                locators: Vec::new(),
                line_refs: Vec::new(),
                raw_lines: Vec::new(),
                input_limited: true,
                limit_code: Some("input_bytes_exceeded"),
            };
        }
        let mut grammar = String::new();
        let mut row_ranges = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let start = grammar.len();
            grammar.push_str(row);
            row_ranges.push((start, grammar.len()));
            // A row may already carry its own LF/CRLF terminator.  Appending
            // another separator would manufacture an extra blank grammar line
            // and make its source locator ambiguous.  An empty row without a
            // terminator still contributes the separator for its blank line.
            if index + 1 < rows.len() && !row.ends_with('\n') {
                grammar.push('\n');
            }
        }
        let mut locators = Vec::new();
        let mut line_refs = Vec::new();
        let mut raw_lines = Vec::new();
        let mut row_line_counts = vec![0u32; rows.len()];
        let mut byte_start = 0usize;
        let mut global_line_no = 1u32;
        while byte_start < grammar.len() {
            if line_refs.len() >= max_lines {
                return Self {
                    grammar: String::new(),
                    locators: Vec::new(),
                    line_refs: Vec::new(),
                    raw_lines: Vec::new(),
                    input_limited: true,
                    limit_code: Some("input_lines_exceeded"),
                };
            }
            let rest = &grammar[byte_start..];
            let inclusive_end = rest
                .find('\n')
                .map_or(grammar.len(), |offset| byte_start + offset + 1);
            let content_end = grammar[byte_start..inclusive_end]
                .strip_suffix('\n')
                .unwrap_or(&grammar[byte_start..inclusive_end]);
            let content_end = content_end.strip_suffix('\r').unwrap_or(content_end);
            let byte_end = byte_start + content_end.len();
            let row_index = owner_row(byte_start, &row_ranges).unwrap_or(0);
            row_line_counts[row_index] = row_line_counts[row_index].saturating_add(1);
            let row_start = row_ranges[row_index].0;
            let raw_start = byte_start
                .saturating_sub(row_start)
                .min(rows[row_index].len());
            let raw_end = byte_end
                .saturating_sub(row_start)
                .min(rows[row_index].len());
            let source_ref = format!(
                "explain:row:{}:line:{}",
                row_index + 1,
                row_line_counts[row_index]
            );
            locators.push(ExplainSourceLocator::RowLine {
                source_ref: source_ref.clone(),
                row_no: (row_index + 1).try_into().unwrap_or(u32::MAX),
                row_line_no: row_line_counts[row_index],
                global_line_no,
                byte_start: raw_start.try_into().unwrap_or(u32::MAX),
                byte_end: raw_end.try_into().unwrap_or(u32::MAX),
            });
            line_refs.push(source_ref.clone());
            raw_lines.push(SourceLine {
                line_no: global_line_no,
                text: content_end.to_owned(),
                source_ref,
            });
            byte_start = inclusive_end;
            global_line_no = global_line_no.saturating_add(1);
        }
        for (index, _) in rows.iter().enumerate() {
            if row_line_counts[index] == 0 {
                let source_ref = format!("explain:row:{}:line:1", index + 1);
                locators.push(ExplainSourceLocator::RowLine {
                    source_ref: source_ref.clone(),
                    row_no: (index + 1).try_into().unwrap_or(u32::MAX),
                    row_line_no: 1,
                    global_line_no,
                    byte_start: 0,
                    byte_end: 0,
                });
                line_refs.push(source_ref.clone());
                raw_lines.push(SourceLine {
                    line_no: global_line_no,
                    text: String::new(),
                    source_ref,
                });
                global_line_no = global_line_no.saturating_add(1);
            }
        }
        Self {
            grammar,
            locators,
            line_refs,
            raw_lines,
            input_limited: false,
            limit_code: None,
        }
    }

    fn ref_for_span(&self, span: ExplainSourceSpan) -> String {
        self.line_refs
            .get(span.line_start.saturating_sub(1) as usize)
            .cloned()
            .unwrap_or_else(|| RESULT_REF.to_owned())
    }

    fn refs_for_span(&self, span: ExplainSourceSpan) -> Vec<String> {
        vec![self.ref_for_span(span)]
    }

    fn refs_for_spans(&self, spans: &[ExplainSourceSpan]) -> Vec<String> {
        let mut refs = Vec::new();
        for span in spans {
            extend_unique(&mut refs, &self.refs_for_span(*span));
        }
        refs
    }
}

fn owner_row(byte_start: usize, row_ranges: &[(usize, usize)]) -> Option<usize> {
    row_ranges
        .iter()
        .enumerate()
        // A separator written for a blank row is at the blank row's zero-width
        // range.  Choose that exact owner before looking at adjacent rows.
        .find(|(_, (start, end))| start == end && byte_start == *start)
        .or_else(|| {
            row_ranges
                .iter()
                .enumerate()
                .find(|(_, (start, end))| byte_start >= *start && byte_start < *end)
        })
        .map(|(index, _)| index)
        .or_else(|| {
            row_ranges
                .iter()
                .enumerate()
                .rev()
                .find(|(_, (start, _))| byte_start >= *start)
                .map(|(index, _)| index)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExplainBudget, ExplainPlanChild, ExplainPlanNode, ExplainSafeStructure};
    use sha2::{Digest, Sha256};

    #[test]
    fn registered_version_accepts_only_registered_spellings() {
        assert!(selected_pack("3.5.14").is_some());
        assert!(selected_pack("3.5.14-23a56ec").is_some());
        assert!(selected_pack("3.5.15").is_none());
    }

    #[test]
    fn schema_manifest_binds_complete_generated_schema_and_changes_snapshot_receipt_identity() {
        let manifest: serde_json::Value =
            serde_json::from_str(EXPLAIN_SCHEMA_MANIFEST).expect("Explain schema manifest");
        let schema_text = include_str!("../../../../contracts/schema/ExplainSnapshot.schema.json");
        let mut schema_hasher = Sha256::new();
        schema_hasher.update(schema_text.as_bytes());
        let schema_hash = format!("{:x}", schema_hasher.finalize());
        let schema: serde_json::Value =
            serde_json::from_str(schema_text).expect("generated ExplainSnapshot schema");
        assert_eq!(manifest["schema_id"], "ExplainSnapshot");
        assert_eq!(manifest["schema_content_sha256"], schema_hash);
        assert_eq!(
            manifest["schema_definitions_canonical_sha256"],
            canonical_hash(
                schema
                    .get("definitions")
                    .expect("ExplainSnapshot transitive definitions")
            )
        );

        let rows = vec![
            "PLAN FRAGMENT 0(F0)".to_owned(),
            "  RESULT SINK".to_owned(),
            "  0:Project".to_owned(),
        ];
        let detected_version = "3.5.14-23a56ec".to_owned();
        let default_database = "secret_database".to_owned();
        let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
            rows: rows.clone(),
            detected_version: detected_version.clone(),
            default_database: Some(default_database.clone()),
        });
        let mut changed_manifest = manifest;
        changed_manifest["schema_content_sha256"] = serde_json::Value::String("f".repeat(64));
        let changed_schema_component = embedded_manifest_component(
            &serde_json::to_string(&changed_manifest).expect("changed schema manifest"),
        );
        let changed_receipt = explain_receipt_with_schema_component(
            changed_schema_component,
            selected_pack(&detected_version),
        );
        let detected_evidence_hash = canonical_hash(&detected_version);
        let database_evidence_hash = canonical_hash(&default_database);
        let changed_snapshot_id = explain_snapshot_id(
            &ExplainMode::Costs,
            &snapshot.plan_hash,
            &snapshot.facts_hash,
            &changed_receipt,
            Some(&detected_evidence_hash),
            Some(&database_evidence_hash),
        );
        assert_eq!(snapshot.plan_hash, canonical_hash(&rows));
        assert_ne!(
            snapshot.receipt.schema.manifest_hash,
            changed_receipt.schema.manifest_hash
        );
        assert_ne!(snapshot.snapshot_id, changed_snapshot_id);
    }

    #[test]
    fn row_index_preserves_empty_and_embedded_line_locations() {
        let index = RowIndex::new(
            &[
                "PLAN FRAGMENT 0(F0)\r\n".to_owned(),
                "".to_owned(),
                "  0:Project".to_owned(),
            ],
            usize::MAX,
            usize::MAX,
        );
        assert_eq!(index.line_refs.len(), 3);
        assert!(matches!(
            index.locators[0],
            ExplainSourceLocator::RowLine {
                row_no: 1,
                row_line_no: 1,
                global_line_no: 1,
                byte_start: 0,
                byte_end: 19,
                ..
            }
        ));
        assert!(matches!(
            index.locators[1],
            ExplainSourceLocator::RowLine {
                row_no: 2,
                row_line_no: 1,
                global_line_no: 2,
                byte_start: 0,
                byte_end: 0,
                ..
            }
        ));
        assert!(matches!(
            index.locators[2],
            ExplainSourceLocator::RowLine {
                row_no: 3,
                row_line_no: 1,
                global_line_no: 3,
                byte_start: 0,
                byte_end: 11,
                ..
            }
        ));
    }

    #[test]
    fn row_index_refuses_line_budget_before_allocating_all_locators() {
        let index = RowIndex::new(&["x\nx\nx".to_owned()], usize::MAX, 2);
        assert!(index.input_limited);
        assert_eq!(index.limit_code, Some("input_lines_exceeded"));
        assert!(index.locators.is_empty());
    }

    #[test]
    fn plan_child_edges_keep_branch_acceptance_order_not_parent_line_order() {
        let index = RowIndex::new(&["r\na\nb\nc".to_owned()], usize::MAX, usize::MAX);
        let span = |line_start| ExplainSourceSpan {
            line_start,
            line_end: line_start,
            byte_start: 0,
            byte_end: 0,
        };
        let node = |node_ref: &str, plan_node_id, source_ordinal, source| ExplainPlanNode {
            node_ref: node_ref.to_owned(),
            fragment_ref: "F0".to_owned(),
            plan_node_id,
            kind: ExplainNodeKind::Project,
            source_ordinal,
            source,
        };
        let facts = ExplainPlanFacts {
            state: ExplainStructureState::Complete,
            fragments: Vec::new(),
            result_root_node_refs: vec!["R".to_owned()],
            nodes: vec![
                node("R", 0, 0, span(1)),
                node("A", 1, 1, span(2)),
                node("B", 2, 2, span(3)),
                node("C", 3, 3, span(4)),
            ],
            plan_children: vec![
                ExplainPlanChild {
                    relation_ref: "R>A".to_owned(),
                    parent_node_ref: "R".to_owned(),
                    child_node_ref: "A".to_owned(),
                    input_role: "child".to_owned(),
                    evidence: vec![span(1)],
                },
                ExplainPlanChild {
                    relation_ref: "A>B".to_owned(),
                    parent_node_ref: "A".to_owned(),
                    child_node_ref: "B".to_owned(),
                    input_role: "child".to_owned(),
                    evidence: vec![span(2)],
                },
                ExplainPlanChild {
                    relation_ref: "R>C".to_owned(),
                    parent_node_ref: "R".to_owned(),
                    child_node_ref: "C".to_owned(),
                    input_role: "child".to_owned(),
                    evidence: vec![span(1)],
                },
            ],
            exchange_candidates: Vec::new(),
            verified_exchange_links: Vec::new(),
            plan_node_facts: Vec::new(),
            limitations: Vec::new(),
            budget: ExplainBudget {
                input_bytes: 0,
                input_lines: 0,
                parsed_nodes: 4,
                parsed_relations: 3,
            },
            safe_structure: ExplainSafeStructure {
                fragments: Vec::new(),
                nodes: Vec::new(),
                relation_refs: Vec::new(),
                limitations: Vec::new(),
            },
        };

        let graph = graph_view(&facts, &index);
        assert_eq!(
            graph
                .edges
                .iter()
                .map(|edge| edge.edge_id.as_str())
                .collect::<Vec<_>>(),
            vec!["R>A", "A>B", "R>C"]
        );
        assert_eq!(
            graph
                .edges
                .iter()
                .map(|edge| edge.source_ordinal)
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(2)]
        );
    }

    #[test]
    fn facts_hash_covers_every_frozen_semantic_graph_field() {
        let rows = include_str!(
            "../../../../fixtures/explain/starrocks-3.5.14/capture-047.sanitized.costs.txt"
        )
        .split_terminator('\n')
        .map(str::to_owned)
        .collect();
        let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
            rows,
            detected_version: "3.5.14-23a56ec".to_owned(),
            default_database: None,
        });
        let base = explain_facts_hash_v1(
            &snapshot.graph_view,
            &snapshot.plan_node_fact_sets,
            &snapshot.receipt,
        );
        let changed = |graph: &GraphView| {
            explain_facts_hash_v1(graph, &snapshot.plan_node_fact_sets, &snapshot.receipt)
        };

        let mut graph = snapshot.graph_view.clone();
        graph.nodes[0].display_name = Some("changed-safe-name".to_owned());
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph.nodes[0].source_quality = match graph.nodes[0].source_quality {
            SourceQuality::Exact => SourceQuality::Partial,
            SourceQuality::Partial => SourceQuality::Exact,
        };
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph.edges[0].quality = match graph.edges[0].quality {
            SourceQuality::Exact => SourceQuality::Partial,
            SourceQuality::Partial => SourceQuality::Exact,
        };
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph.edges[0].derivation_recipe = Some(RecipeRef {
            id: "changed.recipe".to_owned(),
            version: "1".to_owned(),
        });
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph.completeness.state = match graph.completeness.state {
            CompletenessState::Complete => CompletenessState::Partial,
            CompletenessState::Partial | CompletenessState::Unavailable => {
                CompletenessState::Complete
            }
        };
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph
            .completeness
            .reasons
            .push("changed_completeness_reason".to_owned());
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph.unconnected_node_ids.push("changed-node".to_owned());
        assert_ne!(base, changed(&graph));

        let mut graph = snapshot.graph_view.clone();
        graph.unresolved_relations.push(UnresolvedRelation {
            relation_ref: "changed-relation".to_owned(),
            reason: "changed-reason".to_owned(),
            source_refs: Vec::new(),
        });
        assert_ne!(base, changed(&graph));
    }

    #[test]
    fn facts_hash_excludes_all_provenance_references() {
        let rows = include_str!(
            "../../../../fixtures/explain/starrocks-3.5.14/capture-047.sanitized.costs.txt"
        )
        .split_terminator('\n')
        .map(str::to_owned)
        .collect();
        let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
            rows,
            detected_version: "3.5.14-23a56ec".to_owned(),
            default_database: None,
        });
        let base = explain_facts_hash_v1(
            &snapshot.graph_view,
            &snapshot.plan_node_fact_sets,
            &snapshot.receipt,
        );
        let mut graph = snapshot.graph_view.clone();
        graph.source_refs.push("changed:graph".to_owned());
        graph.nodes[0].source_refs.push("changed:node".to_owned());
        graph.edges[0].evidence_refs.push("changed:edge".to_owned());

        let mut fact_sets = snapshot.plan_node_fact_sets.clone();
        fact_sets[0].source_refs.push("changed:set".to_owned());
        fact_sets[0].card_slots[0]
            .source_refs
            .push("changed:slot".to_owned());
        match &mut fact_sets[0].facts[0] {
            ExplainNodeFact::Available { source_refs, .. }
            | ExplainNodeFact::Unavailable { source_refs, .. } => {
                source_refs.push("changed:fact".to_owned());
            }
        }
        assert_eq!(
            base,
            explain_facts_hash_v1(&graph, &fact_sets, &snapshot.receipt)
        );

        let mut unresolved = snapshot.graph_view.clone();
        unresolved.unresolved_relations.push(UnresolvedRelation {
            relation_ref: "semantic-relation".to_owned(),
            reason: "semantic-reason".to_owned(),
            source_refs: Vec::new(),
        });
        let unresolved_base = explain_facts_hash_v1(
            &unresolved,
            &snapshot.plan_node_fact_sets,
            &snapshot.receipt,
        );
        unresolved
            .unresolved_relations
            .last_mut()
            .expect("relation")
            .source_refs
            .push("changed:unresolved".to_owned());
        assert_eq!(
            unresolved_base,
            explain_facts_hash_v1(
                &unresolved,
                &snapshot.plan_node_fact_sets,
                &snapshot.receipt,
            )
        );
    }
}
