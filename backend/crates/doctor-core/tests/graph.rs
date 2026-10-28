use doctor_contracts::{
    Compatibility, DirectionSemantics, EdgeSource, GraphEdge, GraphEntityKind, GraphNode,
    GraphRelation, GraphState, MergeState, OrderedOperatorRef, PackMatch, ProfileContext,
    ProfileLevel, QueryState, RecipeRef, SnapshotKind, SourceQuality, SourceRole, SupportStatus,
    TopologySource, VersionContext, VersionSource,
};
use doctor_core::{
    ExplicitGraphEvidence, GraphEvidence, GraphResolutionError, facts_hash, portable_facts,
    resolve_graph,
};

fn node(id: &str, ordinal: u32, source: &str) -> GraphNode {
    GraphNode {
        sql_location: None,
        sql_facts: None,
        node_id: id.into(),
        entity_kind: GraphEntityKind::PlanNode,
        plan_node_id: Some(id.into()),
        fragment_id: None,
        pipeline_id: None,
        plan_node_type: None,
        display_name: None,
        source_ordinal: Some(ordinal),
        native_operator_refs: vec![],
        subordinate_operator_refs: vec![],
        metric_observation_refs: vec![],
        execution_state: None,
        source_quality: SourceQuality::Exact,
        source_refs: vec![source.into()],
    }
}

fn explicit_edge(id: &str, from: &str, to: &str, ordinal: u32, source: &str) -> GraphEdge {
    GraphEdge {
        edge_id: id.into(),
        from: from.into(),
        to: to.into(),
        relation: GraphRelation::PlanChild,
        direction_semantics: DirectionSemantics::ParentToChild,
        source: EdgeSource::Explicit,
        quality: SourceQuality::Exact,
        source_ordinal: Some(ordinal),
        input_role: None,
        derivation_recipe: None,
        restates_edge_id: None,
        evidence_refs: vec![source.into()],
    }
}

fn version_context() -> VersionContext {
    VersionContext {
        reported_version: None,
        user_declared_version: None,
        effective_version: Some("3.5.14".into()),
        version_source: VersionSource::Detected,
        selected_pack: None,
        pack_match: PackMatch::ExactPack,
        compatibility: Compatibility::Partial,
        support_status: SupportStatus::ParseOnly,
        detection_evidence: vec![],
        compatibility_checks: vec![],
        issues: vec![],
    }
}

fn profile_context() -> ProfileContext {
    ProfileContext {
        profile_level: ProfileLevel::Level1,
        query_state: QueryState::Finished,
        snapshot_kind: SnapshotKind::Final,
        merge_state: MergeState::Merged,
        evidence_refs: vec!["json:".into()],
        limitations: vec![],
    }
}

