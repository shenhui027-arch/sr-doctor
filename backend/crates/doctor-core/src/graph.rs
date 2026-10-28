use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    CompletenessState, DimensionCompleteness, EdgeSource, GraphEdge, GraphNode, GraphRelation,
    GraphState, GraphView, OrderedOperatorRef, SourceQuality, TopologySource, UnresolvedRelation,
};

use crate::{GraphContractError, graph_view};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplicitGraphEvidence {
    pub topology_source: TopologySource,
    pub root_node_ids: Vec<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub unresolved_relations: Vec<UnresolvedRelation>,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphEvidence {
    Explicit(ExplicitGraphEvidence),
    /// The source explicitly declares a closed topology and each accepted
    /// relation is source-backed. This is deliberately distinct from the
    /// default explicit case: a list of nodes alone is not proof of closure.
    ExplicitComplete(ExplicitGraphEvidence),
    /// Text line order is not topology; keep any text-only entities in IR, not GraphView.
    Unavailable {
        reason: String,
        source_refs: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphResolutionError {
    MissingEvidence,
    EmptyTopologyEvidence,
    InvalidTopologySource,
    DuplicateNodeId,
    DuplicateRootId,
    DuplicateEdgeId,
    DuplicateSourceOrdinal,
    SourceOrderViolation,
    MissingNodeRef,
    MissingSourceOrdinal,
    InvalidRootRef,
    InvalidEdgeEndpoint,
    SelfReferentialEdge,
    InvalidUnresolvedRelation,
    ExplicitEvidenceContainsDerivedEdge,
    InvalidRestatedEdge,
    GraphContract(GraphContractError),
    CollectionTooLarge,
}

/// Resolves only explicit topology. It never turns IDs or Text line order into
/// a relation, and no production path derives an edge.
pub fn resolve_graph(evidence: GraphEvidence) -> Result<GraphView, GraphResolutionError> {
    match evidence {
        GraphEvidence::Explicit(input) => resolve_explicit(input, false),
        GraphEvidence::ExplicitComplete(input) => resolve_explicit(input, true),
        GraphEvidence::Unavailable {
            reason,
            source_refs,
        } => {
            validate_refs(&source_refs)?;
            if reason.trim().is_empty() {
                return Err(GraphResolutionError::InvalidUnresolvedRelation);
            }
            base_graph(
                GraphState::Unavailable,
                TopologySource::None,
                vec![],
                vec![],
                vec![],
                vec![UnresolvedRelation {
                    relation_ref: "topology".into(),
                    reason,
                    source_refs: source_refs.clone(),
                }],
                source_refs,
            )
        }
    }
}

fn resolve_explicit(
    input: ExplicitGraphEvidence,
    declared_complete: bool,
) -> Result<GraphView, GraphResolutionError> {
    validate_refs(&input.source_refs)?;
    if input.nodes.is_empty() {
        return Err(GraphResolutionError::EmptyTopologyEvidence);
    }
    if !matches!(
        input.topology_source,
        TopologySource::ExecutionTopology | TopologySource::EmbeddedPlan
    ) {
        return Err(GraphResolutionError::InvalidTopologySource);
    }
    let ids = validate_nodes(&input.nodes)?;
    validate_roots(&input.root_node_ids, &ids)?;
    validate_unresolved(&input.unresolved_relations)?;
    let mut edge_ids = BTreeSet::new();
    let mut previous_ordinal = None;
    for edge in &input.edges {
        if edge.edge_id.is_empty() || !edge_ids.insert(edge.edge_id.as_str()) {
            return Err(GraphResolutionError::DuplicateEdgeId);
        }
        if edge.source != EdgeSource::Explicit || edge.derivation_recipe.is_some() {
            return Err(GraphResolutionError::ExplicitEvidenceContainsDerivedEdge);
        }
        if edge.from == edge.to {
            return Err(GraphResolutionError::SelfReferentialEdge);
        }
        if !ids.contains(&edge.from) || !ids.contains(&edge.to) {
            return Err(GraphResolutionError::InvalidEdgeEndpoint);
        }
        validate_refs(&edge.evidence_refs)?;
        let ordinal = edge
            .source_ordinal
            .ok_or(GraphResolutionError::MissingSourceOrdinal)?;
        if previous_ordinal.is_some_and(|previous| ordinal <= previous) {
            return Err(GraphResolutionError::SourceOrderViolation);
        }
        previous_ordinal = Some(ordinal);
    }
    validate_restated_edges(&input.edges)?;
    let state = if declared_complete
        && input.unresolved_relations.is_empty()
        && input
            .nodes
            .iter()
            .all(|node| node.source_quality == SourceQuality::Exact)
        && input
            .edges
            .iter()
            .all(|edge| edge.quality == SourceQuality::Exact)
    {
        GraphState::Complete
    } else {
        GraphState::Partial
    };
    let mut graph = base_graph(
        state,
        input.topology_source,
        input.root_node_ids,
        input.nodes,
        input.edges,
        input.unresolved_relations,
        input.source_refs,
    )?;
    graph.unconnected_node_ids = unconnected_ids(&graph.nodes, &graph.edges);
    Ok(graph)
}

fn base_graph(
    state: GraphState,
    topology_source: TopologySource,
    roots: Vec<String>,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    unresolved: Vec<UnresolvedRelation>,
    source_refs: Vec<String>,
) -> Result<GraphView, GraphResolutionError> {
    let present_count =
        u32::try_from(nodes.len()).map_err(|_| GraphResolutionError::CollectionTooLarge)?;
    let mut graph = graph_view(
        state.clone(),
        topology_source,
        roots,
        nodes,
        edges,
        unresolved,
    )
    .map_err(GraphResolutionError::GraphContract)?;
    graph.source_refs = source_refs;
    graph.completeness = DimensionCompleteness {
        state: match state {
            GraphState::Unavailable => CompletenessState::Unavailable,
            GraphState::Complete => CompletenessState::Complete,
            GraphState::Partial => CompletenessState::Partial,
        },
        present_count,
        expected_count: None,
        reasons: Vec::new(),
    };
    Ok(graph)
}

fn validate_nodes(nodes: &[GraphNode]) -> Result<BTreeSet<String>, GraphResolutionError> {
    let mut ids = BTreeSet::new();
    let mut previous_ordinal = None;
    for node in nodes {
        let ordinal = node
            .source_ordinal
            .ok_or(GraphResolutionError::MissingSourceOrdinal)?;
        if node.node_id.is_empty() {
            return Err(GraphResolutionError::MissingNodeRef);
        }
        if previous_ordinal.is_some_and(|previous| ordinal <= previous) {
            return Err(GraphResolutionError::SourceOrderViolation);
        }
        previous_ordinal = Some(ordinal);
        validate_refs(&node.source_refs).map_err(|_| GraphResolutionError::MissingNodeRef)?;
        validate_operator_refs(&node.native_operator_refs)?;
        validate_operator_refs(&node.subordinate_operator_refs)?;
        if !ids.insert(node.node_id.clone()) {
            return Err(GraphResolutionError::DuplicateNodeId);
        }
    }
    Ok(ids)
}

fn validate_operator_refs(operators: &[OrderedOperatorRef]) -> Result<(), GraphResolutionError> {
    let mut ordinals = BTreeSet::new();
    let mut previous_ordinal = None;
    for operator in operators {
        if operator.operator_ref.is_empty() {
            return Err(GraphResolutionError::MissingNodeRef);
        }
        validate_refs(&operator.source_refs)?;
        if !ordinals.insert(operator.source_ordinal) {
            return Err(GraphResolutionError::DuplicateSourceOrdinal);
        }
        if previous_ordinal.is_some_and(|previous| operator.source_ordinal <= previous) {
            return Err(GraphResolutionError::SourceOrderViolation);
        }
        previous_ordinal = Some(operator.source_ordinal);
    }
    Ok(())
}

fn validate_roots(roots: &[String], ids: &BTreeSet<String>) -> Result<(), GraphResolutionError> {
    if !ids.is_empty() && roots.is_empty() {
        return Err(GraphResolutionError::InvalidRootRef);
    }
    let mut unique = BTreeSet::new();
    if roots.iter().any(|root| !unique.insert(root.as_str())) {
        return Err(GraphResolutionError::DuplicateRootId);
    }
    if roots
        .iter()
        .any(|root| root.is_empty() || !ids.contains(root))
    {
        return Err(GraphResolutionError::InvalidRootRef);
    }
    Ok(())
}

/// A `restates_edge_id` may only mark a fragment_sink edge as the reverse of an
/// existing plan_child edge; anything else would hide a real relation.
fn validate_restated_edges(edges: &[GraphEdge]) -> Result<(), GraphResolutionError> {
    let plan_children = edges
        .iter()
        .filter(|edge| edge.relation == GraphRelation::PlanChild)
        .map(|edge| (edge.edge_id.as_str(), edge))
        .collect::<BTreeMap<_, _>>();
    for edge in edges {
        let Some(restated) = edge.restates_edge_id.as_deref() else {
            continue;
        };
        let valid = edge.relation == GraphRelation::FragmentSink
            && plan_children
                .get(restated)
                .is_some_and(|child| child.from == edge.to && child.to == edge.from);
        if !valid {
            return Err(GraphResolutionError::InvalidRestatedEdge);
        }
    }
    Ok(())
}

fn validate_unresolved(relations: &[UnresolvedRelation]) -> Result<(), GraphResolutionError> {
    for relation in relations {
        if relation.relation_ref.is_empty()
            || relation.reason.trim().is_empty()
            || validate_refs(&relation.source_refs).is_err()
        {
            return Err(GraphResolutionError::InvalidUnresolvedRelation);
        }
    }
    Ok(())
}

fn validate_refs(refs: &[String]) -> Result<(), GraphResolutionError> {
    if refs.is_empty() || refs.iter().any(|reference| reference.trim().is_empty()) {
        Err(GraphResolutionError::MissingEvidence)
    } else {
        Ok(())
    }
}

/// Nodes that no edge touches: one pass over the edges, then one over the nodes.
fn unconnected_ids(nodes: &[GraphNode], edges: &[GraphEdge]) -> Vec<String> {
    let connected = edges
        .iter()
        .flat_map(|edge| [edge.from.as_str(), edge.to.as_str()])
        .collect::<BTreeSet<_>>();
    nodes
        .iter()
        .filter(|node| !connected.contains(node.node_id.as_str()))
        .map(|node| node.node_id.clone())
        .collect()
}
