//! Bounded StarRocks 3.5.14 `EXPLAIN COSTS` structure facts.
//!
//! This is intentionally a private-stage normalizer.  It does not construct a
//! public `GraphView`, attach runtime metrics, or reinterpret arbitrary text as
//! topology.  EX-02 owns the version/receipt/source-schema seam.

use std::collections::{BTreeMap, BTreeSet};

const BASE_PREFIX: &str = "  ";
const BRANCH_PREFIX: &str = "|----";
const CHILD_DETAIL_PREFIX: &str = "|    ";
const NODE_DETAIL_PREFIX: &str = "|  ";
pub const EXPLAIN_EXCHANGE_RECIPE_ID: &str = "sr3514.explain_costs.unique_exchange_link";
pub const EXPLAIN_EXCHANGE_RECIPE_VERSION: &str = "1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainLimits {
    pub max_input_bytes: usize,
    pub max_lines: usize,
    pub max_line_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_relations: usize,
    pub max_fact_candidates: usize,
    pub max_fact_digits: usize,
}

impl Default for ExplainLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_lines: 250_000,
            max_line_bytes: 256 * 1024,
            max_nodes: 20_000,
            max_depth: 128,
            // A plan tree has at most node_count - fragment_count PlanChild
            // relations; reserve the other half for cross-fragment links.
            max_relations: 40_000,
            max_fact_candidates: 250_000,
            max_fact_digits: 39,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainStructureState {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainSinkKind {
    None,
    Result,
    Exchange,
    Multicast,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainNodeKind {
    Aggregate,
    Analytic,
    AssertNumberOfRows,
    Decode,
    Except,
    Exchange,
    FileScan,
    HashJoin,
    Intersect,
    MergingExchange,
    NestLoopJoin,
    OlapScan,
    PartitionTopN,
    Project,
    Repeat,
    ScanMysql,
    Select,
    Sort,
    TableValueFunction,
    TopN,
    Union,
}

impl ExplainNodeKind {
    fn is_exchange_target(&self) -> bool {
        matches!(self, Self::Exchange | Self::MergingExchange)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplainSourceSpan {
    pub line_start: u32,
    pub line_end: u32,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainFragment {
    pub fragment_ref: String,
    pub ordinal: u32,
    pub root_node_ref: Option<String>,
    pub sink_kind: ExplainSinkKind,
    pub source: ExplainSourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainPlanNode {
    pub node_ref: String,
    pub fragment_ref: String,
    pub plan_node_id: u32,
    pub kind: ExplainNodeKind,
    pub source_ordinal: u32,
    pub source: ExplainSourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainPlanChild {
    pub relation_ref: String,
    pub parent_node_ref: String,
    pub child_node_ref: String,
    pub input_role: String,
    pub evidence: Vec<ExplainSourceSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainExchangeCandidate {
    pub candidate_ref: String,
    pub source_fragment_ref: String,
    pub fragment_source: ExplainSourceSpan,
    pub source_root_node_ref: Option<String>,
    pub exchange_identity: String,
    pub sink_source: ExplainSourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainExchangeLink {
    pub relation_ref: String,
    pub source_node_ref: String,
    pub target_node_ref: String,
    pub exchange_identity: String,
    pub recipe_id: String,
    pub recipe_version: String,
    pub evidence: Vec<ExplainSourceSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainLimitation {
    pub code: String,
    pub source: Option<ExplainSourceSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainBudget {
    pub input_bytes: usize,
    pub input_lines: usize,
    pub parsed_nodes: usize,
    pub parsed_relations: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainSafeFragment {
    pub fragment_ref: String,
    pub ordinal: u32,
    pub sink_kind: ExplainSinkKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainSafeNode {
    pub node_ref: String,
    pub fragment_ref: String,
    pub plan_node_id: u32,
    pub kind: ExplainNodeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainSafeStructure {
    pub fragments: Vec<ExplainSafeFragment>,
    pub nodes: Vec<ExplainSafeNode>,
    pub relation_refs: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainPlanFacts {
    pub state: ExplainStructureState,
    pub fragments: Vec<ExplainFragment>,
    pub result_root_node_refs: Vec<String>,
    pub nodes: Vec<ExplainPlanNode>,
    pub plan_children: Vec<ExplainPlanChild>,
    pub exchange_candidates: Vec<ExplainExchangeCandidate>,
    pub verified_exchange_links: Vec<ExplainExchangeLink>,
    pub plan_node_facts: Vec<ExplainPlanNodeFactSet>,
    pub limitations: Vec<ExplainLimitation>,
    pub budget: ExplainBudget,
    pub safe_structure: ExplainSafeStructure,
}

struct FinishFactsInput {
    state: ExplainStructureState,
    input_bytes: usize,
    input_lines: usize,
    fragments: Vec<ExplainFragment>,
    result_root_node_refs: Vec<String>,
    nodes: Vec<ExplainPlanNode>,
    plan_children: Vec<ExplainPlanChild>,
    exchange_candidates: Vec<ExplainExchangeCandidate>,
    verified_exchange_links: Vec<ExplainExchangeLink>,
    plan_node_fact_works: Vec<ExplainPlanNodeFactWork>,
    limitations: Vec<ExplainLimitation>,
}

#[derive(Clone, Debug)]
struct NodeContext {
    node_index: usize,
    depth: usize,
    prefix: String,
    detail_prefix: String,
    has_children: bool,
    child_zero_seen: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExplainPlanFactKind {
    CardinalityEstimate,
    Distribution,
    JoinStrategy,
    AggregatePhase,
    PlanChildCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainPlanDistribution {
    Broadcast,
    Gather,
    Shuffle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainPlanJoinStrategy {
    InnerBroadcast,
    InnerPartitioned,
    RightOuterBucketShuffle,
    RightOuterPartitioned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainPlanAggregatePhase {
    UpdateSerialize,
    UpdateFinalize,
    MergeFinalize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainPlanFactValue {
    CardinalityEstimate(String),
    Distribution(ExplainPlanDistribution),
    JoinStrategy(ExplainPlanJoinStrategy),
    AggregatePhase(ExplainPlanAggregatePhase),
    PlanChildCount(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainPlanFactCandidate {
    pub kind: ExplainPlanFactKind,
    pub value: Result<ExplainPlanFactValue, &'static str>,
    pub source: ExplainSourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainPlanNodeFact {
    Available {
        kind: ExplainPlanFactKind,
        value: ExplainPlanFactValue,
        source: Vec<ExplainSourceSpan>,
    },
    Unavailable {
        kind: ExplainPlanFactKind,
        reason_codes: Vec<String>,
        source: Vec<ExplainSourceSpan>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplainPlanNodeFactSet {
    pub node_ref: String,
    pub source_ordinal: u32,
    pub facts: Vec<ExplainPlanNodeFact>,
    pub source: Vec<ExplainSourceSpan>,
    pub partial: bool,
}

#[derive(Clone, Debug)]
struct ExplainPlanNodeFactWork {
    node_ref: String,
    source_ordinal: u32,
    node_source: ExplainSourceSpan,
    candidates: Vec<ExplainPlanFactCandidate>,
}

#[derive(Clone, Debug)]
struct ExchangeSink {
    identity: String,
    source: ExplainSourceSpan,
}

#[derive(Clone, Debug)]
struct FragmentWork {
    fragment_ref: String,
    ordinal: u32,
    source: ExplainSourceSpan,
    root_node_ref: Option<String>,
    sink_kind: ExplainSinkKind,
    exchange_sinks: Vec<ExchangeSink>,
    conflicting_exchange_identities: BTreeSet<String>,
    open_attribute_quote: Option<(char, ExplainSourceSpan)>,
    multiline_attribute_source: Option<ExplainSourceSpan>,
    contexts: Vec<NodeContext>,
    last_node_index: Option<usize>,
    previous_was_separator: bool,
    blocked: bool,
    accepted: bool,
}

impl FragmentWork {
    fn new(fragment_ref: String, ordinal: u32, source: ExplainSourceSpan, accepted: bool) -> Self {
        Self {
            fragment_ref,
            ordinal,
            source,
            root_node_ref: None,
            sink_kind: ExplainSinkKind::None,
            exchange_sinks: Vec::new(),
            conflicting_exchange_identities: BTreeSet::new(),
            open_attribute_quote: None,
            multiline_attribute_source: None,
            contexts: Vec::new(),
            last_node_index: None,
            previous_was_separator: false,
            blocked: false,
            accepted,
        }
    }

    fn source_record(&self) -> ExplainFragment {
        ExplainFragment {
            fragment_ref: self.fragment_ref.clone(),
            ordinal: self.ordinal,
            root_node_ref: self.root_node_ref.clone(),
            sink_kind: self.sink_kind.clone(),
            source: self.source,
        }
    }
}

#[derive(Clone, Debug)]
struct NodeHeader {
    parent_prefix: String,
    node_prefix: String,
    is_branch: bool,
    plan_node_id: u32,
    kind: ExplainNodeKind,
    aggregate_phase: Option<Result<ExplainPlanAggregatePhase, &'static str>>,
}

/// Parses only the frozen 3.5.14 `EXPLAIN COSTS` formatter grammar.  The
/// returned facts deliberately exclude every raw attribute, expression,
/// predicate, table name, address, and runtime metric.
pub fn normalize_explain_costs(input: &str, limits: &ExplainLimits) -> ExplainPlanFacts {
    if input.len() > limits.max_input_bytes {
        return unavailable(input.len(), "input_bytes_exceeded", None);
    }
    if limits.max_nodes == 0
        || limits.max_depth == 0
        || limits.max_relations == 0
        || limits.max_fact_candidates == 0
        || limits.max_fact_digits == 0
    {
        return unavailable(input.len(), "invalid_explain_limits", None);
    }

    let mut fragments = Vec::new();
    let mut works = Vec::new();
    let mut nodes = Vec::new();
    let mut plan_children = Vec::new();
    let mut child_zero_flags = Vec::new();
    let mut plan_node_fact_works = Vec::new();
    let mut fact_candidate_count = 0usize;
    let mut limitations = Vec::new();
    let mut fragment_refs = BTreeSet::new();
    let mut fragment_ordinals = BTreeSet::new();
    let mut plan_node_ids = BTreeSet::new();
    let mut current: Option<FragmentWork> = None;
    let mut line_count = 0usize;
    let mut stopped_for_budget = false;

    for line in lines_with_spans(input) {
        line_count += 1;
        if line_count > limits.max_lines {
            limitations.push(limitation("input_lines_exceeded", Some(line.source)));
            stopped_for_budget = true;
            break;
        }
        if line.text.len() > limits.max_line_bytes {
            limitations.push(limitation("input_line_bytes_exceeded", Some(line.source)));
            stopped_for_budget = true;
            break;
        }

        if let Some(fragment) = current.as_mut()
            && fragment.open_attribute_quote.is_some()
        {
            advance_attribute_quote(fragment, line.text, line.source);
            continue;
        }

        if let Some((ordinal, fragment_ref)) = parse_fragment_header(line.text) {
            finish_fragment(current.take(), &mut fragments, &mut works, &mut limitations);
            let unique_fragment_ref = fragment_refs.insert(fragment_ref.clone());
            let unique_ordinal = fragment_ordinals.insert(ordinal);
            let accepted = unique_fragment_ref && unique_ordinal;
            if !accepted {
                limitations.push(limitation("duplicate_plan_fragment", Some(line.source)));
            }
            current = Some(FragmentWork::new(
                fragment_ref,
                ordinal,
                line.source,
                accepted,
            ));
            continue;
        }

        let Some(fragment) = current.as_mut() else {
            continue;
        };
        if !fragment.accepted {
            observe_conflicted_fragment_line(fragment, line.text);
            continue;
        }

        if let Some(sink) = parse_sink_line(line.text) {
            accept_sink(fragment, sink, line.source, &mut limitations);
            fragment.previous_was_separator = false;
            continue;
        }

        match parse_node_header(line.text) {
            Some(Ok(header)) => accept_node(
                fragment,
                header,
                line.source,
                limits,
                &mut nodes,
                &mut plan_children,
                &mut child_zero_flags,
                &mut plan_node_fact_works,
                &mut fact_candidate_count,
                &mut plan_node_ids,
                &mut limitations,
            ),
            Some(Err(code)) => {
                limitations.push(limitation(code, Some(line.source)));
                fragment.blocked = true;
                fragment.previous_was_separator = false;
            }
            None => {
                collect_plan_fact_candidate(
                    fragment,
                    line.text,
                    line.source,
                    limits,
                    &mut plan_node_fact_works,
                    &mut fact_candidate_count,
                    &mut limitations,
                );
                advance_attribute_quote(fragment, line.text, line.source);
                if looks_like_malformed_tree_prefix(line.text) {
                    limitations.push(limitation("malformed_tree_prefix", Some(line.source)));
                    fragment.blocked = true;
                }
                mark_separator(fragment, line.text);
            }
        }
    }

    finish_fragment(current, &mut fragments, &mut works, &mut limitations);
    restore_child_order(&mut plan_children, &child_zero_flags);
    if stopped_for_budget && nodes.is_empty() {
        return finish_facts(FinishFactsInput {
            state: ExplainStructureState::Unavailable,
            input_bytes: input.len(),
            input_lines: line_count.saturating_sub(1),
            fragments,
            result_root_node_refs: Vec::new(),
            nodes,
            plan_children,
            exchange_candidates: Vec::new(),
            verified_exchange_links: Vec::new(),
            plan_node_fact_works,
            limitations,
        });
    }

    let (
        result_root_node_refs,
        exchange_candidates,
        conflicted_exchange_identities,
        conflicted_fragment_refs,
    ) = collect_candidates(&works, &mut limitations);
    // A verified Exchange relation makes a global uniqueness claim.  Emit one
    // only after every identity-relevant grammar path was observed without a
    // trust, structure, or budget limitation; local facts remain useful when
    // this conservative gate is closed.
    let identity_observation_complete = !stopped_for_budget && limitations.is_empty();
    let resolved_exchange_links = resolve_unique_exchange_links(
        &exchange_candidates,
        &nodes,
        limits.max_relations,
        plan_children.len(),
        &conflicted_exchange_identities,
        &conflicted_fragment_refs,
        &mut limitations,
    );
    let verified_exchange_links = if !identity_observation_complete {
        limitations.push(limitation("exchange_identity_observation_incomplete", None));
        Vec::new()
    } else {
        resolved_exchange_links
    };
    if result_root_node_refs.is_empty() {
        limitations.push(limitation("result_root_unproven", None));
    }
    if result_root_node_refs.len() > 1 {
        limitations.push(limitation("multiple_result_roots", None));
    }

    let state = if nodes.is_empty() && fragments.is_empty() {
        ExplainStructureState::Unavailable
    } else if nodes.is_empty() {
        // A recognized fragment/sink is still trustworthy structure evidence.
        // Preserve it as Partial while keeping the unregistered node label out
        // of the normalized graph and safe projection.
        ExplainStructureState::Partial
    } else if limitations.is_empty() {
        ExplainStructureState::Complete
    } else {
        ExplainStructureState::Partial
    };
    finish_facts(FinishFactsInput {
        state,
        input_bytes: input.len(),
        input_lines: line_count,
        fragments,
        result_root_node_refs,
        nodes,
        plan_children,
        exchange_candidates,
        verified_exchange_links,
        plan_node_fact_works,
        limitations,
    })
}

fn accept_sink(
    fragment: &mut FragmentWork,
    sink: SinkLine,
    source: ExplainSourceSpan,
    limitations: &mut Vec<ExplainLimitation>,
) {
    if let SinkLine::Exchange(identity) = &sink {
        // Even an otherwise-unacceptable sink observed this identity.  Keep
        // that negative evidence so a later acceptable fragment cannot make
        // the identity appear globally unique.
        fragment.exchange_sinks.push(ExchangeSink {
            identity: identity.clone(),
            source,
        });
    }
    if fragment.root_node_ref.is_some() {
        limitations.push(limitation("sink_after_plan_structure", Some(source)));
        fragment.blocked = true;
        return;
    }
    match sink {
        SinkLine::Result => match fragment.sink_kind {
            ExplainSinkKind::None => fragment.sink_kind = ExplainSinkKind::Result,
            _ => {
                limitations.push(limitation("conflicting_fragment_sink", Some(source)));
                fragment.blocked = true;
            }
        },
        // `MultiCastDataSink.getVerboseExplain` prints `MultiCastDataSinks:`,
        // then one `OutPut Partition` / `OutPut Exchange Id` pair per destination.
        SinkLine::Multicast => match fragment.sink_kind {
            ExplainSinkKind::None => fragment.sink_kind = ExplainSinkKind::Multicast,
            _ => {
                limitations.push(limitation("conflicting_fragment_sink", Some(source)));
                fragment.blocked = true;
            }
        },
        SinkLine::Exchange(_) => match fragment.sink_kind {
            ExplainSinkKind::Result => {
                limitations.push(limitation("conflicting_fragment_sink", Some(source)));
                fragment.blocked = true;
            }
            ExplainSinkKind::Multicast => {}
            ExplainSinkKind::None | ExplainSinkKind::Exchange => {
                fragment.sink_kind = ExplainSinkKind::Exchange;
            }
        },
        SinkLine::InvalidExchange => {
            limitations.push(limitation("invalid_exchange_identity", Some(source)));
            fragment.blocked = true;
        }
        SinkLine::Unsupported => {
            limitations.push(limitation("unsupported_sink_form", Some(source)));
            fragment.blocked = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn accept_node(
    fragment: &mut FragmentWork,
    header: NodeHeader,
    source: ExplainSourceSpan,
    limits: &ExplainLimits,
    nodes: &mut Vec<ExplainPlanNode>,
    plan_children: &mut Vec<ExplainPlanChild>,
    child_zero_flags: &mut Vec<bool>,
    plan_node_fact_works: &mut Vec<ExplainPlanNodeFactWork>,
    fact_candidate_count: &mut usize,
    plan_node_ids: &mut BTreeSet<u32>,
    limitations: &mut Vec<ExplainLimitation>,
) {
    if fragment.blocked {
        limitations.push(limitation("node_after_untrusted_structure", Some(source)));
        fragment.previous_was_separator = false;
        return;
    }
    if nodes.len() >= limits.max_nodes {
        limitations.push(limitation("node_limit_exceeded", Some(source)));
        fragment.blocked = true;
        fragment.previous_was_separator = false;
        return;
    }
    let parent_context_index = if fragment.root_node_ref.is_none() {
        if header.is_branch || header.parent_prefix != BASE_PREFIX {
            limitations.push(limitation("fragment_root_prefix_invalid", Some(source)));
            fragment.blocked = true;
            fragment.previous_was_separator = false;
            return;
        }
        None
    } else {
        if !fragment.previous_was_separator {
            limitations.push(limitation("node_header_outside_structure", Some(source)));
            fragment.blocked = true;
            fragment.previous_was_separator = false;
            return;
        }
        let parent_context_index = if header.is_branch {
            find_branch_parent(&fragment.contexts, &header.parent_prefix)
        } else {
            find_child_zero_parent(&fragment.contexts, &header.parent_prefix)
        };
        let Some(parent_context_index) = parent_context_index else {
            limitations.push(limitation("plan_child_parent_unproven", Some(source)));
            fragment.blocked = true;
            fragment.previous_was_separator = false;
            return;
        };
        let parent_index = fragment.contexts[parent_context_index].node_index;
        if nodes[parent_index].kind.is_exchange_target() {
            limitations.push(limitation(
                "exchange_node_children_unsupported",
                Some(source),
            ));
            fragment.blocked = true;
            fragment.previous_was_separator = false;
            return;
        }
        Some(parent_context_index)
    };

    let depth = parent_context_index
        .map(|context_index| fragment.contexts[context_index].depth.saturating_add(1))
        .unwrap_or(0);
    if depth > limits.max_depth {
        limitations.push(limitation("depth_limit_exceeded", Some(source)));
        fragment.blocked = true;
        fragment.previous_was_separator = false;
        return;
    }
    if parent_context_index.is_some() && plan_children.len() >= limits.max_relations {
        limitations.push(limitation("relation_limit_exceeded", Some(source)));
        fragment.blocked = true;
        fragment.previous_was_separator = false;
        return;
    }
    if !plan_node_ids.insert(header.plan_node_id) {
        limitations.push(limitation("duplicate_plan_node_id", Some(source)));
    }

    let source_ordinal = match u32::try_from(nodes.len()) {
        Ok(value) => value,
        Err(_) => {
            limitations.push(limitation("node_ordinal_out_of_range", Some(source)));
            fragment.blocked = true;
            fragment.previous_was_separator = false;
            return;
        }
    };
    let node_ref_base = format!(
        "explain:{}:plan:{}",
        fragment.fragment_ref, header.plan_node_id
    );
    let node_ref = if nodes.iter().any(|node| {
        node.fragment_ref == fragment.fragment_ref && node.plan_node_id == header.plan_node_id
    }) {
        format!("{node_ref_base}:line:{}", source.line_start)
    } else {
        node_ref_base
    };
    let node_index = nodes.len();
    let is_root = fragment.root_node_ref.is_none();
    let detail_prefix = node_detail_prefix(&header, is_root);
    let aggregate_phase = header.aggregate_phase.clone().map(|value| {
        if *fact_candidate_count >= limits.max_fact_candidates {
            if !limitations
                .iter()
                .any(|item| item.code == "fact_candidate_limit_exceeded")
            {
                limitations.push(limitation("fact_candidate_limit_exceeded", Some(source)));
            }
            Err("fact_candidate_limit_exceeded")
        } else {
            *fact_candidate_count += 1;
            value
        }
    });
    nodes.push(ExplainPlanNode {
        node_ref: node_ref.clone(),
        fragment_ref: fragment.fragment_ref.clone(),
        plan_node_id: header.plan_node_id,
        kind: header.kind,
        source_ordinal,
        source,
    });
    plan_node_fact_works.push(ExplainPlanNodeFactWork {
        node_ref: node_ref.clone(),
        source_ordinal,
        node_source: source,
        candidates: aggregate_phase
            .map(|result| ExplainPlanFactCandidate {
                kind: ExplainPlanFactKind::AggregatePhase,
                value: result.map(ExplainPlanFactValue::AggregatePhase),
                source,
            })
            .into_iter()
            .collect(),
    });

    if let Some(parent_context_index) = parent_context_index {
        let parent_index = fragment.contexts[parent_context_index].node_index;
        let parent = &nodes[parent_index];
        plan_children.push(ExplainPlanChild {
            relation_ref: format!("plan_child:{}->{}", parent.node_ref, node_ref),
            parent_node_ref: parent.node_ref.clone(),
            child_node_ref: node_ref.clone(),
            input_role: "child".into(),
            evidence: vec![parent.source, source],
        });
        child_zero_flags.push(!header.is_branch);
        if !header.is_branch
            && let Some(context) = fragment.contexts.get_mut(parent_context_index)
        {
            context.child_zero_seen = true;
        }
    } else {
        fragment.root_node_ref = Some(node_ref.clone());
    }

    fragment.contexts.push(NodeContext {
        node_index,
        depth,
        prefix: header.node_prefix,
        detail_prefix,
        has_children: false,
        child_zero_seen: false,
    });
    fragment.last_node_index = Some(node_index);
    fragment.previous_was_separator = false;
}

/// `PlanNode.getCostExplain` prints `children[1..]` as `|----` branches before
/// `children[0]`, so text order is not child order.  Refill each parent's own
/// relation slots in child order (child zero, then branches as printed); every
/// other relation keeps its position.
fn restore_child_order(plan_children: &mut [ExplainPlanChild], child_zero_flags: &[bool]) {
    let mut slots_by_parent = BTreeMap::<&str, Vec<usize>>::new();
    for (index, relation) in plan_children.iter().enumerate() {
        slots_by_parent
            .entry(relation.parent_node_ref.as_str())
            .or_default()
            .push(index);
    }
    let reorders = slots_by_parent
        .into_values()
        .filter(|slots| slots.len() > 1)
        .map(|slots| {
            let mut ordered = slots.clone();
            ordered.sort_by_key(|index| !child_zero_flags[*index]);
            (slots, ordered)
        })
        .collect::<Vec<_>>();
    if reorders.is_empty() {
        return;
    }
    let original = plan_children.to_vec();
    for (slots, ordered) in reorders {
        for (slot, from) in slots.into_iter().zip(ordered) {
            plan_children[slot] = original[from].clone();
        }
    }
}

fn find_branch_parent(contexts: &[NodeContext], prefix: &str) -> Option<usize> {
    contexts.iter().rposition(|context| {
        context.prefix == prefix && context.has_children && !context.child_zero_seen
    })
}

fn find_child_zero_parent(contexts: &[NodeContext], prefix: &str) -> Option<usize> {
    contexts.iter().rposition(|context| {
        context.prefix == prefix && context.has_children && !context.child_zero_seen
    })
}

fn mark_separator(fragment: &mut FragmentWork, line: &str) {
    fragment.previous_was_separator = is_structural_separator(line);
    if !fragment.previous_was_separator {
        return;
    }
    let Some(last_node_index) = fragment.last_node_index else {
        return;
    };
    if let Some(context) = fragment
        .contexts
        .iter_mut()
        .rev()
        .find(|context| context.node_index == last_node_index)
        && line == format!("{}{}", context.prefix, NODE_DETAIL_PREFIX)
    {
        context.has_children = true;
    }
}

fn node_detail_prefix(header: &NodeHeader, _is_root: bool) -> String {
    if header.is_branch {
        format!("{}   ", header.node_prefix)
    } else {
        format!("{}{}", header.node_prefix, NODE_DETAIL_PREFIX)
    }
}

fn collect_plan_fact_candidate(
    fragment: &FragmentWork,
    line: &str,
    source: ExplainSourceSpan,
    limits: &ExplainLimits,
    works: &mut [ExplainPlanNodeFactWork],
    count: &mut usize,
    limitations: &mut Vec<ExplainLimitation>,
) {
    let Some(node_index) = fragment.last_node_index else {
        return;
    };
    let Some(context) = fragment
        .contexts
        .iter()
        .rev()
        .find(|item| item.node_index == node_index)
    else {
        return;
    };
    let leaf_detail_prefix = format!("{}   ", context.prefix);
    let Some(detail) = line
        .strip_prefix(&context.detail_prefix)
        .or_else(|| line.strip_prefix(&leaf_detail_prefix))
    else {
        return;
    };
    let candidate = if let Some(value) = detail.strip_prefix("cardinality: ") {
        Some((
            ExplainPlanFactKind::CardinalityEstimate,
            parse_cardinality_estimate(value, limits.max_fact_digits)
                .map(ExplainPlanFactValue::CardinalityEstimate),
        ))
    } else if let Some(value) = detail.strip_prefix("distribution type: ") {
        Some((
            ExplainPlanFactKind::Distribution,
            parse_distribution(value).map(ExplainPlanFactValue::Distribution),
        ))
    } else {
        detail.strip_prefix("join op: ").map(|value| {
            (
                ExplainPlanFactKind::JoinStrategy,
                parse_join_strategy(value).map(ExplainPlanFactValue::JoinStrategy),
            )
        })
    };
    let Some((kind, value)) = candidate else {
        return;
    };
    if *count >= limits.max_fact_candidates {
        if !limitations
            .iter()
            .any(|item| item.code == "fact_candidate_limit_exceeded")
        {
            limitations.push(limitation("fact_candidate_limit_exceeded", Some(source)));
        }
        return;
    }
    *count += 1;
    if let Some(work) = works.get_mut(node_index) {
        work.candidates.push(ExplainPlanFactCandidate {
            kind,
            value,
            source,
        });
    }
}

fn parse_cardinality_estimate(value: &str, max_digits: usize) -> Result<String, &'static str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid_cardinality_estimate");
    }
    if value.len() > max_digits {
        return Err("fact_digit_budget_exceeded");
    }
    let canonical = value.trim_start_matches('0');
    Ok(if canonical.is_empty() {
        "0".into()
    } else {
        canonical.into()
    })
}

fn parse_distribution(value: &str) -> Result<ExplainPlanDistribution, &'static str> {
    match value {
        "BROADCAST" => Ok(ExplainPlanDistribution::Broadcast),
        "GATHER" => Ok(ExplainPlanDistribution::Gather),
        "SHUFFLE" => Ok(ExplainPlanDistribution::Shuffle),
        _ => Err("unregistered_distribution"),
    }
}

fn parse_join_strategy(value: &str) -> Result<ExplainPlanJoinStrategy, &'static str> {
    match value {
        "INNER JOIN (BROADCAST)" => Ok(ExplainPlanJoinStrategy::InnerBroadcast),
        "INNER JOIN (PARTITIONED)" => Ok(ExplainPlanJoinStrategy::InnerPartitioned),
        "RIGHT OUTER JOIN (BUCKET_SHUFFLE(S))" => {
            Ok(ExplainPlanJoinStrategy::RightOuterBucketShuffle)
        }
        "RIGHT OUTER JOIN (PARTITIONED)" => Ok(ExplainPlanJoinStrategy::RightOuterPartitioned),
        _ => Err("unregistered_join_strategy"),
    }
}

fn observe_conflicted_fragment_line(fragment: &mut FragmentWork, line: &str) {
    if let Some(SinkLine::Exchange(identity)) = parse_sink_line(line) {
        fragment.conflicting_exchange_identities.insert(identity);
    }
    if let Some(Ok(header)) = parse_node_header(line) {
        fragment
            .conflicting_exchange_identities
            .insert(header.plan_node_id.to_string());
    }
}

fn advance_attribute_quote(fragment: &mut FragmentWork, line: &str, source: ExplainSourceSpan) {
    if let Some((_, quote_source)) = fragment.open_attribute_quote {
        fragment
            .multiline_attribute_source
            .get_or_insert(quote_source);
    }
    let mut active = fragment.open_attribute_quote.map(|(quote, _)| quote);
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        match active {
            Some(quote) if character == quote => active = None,
            Some(_) => {}
            None if matches!(character, '\'' | '"') => active = Some(character),
            None => {}
        }
    }
    fragment.open_attribute_quote = active.map(|quote| {
        let source = fragment
            .open_attribute_quote
            .map(|(_, source)| source)
            .unwrap_or(source);
        (quote, source)
    });
}

fn finish_fragment(
    current: Option<FragmentWork>,
    fragments: &mut Vec<ExplainFragment>,
    works: &mut Vec<FragmentWork>,
    limitations: &mut Vec<ExplainLimitation>,
) {
    let Some(fragment) = current else {
        return;
    };
    if !fragment.accepted {
        works.push(fragment);
        return;
    }
    if let Some((_, source)) = fragment.open_attribute_quote {
        limitations.push(limitation("unterminated_attribute_quote", Some(source)));
    }
    if let Some(source) = fragment.multiline_attribute_source {
        limitations.push(limitation("multiline_attribute_unsupported", Some(source)));
    }
    if fragment
        .contexts
        .iter()
        .any(|context| context.has_children && !context.child_zero_seen)
    {
        limitations.push(limitation("plan_child_zero_missing", Some(fragment.source)));
    }
    if fragment.root_node_ref.is_none() {
        limitations.push(limitation("fragment_root_missing", Some(fragment.source)));
    }
    fragments.push(fragment.source_record());
    works.push(fragment);
}

fn collect_candidates(
    works: &[FragmentWork],
    limitations: &mut Vec<ExplainLimitation>,
) -> (
    Vec<String>,
    Vec<ExplainExchangeCandidate>,
    BTreeSet<String>,
    BTreeSet<String>,
) {
    let mut result_roots = Vec::new();
    let mut candidates = Vec::new();
    let mut conflicted_exchange_identities = BTreeSet::new();
    let mut conflicted_fragment_refs = BTreeSet::new();
    for fragment in works {
        if !fragment.accepted {
            conflicted_fragment_refs.insert(fragment.fragment_ref.clone());
            conflicted_exchange_identities
                .extend(fragment.conflicting_exchange_identities.iter().cloned());
            continue;
        }
        if matches!(fragment.sink_kind, ExplainSinkKind::Result) {
            if let Some(root) = &fragment.root_node_ref {
                result_roots.push(root.clone());
            } else {
                limitations.push(limitation(
                    "result_sink_without_root",
                    Some(fragment.source),
                ));
            }
        }
        let is_multicast = matches!(fragment.sink_kind, ExplainSinkKind::Multicast);
        if fragment.blocked
            || (!fragment.exchange_sinks.is_empty()
                && !matches!(fragment.sink_kind, ExplainSinkKind::Exchange)
                && !is_multicast)
        {
            conflicted_exchange_identities.extend(
                fragment
                    .exchange_sinks
                    .iter()
                    .map(|sink| sink.identity.clone()),
            );
            continue;
        }
        if !matches!(fragment.sink_kind, ExplainSinkKind::Exchange) && !is_multicast {
            continue;
        }
        // A data stream sink names exactly one exchange; a multicast sink names
        // one distinct exchange per destination.
        let distinct = fragment
            .exchange_sinks
            .iter()
            .map(|sink| sink.identity.as_str())
            .collect::<BTreeSet<_>>();
        let proven = if is_multicast {
            !fragment.exchange_sinks.is_empty() && distinct.len() == fragment.exchange_sinks.len()
        } else {
            fragment.exchange_sinks.len() == 1
        };
        if !proven {
            conflicted_exchange_identities.extend(
                fragment
                    .exchange_sinks
                    .iter()
                    .map(|sink| sink.identity.clone()),
            );
            limitations.push(limitation(
                if is_multicast {
                    "multicast_destinations_not_proven"
                } else {
                    "single_exchange_sink_not_proven"
                },
                Some(fragment.source),
            ));
            continue;
        }
        for sink in &fragment.exchange_sinks {
            candidates.push(ExplainExchangeCandidate {
                candidate_ref: format!(
                    "exchange_candidate:{}:{}",
                    fragment.fragment_ref, sink.identity
                ),
                source_fragment_ref: fragment.fragment_ref.clone(),
                fragment_source: fragment.source,
                source_root_node_ref: fragment.root_node_ref.clone(),
                exchange_identity: sink.identity.clone(),
                sink_source: sink.source,
            });
        }
    }
    (
        result_roots,
        candidates,
        conflicted_exchange_identities,
        conflicted_fragment_refs,
    )
}

fn resolve_unique_exchange_links(
    candidates: &[ExplainExchangeCandidate],
    nodes: &[ExplainPlanNode],
    max_relations: usize,
    plan_relation_count: usize,
    conflicted_exchange_identities: &BTreeSet<String>,
    conflicted_fragment_refs: &BTreeSet<String>,
    limitations: &mut Vec<ExplainLimitation>,
) -> Vec<ExplainExchangeLink> {
    let mut source_count_by_identity = BTreeMap::<&str, usize>::new();
    for candidate in candidates {
        *source_count_by_identity
            .entry(candidate.exchange_identity.as_str())
            .or_default() += 1;
    }
    let mut nodes_by_identity = BTreeMap::<String, Vec<&ExplainPlanNode>>::new();
    for node in nodes {
        nodes_by_identity
            .entry(node.plan_node_id.to_string())
            .or_default()
            .push(node);
    }

    let mut links = Vec::new();
    for source in candidates {
        let identity = source.exchange_identity.as_str();
        if conflicted_fragment_refs.contains(&source.source_fragment_ref) {
            limitations.push(limitation(
                "exchange_source_fragment_conflicted",
                Some(source.sink_source),
            ));
            continue;
        }
        if conflicted_exchange_identities.contains(identity) {
            limitations.push(limitation(
                "exchange_identity_conflicted_by_duplicate_fragment",
                Some(source.sink_source),
            ));
            continue;
        }
        let Some(source_root) = &source.source_root_node_ref else {
            limitations.push(limitation(
                "exchange_source_root_missing",
                Some(source.sink_source),
            ));
            continue;
        };
        let identity_nodes = nodes_by_identity
            .get(identity)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if source_count_by_identity.get(identity) != Some(&1) || identity_nodes.len() != 1 {
            limitations.push(limitation(
                "exchange_identity_not_unique",
                Some(source.sink_source),
            ));
            continue;
        }
        let target = identity_nodes[0];
        if conflicted_fragment_refs.contains(&target.fragment_ref) {
            limitations.push(limitation(
                "exchange_target_fragment_conflicted",
                Some(source.sink_source),
            ));
            continue;
        }
        if !target.kind.is_exchange_target() {
            limitations.push(limitation(
                "exchange_target_not_exchange",
                Some(source.sink_source),
            ));
            continue;
        }
        let Some(source_root_node) = nodes.iter().find(|node| node.node_ref == *source_root) else {
            limitations.push(limitation(
                "exchange_source_root_missing",
                Some(source.sink_source),
            ));
            continue;
        };
        if source.source_fragment_ref == target.fragment_ref {
            limitations.push(limitation(
                "exchange_target_same_fragment",
                Some(source.sink_source),
            ));
            continue;
        }
        if plan_relation_count + links.len() >= max_relations {
            limitations.push(limitation(
                "relation_limit_exceeded",
                Some(source.sink_source),
            ));
            continue;
        }
        links.push(ExplainExchangeLink {
            relation_ref: format!("exchange_link:{}->{}", source_root, target.node_ref),
            source_node_ref: source_root.clone(),
            target_node_ref: target.node_ref.clone(),
            exchange_identity: identity.into(),
            recipe_id: EXPLAIN_EXCHANGE_RECIPE_ID.into(),
            recipe_version: EXPLAIN_EXCHANGE_RECIPE_VERSION.into(),
            evidence: vec![
                source.fragment_source,
                source_root_node.source,
                source.sink_source,
                target.source,
            ],
        });
    }
    links
}

fn finish_facts(input: FinishFactsInput) -> ExplainPlanFacts {
    let FinishFactsInput {
        state,
        input_bytes,
        input_lines,
        fragments,
        result_root_node_refs,
        nodes,
        plan_children,
        exchange_candidates,
        verified_exchange_links,
        plan_node_fact_works,
        limitations,
    } = input;
    let plan_node_facts =
        build_plan_node_facts(&nodes, &plan_children, &plan_node_fact_works, &state);
    let relation_refs = plan_children
        .iter()
        .map(|relation| relation.relation_ref.clone())
        .chain(
            verified_exchange_links
                .iter()
                .map(|relation| relation.relation_ref.clone()),
        )
        .collect::<Vec<_>>();
    let limitation_codes = limitations
        .iter()
        .map(|limitation| limitation.code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let safe_structure = ExplainSafeStructure {
        fragments: fragments
            .iter()
            .map(|fragment| ExplainSafeFragment {
                fragment_ref: fragment.fragment_ref.clone(),
                ordinal: fragment.ordinal,
                sink_kind: fragment.sink_kind.clone(),
            })
            .collect(),
        nodes: nodes
            .iter()
            .map(|node| ExplainSafeNode {
                node_ref: node.node_ref.clone(),
                fragment_ref: node.fragment_ref.clone(),
                plan_node_id: node.plan_node_id,
                kind: node.kind.clone(),
            })
            .collect(),
        relation_refs,
        limitations: limitation_codes,
    };
    ExplainPlanFacts {
        state,
        fragments,
        result_root_node_refs,
        nodes,
        plan_children,
        exchange_candidates,
        verified_exchange_links,
        plan_node_facts,
        limitations,
        budget: ExplainBudget {
            input_bytes,
            input_lines,
            parsed_nodes: safe_structure.nodes.len(),
            parsed_relations: safe_structure.relation_refs.len(),
        },
        safe_structure,
    }
}

fn build_plan_node_facts(
    nodes: &[ExplainPlanNode],
    children: &[ExplainPlanChild],
    works: &[ExplainPlanNodeFactWork],
    state: &ExplainStructureState,
) -> Vec<ExplainPlanNodeFactSet> {
    let mut sets = Vec::with_capacity(nodes.len());
    for node in nodes {
        let Some(work) = works.iter().find(|item| item.node_ref == node.node_ref) else {
            continue;
        };
        let mut candidates = BTreeMap::<ExplainPlanFactKind, Vec<&ExplainPlanFactCandidate>>::new();
        for candidate in &work.candidates {
            candidates
                .entry(candidate.kind.clone())
                .or_default()
                .push(candidate);
        }
        let mut facts = Vec::new();
        for (kind, candidates) in candidates {
            if candidates.len() == 1 {
                let candidate = candidates[0];
                match &candidate.value {
                    Ok(value) => facts.push(ExplainPlanNodeFact::Available {
                        kind,
                        value: value.clone(),
                        source: vec![candidate.source],
                    }),
                    Err(code) => facts.push(ExplainPlanNodeFact::Unavailable {
                        kind,
                        reason_codes: vec![(*code).into()],
                        source: vec![candidate.source],
                    }),
                }
            } else {
                let available_values = candidates
                    .iter()
                    .filter_map(|candidate| candidate.value.as_ref().ok())
                    .collect::<Vec<_>>();
                let mut reason_codes = vec!["duplicate_fact".into()];
                if available_values.first().is_some_and(|first| {
                    available_values.iter().skip(1).any(|value| value != first)
                }) {
                    reason_codes.push("conflicting_fact".into());
                }
                facts.push(ExplainPlanNodeFact::Unavailable {
                    kind,
                    reason_codes,
                    source: candidates
                        .iter()
                        .map(|candidate| candidate.source)
                        .collect(),
                });
            }
        }
        let child_sources = children
            .iter()
            .filter(|child| child.parent_node_ref == node.node_ref)
            .flat_map(|child| child.evidence.iter().copied())
            .collect::<Vec<_>>();
        if matches!(state, ExplainStructureState::Complete) {
            let count = children
                .iter()
                .filter(|child| child.parent_node_ref == node.node_ref)
                .count();
            match u32::try_from(count) {
                Ok(count) => facts.push(ExplainPlanNodeFact::Available {
                    kind: ExplainPlanFactKind::PlanChildCount,
                    value: ExplainPlanFactValue::PlanChildCount(count),
                    source: if child_sources.is_empty() {
                        vec![work.node_source]
                    } else {
                        child_sources
                    },
                }),
                Err(_) => facts.push(ExplainPlanNodeFact::Unavailable {
                    kind: ExplainPlanFactKind::PlanChildCount,
                    reason_codes: vec!["plan_child_count_out_of_range".into()],
                    source: vec![work.node_source],
                }),
            }
        } else {
            facts.push(ExplainPlanNodeFact::Unavailable {
                kind: ExplainPlanFactKind::PlanChildCount,
                reason_codes: vec!["plan_child_count_incomplete".into()],
                source: vec![work.node_source],
            });
        }
        let mut source = facts
            .iter()
            .flat_map(|fact| match fact {
                ExplainPlanNodeFact::Available { source, .. }
                | ExplainPlanNodeFact::Unavailable { source, .. } => source.iter().copied(),
            })
            .collect::<Vec<_>>();
        source.sort_by_key(|item| (item.line_start, item.byte_start));
        source.dedup();
        let partial = facts
            .iter()
            .any(|fact| matches!(fact, ExplainPlanNodeFact::Unavailable { .. }))
            || !matches!(state, ExplainStructureState::Complete);
        sets.push(ExplainPlanNodeFactSet {
            node_ref: node.node_ref.clone(),
            source_ordinal: work.source_ordinal,
            facts,
            source: if source.is_empty() {
                vec![work.node_source]
            } else {
                source
            },
            partial,
        });
    }
    sets
}

fn unavailable(
    input_bytes: usize,
    code: &str,
    source: Option<ExplainSourceSpan>,
) -> ExplainPlanFacts {
    finish_facts(FinishFactsInput {
        state: ExplainStructureState::Unavailable,
        input_bytes,
        input_lines: 0,
        fragments: Vec::new(),
        result_root_node_refs: Vec::new(),
        nodes: Vec::new(),
        plan_children: Vec::new(),
        exchange_candidates: Vec::new(),
        verified_exchange_links: Vec::new(),
        plan_node_fact_works: Vec::new(),
        limitations: vec![limitation(code, source)],
    })
}

fn limitation(code: &str, source: Option<ExplainSourceSpan>) -> ExplainLimitation {
    ExplainLimitation {
        code: code.into(),
        source,
    }
}

#[derive(Clone, Copy)]
struct InputLine<'a> {
    text: &'a str,
    source: ExplainSourceSpan,
}

struct InputLines<'a> {
    remainder: &'a str,
    byte_start: usize,
    line_no: u32,
}

impl<'a> Iterator for InputLines<'a> {
    type Item = InputLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remainder.is_empty() {
            return None;
        }
        let input = self.remainder;
        let (inclusive, remainder) = match input.find('\n') {
            Some(index) => (&input[..=index], &input[index + 1..]),
            None => (input, ""),
        };
        self.remainder = remainder;
        let without_lf = inclusive.strip_suffix('\n').unwrap_or(inclusive);
        let text = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        let source = ExplainSourceSpan {
            line_start: self.line_no,
            line_end: self.line_no,
            byte_start: self.byte_start,
            byte_end: self.byte_start + text.len(),
        };
        self.byte_start += inclusive.len();
        self.line_no = self.line_no.saturating_add(1);
        Some(InputLine { text, source })
    }
}

fn lines_with_spans(input: &str) -> InputLines<'_> {
    InputLines {
        remainder: input,
        byte_start: 0,
        line_no: 1,
    }
}

fn parse_fragment_header(line: &str) -> Option<(u32, String)> {
    let rest = line.strip_prefix("PLAN FRAGMENT ")?;
    let ordinal_digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let ordinal = parse_decimal_exact(&rest[..ordinal_digits])?;
    let rest = &rest[ordinal_digits..];
    let rest = rest.strip_prefix("(F")?;
    let fragment_digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let fragment_number = &rest[..fragment_digits];
    parse_decimal_exact(fragment_number)?;
    let rest = &rest[fragment_digits..];
    (rest == ")").then(|| (ordinal, format!("F{fragment_number}")))
}

fn parse_node_header(line: &str) -> Option<Result<NodeHeader, &'static str>> {
    let rest = line.strip_prefix(BASE_PREFIX)?;
    let (parent_prefix, rest) = tree_prefix(rest)?;
    if rest.starts_with(NODE_DETAIL_PREFIX) {
        return None;
    }
    if rest.starts_with('|') {
        return Some(Err("malformed_tree_prefix"));
    }
    let (plan_node_id, label) = split_decimal(rest)?;
    let label = label.strip_prefix(':')?;
    let Some((kind, aggregate_phase)) = parse_node_kind(label) else {
        return Some(Err("unregistered_plan_node"));
    };
    let is_branch = parent_prefix.ends_with(BRANCH_PREFIX);
    let node_prefix = if is_branch {
        let parent = parent_prefix.strip_suffix(BRANCH_PREFIX)?;
        format!("{parent}{CHILD_DETAIL_PREFIX}")
    } else {
        parent_prefix.clone()
    };
    let parent_prefix = if is_branch {
        parent_prefix.strip_suffix(BRANCH_PREFIX)?.into()
    } else {
        parent_prefix
    };
    Some(Ok(NodeHeader {
        parent_prefix,
        node_prefix,
        is_branch,
        plan_node_id,
        kind,
        aggregate_phase,
    }))
}

fn tree_prefix(rest: &str) -> Option<(String, &str)> {
    let mut consumed = String::new();
    let mut remainder = rest;
    while let Some(after) = remainder.strip_prefix(CHILD_DETAIL_PREFIX) {
        consumed.push_str(CHILD_DETAIL_PREFIX);
        remainder = after;
    }
    if let Some(after) = remainder.strip_prefix(BRANCH_PREFIX) {
        consumed.push_str(BRANCH_PREFIX);
        remainder = after;
    }
    Some((format!("{BASE_PREFIX}{consumed}"), remainder))
}

fn parse_node_kind(
    label: &str,
) -> Option<(
    ExplainNodeKind,
    Option<Result<ExplainPlanAggregatePhase, &'static str>>,
)> {
    match label {
        "AGGREGATE" => Some((ExplainNodeKind::Aggregate, None)),
        "AGGREGATE (update serialize)" => Some((
            ExplainNodeKind::Aggregate,
            Some(Ok(ExplainPlanAggregatePhase::UpdateSerialize)),
        )),
        "AGGREGATE (update finalize)" => Some((
            ExplainNodeKind::Aggregate,
            Some(Ok(ExplainPlanAggregatePhase::UpdateFinalize)),
        )),
        "AGGREGATE (merge finalize)" => Some((
            ExplainNodeKind::Aggregate,
            Some(Ok(ExplainPlanAggregatePhase::MergeFinalize)),
        )),
        _ if label
            .strip_prefix("AGGREGATE ")
            .is_some_and(|mode| mode.starts_with('(') && mode.ends_with(')')) =>
        {
            Some((
                ExplainNodeKind::Aggregate,
                Some(Err("unregistered_aggregate_phase")),
            ))
        }
        "ANALYTIC" => Some((ExplainNodeKind::Analytic, None)),
        "ASSERT NUMBER OF ROWS" => Some((ExplainNodeKind::AssertNumberOfRows, None)),
        "Decode" => Some((ExplainNodeKind::Decode, None)),
        "EXCEPT" => Some((ExplainNodeKind::Except, None)),
        "EXCHANGE" => Some((ExplainNodeKind::Exchange, None)),
        "FileScanNode" => Some((ExplainNodeKind::FileScan, None)),
        "HASH JOIN" => Some((ExplainNodeKind::HashJoin, None)),
        "INTERSECT" => Some((ExplainNodeKind::Intersect, None)),
        "MERGING-EXCHANGE" => Some((ExplainNodeKind::MergingExchange, None)),
        "NESTLOOP JOIN" => Some((ExplainNodeKind::NestLoopJoin, None)),
        "OlapScanNode" => Some((ExplainNodeKind::OlapScan, None)),
        "PARTITION-TOP-N" => Some((ExplainNodeKind::PartitionTopN, None)),
        "Project" => Some((ExplainNodeKind::Project, None)),
        "REPEAT_NODE" => Some((ExplainNodeKind::Repeat, None)),
        "SCAN MYSQL" => Some((ExplainNodeKind::ScanMysql, None)),
        "SELECT" => Some((ExplainNodeKind::Select, None)),
        "SORT" => Some((ExplainNodeKind::Sort, None)),
        "TableValueFunction" => Some((ExplainNodeKind::TableValueFunction, None)),
        "TOP-N" => Some((ExplainNodeKind::TopN, None)),
        "UNION" => Some((ExplainNodeKind::Union, None)),
        _ => None,
    }
}

#[derive(Clone, Debug)]
enum SinkLine {
    Result,
    Multicast,
    Exchange(String),
    InvalidExchange,
    Unsupported,
}

fn parse_sink_line(line: &str) -> Option<SinkLine> {
    let rest = line.strip_prefix(BASE_PREFIX)?;
    if rest == "RESULT SINK" {
        return Some(SinkLine::Result);
    }
    if rest == "MultiCastDataSinks:" || rest == "MultiCastDataSinks" {
        return Some(SinkLine::Multicast);
    }
    if let Some(identity) = rest.strip_prefix("OutPut Exchange Id: ") {
        return Some(
            parse_decimal_exact(identity)
                .map(|value| SinkLine::Exchange(value.to_string()))
                .unwrap_or(SinkLine::InvalidExchange),
        );
    }
    if rest == "STREAM DATA SINK" || rest.starts_with("EXCHANGE ID: ") {
        return Some(SinkLine::Unsupported);
    }
    None
}

fn is_structural_separator(line: &str) -> bool {
    let Some(mut rest) = line.strip_prefix(BASE_PREFIX) else {
        return false;
    };
    while let Some(after) = rest.strip_prefix(CHILD_DETAIL_PREFIX) {
        rest = after;
    }
    rest.is_empty() || rest == NODE_DETAIL_PREFIX
}

fn looks_like_malformed_tree_prefix(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(BASE_PREFIX) else {
        return false;
    };
    rest.starts_with("|---")
        || rest.starts_with('├')
        || rest.starts_with('└')
        || rest.starts_with('│')
}

fn split_decimal(value: &str) -> Option<(u32, &str)> {
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    Some((parse_decimal_exact(&value[..digits])?, &value[digits..]))
}

fn parse_decimal_exact(value: &str) -> Option<u32> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<u32>().ok())
        .flatten()
}