#[test]
fn graph_goldens_keep_explicit_order_and_leave_text_unavailable() {
    let mut join = node("join", 0, "json:/nodes/0");
    join.native_operator_refs = vec![
        OrderedOperatorRef {
            operator_ref: "native-z".into(),
            source_ordinal: 0,
            source_role: SourceRole::Native,
            source_refs: vec!["json:/operators/0".into()],
        },
        OrderedOperatorRef {
            operator_ref: "native-a".into(),
            source_ordinal: 1,
            source_role: SourceRole::Native,
            source_refs: vec!["json:/operators/1".into()],
        },
    ];
    join.subordinate_operator_refs = vec![
        OrderedOperatorRef {
            operator_ref: "sub-z".into(),
            source_ordinal: 0,
            source_role: SourceRole::Subordinate,
            source_refs: vec!["json:/subordinates/0".into()],
        },
        OrderedOperatorRef {
            operator_ref: "sub-a".into(),
            source_ordinal: 1,
            source_role: SourceRole::Subordinate,
            source_refs: vec!["json:/subordinates/1".into()],
        },
    ];
    let explicit = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::EmbeddedPlan,
        root_node_ids: vec!["join".into()],
        nodes: vec![
            join,
            node("child-z", 1, "json:/nodes/1"),
            node("child-a", 2, "json:/nodes/2"),
        ],
        edges: vec![
            explicit_edge("edge-z", "join", "child-z", 0, "json:/children/0"),
            explicit_edge("edge-a", "join", "child-a", 1, "json:/children/1"),
        ],
        unresolved_relations: vec![],
        source_refs: vec!["json:/plan".into()],
    }))
    .expect("explicit graph");
    assert_eq!(explicit.state, GraphState::Partial);
    assert_eq!(
        explicit
            .nodes
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<Vec<_>>(),
        ["join", "child-z", "child-a"]
    );
    assert_eq!(
        explicit
            .edges
            .iter()
            .map(|edge| edge.edge_id.as_str())
            .collect::<Vec<_>>(),
        ["edge-z", "edge-a"]
    );
    assert_eq!(
        explicit
            .edges
            .iter()
            .map(|edge| edge.source_ordinal)
            .collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
    assert!(
        explicit
            .edges
            .iter()
            .all(|edge| edge.source == EdgeSource::Explicit)
    );
    assert_eq!(
        explicit.nodes[0]
            .native_operator_refs
            .iter()
            .map(|operator| operator.operator_ref.as_str())
            .collect::<Vec<_>>(),
        ["native-z", "native-a"]
    );
    assert_eq!(
        explicit.nodes[0]
            .subordinate_operator_refs
            .iter()
            .map(|operator| operator.operator_ref.as_str())
            .collect::<Vec<_>>(),
        ["sub-z", "sub-a"]
    );

    let unavailable = resolve_graph(GraphEvidence::Unavailable {
        reason: "topology unavailable".into(),
        source_refs: vec!["text:1".into()],
    })
    .expect("text order never becomes topology");
    assert_eq!(unavailable.state, GraphState::Unavailable);
    assert!(unavailable.nodes.is_empty());
    assert!(unavailable.root_node_ids.is_empty());
    assert!(unavailable.edges.is_empty());
    assert_eq!(
        unavailable.unresolved_relations[0].reason,
        "topology unavailable"
    );
}

#[test]
fn only_closed_explicit_evidence_can_mark_a_graph_complete() {
    let complete = resolve_graph(GraphEvidence::ExplicitComplete(ExplicitGraphEvidence {
        topology_source: TopologySource::ExecutionTopology,
        root_node_ids: vec!["root".into()],
        nodes: vec![
            node("root", 0, "json:/topology"),
            node("child", 1, "json:/topology"),
        ],
        edges: vec![explicit_edge("child", "root", "child", 0, "json:/topology")],
        unresolved_relations: vec![],
        source_refs: vec!["json:/topology".into()],
    }))
    .expect("closed explicit graph");
    assert_eq!(complete.state, GraphState::Complete);
    assert_eq!(
        complete.completeness.state,
        doctor_contracts::CompletenessState::Complete
    );

    let partial = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::ExecutionTopology,
        root_node_ids: vec!["root".into()],
        nodes: vec![
            node("root", 0, "json:/topology"),
            node("child", 1, "json:/topology"),
        ],
        edges: vec![explicit_edge("child", "root", "child", 0, "json:/topology")],
        unresolved_relations: vec![],
        source_refs: vec!["json:/topology".into()],
    }))
    .expect("open explicit graph stays partial");
    assert_eq!(partial.state, GraphState::Partial);
}

#[test]
fn graph_rejects_unproven_derived_edges_and_invalid_explicit_provenance() {
    let empty_explicit = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::EmbeddedPlan,
        root_node_ids: vec![],
        nodes: vec![],
        edges: vec![],
        unresolved_relations: vec![],
        source_refs: vec!["json:/plan".into()],
    }));
    assert_eq!(
        empty_explicit,
        Err(GraphResolutionError::EmptyTopologyEvidence)
    );

    let derived_without_recipe = GraphEdge {
        source: EdgeSource::VerifiedDerived,
        ..explicit_edge("bad", "left", "right", 0, "json:/bad")
    };
    let result = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::EmbeddedPlan,
        root_node_ids: vec!["left".into()],
        nodes: vec![
            node("left", 0, "json:/left"),
            node("right", 1, "json:/right"),
        ],
        edges: vec![derived_without_recipe],
        unresolved_relations: vec![],
        source_refs: vec!["json:/plan".into()],
    }));
    assert_eq!(
        result,
        Err(GraphResolutionError::ExplicitEvidenceContainsDerivedEdge)
    );

    let self_loop = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::EmbeddedPlan,
        root_node_ids: vec!["left".into()],
        nodes: vec![node("left", 0, "json:/left")],
        edges: vec![explicit_edge("self", "left", "left", 0, "json:/self")],
        unresolved_relations: vec![],
        source_refs: vec!["json:/plan".into()],
    }));
    assert_eq!(self_loop, Err(GraphResolutionError::SelfReferentialEdge));

    let bad_root = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::EmbeddedPlan,
        root_node_ids: vec!["missing".into()],
        nodes: vec![node("left", 0, "json:/left")],
        edges: vec![],
        unresolved_relations: vec![],
        source_refs: vec!["json:/plan".into()],
    }));
    assert_eq!(bad_root, Err(GraphResolutionError::InvalidRootRef));

    let missing_root = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::EmbeddedPlan,
        root_node_ids: vec![],
        nodes: vec![node("left", 0, "json:/left")],
        edges: vec![],
        unresolved_relations: vec![],
        source_refs: vec!["json:/plan".into()],
    }));
    assert_eq!(missing_root, Err(GraphResolutionError::InvalidRootRef));
}

#[test]
fn fragment_sink_order_and_derived_recipe_are_hash_significant() {
    let mut sink = explicit_edge("sink", "fragment", "sink-node", 0, "json:/sink_ids/0");
    sink.relation = GraphRelation::FragmentSink;
    sink.direction_semantics = DirectionSemantics::UpstreamToDownstream;
    let fragment_graph = resolve_graph(GraphEvidence::Explicit(ExplicitGraphEvidence {
        topology_source: TopologySource::ExecutionTopology,
        root_node_ids: vec!["fragment".into()],
        nodes: vec![
            node("fragment", 0, "json:/fragments/0"),
            node("sink-node", 1, "json:/sinks/0"),
        ],
        edges: vec![sink],
        unresolved_relations: vec![],
        source_refs: vec!["json:/fragments/0".into()],
    }))
    .expect("source ordered fragment sink");
    assert_eq!(
        fragment_graph.edges[0].relation,
        GraphRelation::FragmentSink
    );
    assert_eq!(fragment_graph.edges[0].source_ordinal, Some(0));

    let base = portable_facts(
        &version_context(),
        profile_context(),
        vec![],
        &[],
        &fragment_graph,
        &[],
        &[],
    );
    // No production path derives an edge, but a recipe on an edge stays hash-significant.
    let mut changed_recipe = fragment_graph.clone();
    changed_recipe.edges[0].derivation_recipe = Some(RecipeRef {
        id: "test.recipe".into(),
        version: "1".into(),
    });
    let changed = portable_facts(
        &version_context(),
        profile_context(),
        vec![],
        &[],
        &changed_recipe,
        &[],
        &[],
    );
    assert_ne!(facts_hash(&base), facts_hash(&changed));
}

#[test]
fn graph_fixture_cases_are_asserted_field_by_field() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_graph_contract_cases.json"
    ))
    .expect("frozen graph cases");
    let find = |id| {
        cases
            .as_array()
            .expect("array")
            .iter()
            .find(|case| case["case_id"] == id)
            .expect("case")
    };
    let explicit = find("graph_explicit");
    assert_eq!(explicit["state"], "partial");
    assert_eq!(explicit["edge_source"], "explicit");
    assert_eq!(explicit["source_ordinal"], serde_json::json!([0, 1]));
    assert!(
        cases
            .as_array()
            .expect("array")
            .iter()
            .all(|case| case["edge_source"] != "verified_derived"),
        "no production path derives an edge"
    );
    let unavailable = find("graph_unavailable");
    assert_eq!(unavailable["state"], "unavailable");
    assert_eq!(unavailable["edges"], serde_json::json!([]));
    assert_eq!(unavailable["unresolved_reason"], "topology unavailable");
}
