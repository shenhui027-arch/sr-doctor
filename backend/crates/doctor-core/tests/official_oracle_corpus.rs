//! Offline black-box regression against the official StarRocks 3.5.14 FE.
//!
//! Every case in `fixtures/runtime/3.5.14/oracle-corpus/manifest.json` (single
//! FE / single BE allin1), `oracle-corpus-multi-be/manifest.json` (1 FE +
//! 3 BE split images), `oracle-corpus-shapes/manifest.json` (allin1, plan
//! shapes absent from the first two) and `oracle-corpus-multi-be-shapes/` (the
//! v0.6-12 shapes on 1 FE + 3 BE) carries a real Level 1 Profile
//! (`get_query_profile`) and the same FE's own `ANALYZE PROFILE` text (for the
//! spill cases, the `EXPLAIN ANALYZE` text of the same run), captured from
//! an isolated fixed-digest Docker fixture by `scripts/e2e/capture-oracle-corpus.ps1`
//! (`-Topology multi-be` for the two multi-BE corpora). This test runs each
//! Profile through the product pipeline and compares the report with the
//! official ExplainAnalyzer output:
//!
//! * plan topology: official (parent, child) plan-node pairs versus
//!   `graph_view.edges` (normalized by `direction_semantics`);
//! * per-node time share (0.01 percentage point tolerance) and the official
//!   `>30` / `>15` hotspot classes versus `execution_summary.nodes`;
//! * per-node `OutputRows` versus `execution_summary.nodes[*].output_rows`;
//! * every rule evaluation (status and first reason), so a rule regression or
//!   fix on real Profiles is a visible diff change too;
//! * per-node fragment: the `Fragment N` section each plan node is printed in
//!   versus `graph_view.nodes[*].fragment_id`.
//!
//! A second test proves that each evaluated time-share rule consumes exactly
//! the execution-summary node time (no second NodeInfo implementation).
//!
//! Each corpus's diff is pinned in `fixtures/expected/3.5.14/` (`oracle-corpus-diff.json`,
//! `oracle-corpus-multi-be-diff.json`, `oracle-corpus-shapes-diff.json`,
//! `oracle-corpus-multi-be-shapes-diff.json`): it
//! records today's agreement *and* today's gaps, so a later fix or regression
//! is a visible diff change. Re-bless deliberately with
//! `ORACLE_CORPUS_BLESS=1 cargo test -p doctor-core --test official_oracle_corpus`.

use std::collections::{BTreeMap, BTreeSet};
use std::{env, fs, path::PathBuf};

use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    AdapterContext, AdapterLimits, AdapterOutput, InputEnvelope, adapt_input,
    assemble_public_report, evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Official ExplainAnalyzer pseudo plan-node id of the final RESULT_SINK.
const FINAL_SINK_ID: &str = "-1";
/// connected-smoke.ps1 tolerance: 0.01 percentage point = 1 basis point.
const SHARE_TOLERANCE_BP: i64 = 1;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("workspace root")
}

/// Single FE / single BE allin1 corpus (v0.4-12, v0.5-04).
const SINGLE_BE: Corpus = Corpus {
    dir: "oracle-corpus",
    expected: "oracle-corpus-diff.json",
    min_cases: 10,
};
/// 1 FE + 3 BE split-image corpus (v0.6-01): the same SQL plus cross-BE cases.
const MULTI_BE: Corpus = Corpus {
    dir: "oracle-corpus-multi-be",
    expected: "oracle-corpus-multi-be-diff.json",
    min_cases: 24,
};
/// allin1 corpus of plan shapes the first two never produce (v0.6-12): spill,
/// ASSERT_NUM_ROWS, REPEAT, TABLE_FUNCTION, INTERSECT, FILE_SCAN, MV rewrite;
/// v0.7-05 added PARTITION_TOP_N and MYSQL_SCAN.
/// Spill cases carry an `EXPLAIN ANALYZE` answer of the same run because
/// `ANALYZE PROFILE` cannot re-parse their DOUBLE_VALUE counter.
const SHAPES: Corpus = Corpus {
    dir: "oracle-corpus-shapes",
    expected: "oracle-corpus-shapes-diff.json",
    min_cases: 11,
};
/// The v0.6-12 shapes (c25–c33, same SQL) on the 1 FE + 3 BE fixture (v0.7-05).
const MULTI_BE_SHAPES: Corpus = Corpus {
    dir: "oracle-corpus-multi-be-shapes",
    expected: "oracle-corpus-multi-be-shapes-diff.json",
    min_cases: 9,
};
const CORPORA: [Corpus; 4] = [SINGLE_BE, MULTI_BE, SHAPES, MULTI_BE_SHAPES];

#[derive(Clone, Copy)]
struct Corpus {
    dir: &'static str,
    expected: &'static str,
    min_cases: usize,
}

impl Corpus {
    fn root(self) -> PathBuf {
        workspace_root()
            .join("fixtures/runtime/3.5.14")
            .join(self.dir)
    }

    fn expected_path(self) -> PathBuf {
        workspace_root()
            .join("fixtures/expected/3.5.14")
            .join(self.expected)
    }

    fn manifest(self) -> Value {
        serde_json::from_str(
            &fs::read_to_string(self.root().join("manifest.json")).expect("manifest"),
        )
        .expect("manifest json")
    }

    fn read_verified(self, name: &str, expected_sha256: &str) -> String {
        let bytes = fs::read(self.root().join(name)).unwrap_or_else(|_| panic!("read {name}"));
        assert_eq!(
            sha256_hex(&bytes),
            expected_sha256.to_ascii_lowercase(),
            "{name}: SHA-256 drift from manifest"
        );
        String::from_utf8(bytes).unwrap_or_else(|_| panic!("{name} is UTF-8"))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// Official ANALYZE PROFILE text parser (ExplainAnalyzer 3.5.14 layout).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct OfficialNode {
    name: String,
    share_bp: Option<i64>,
    output_rows: Option<u64>,
    /// `N` of the `Fragment N` section the node is printed in.
    fragment: Option<String>,
}

struct OfficialEdge {
    parent: String,
    child: String,
    /// `PROBE` / `BUILD` as printed for join children.
    role: Option<String>,
    /// Exchange (named by a fragment sink) to the producing fragment's root.
    cross_fragment: bool,
}

#[derive(Default)]
struct OfficialAnalysis {
    nodes: BTreeMap<String, OfficialNode>,
    edges: Vec<OfficialEdge>,
    top_nodes: Vec<(String, i64)>,
    /// Roots of fragments ending in MULTI_CAST_DATA_SINK: one parent per target.
    multicast_roots: BTreeMap<String, usize>,
}

/// Removes ANSI SGR sequences (`ESC [ ... m`); ANALYZE PROFILE colors its output.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// `"97.15%"`-style text inside a trailing `(...)` group, as basis points.
fn percent_to_bp(text: &str) -> Option<i64> {
    let start = text.rfind('(')?;
    let end = text[start..].find("%)")? + start;
    let number = &text[start + 1..end];
    let (whole, fraction) = number.split_once('.')?;
    if fraction.len() != 2 {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let fraction: i64 = fraction.parse().ok()?;
    Some(whole * 100 + fraction)
}

/// RuntimeProfile.printCounter(UNIT): `60` or `200.000K (200000)`.
fn parse_unit_counter(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Some(start) = text.find('(') {
        let end = text[start..].find(')')? + start;
        return text[start + 1..end].parse().ok();
    }
    text.parse().ok()
}

fn id_after(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)? + marker.len();
    let end = text[start..].find(')')? + start;
    Some(text[start..end].to_owned())
}

fn parse_official(raw: &str) -> OfficialAnalysis {
    let text = strip_ansi(raw);
    let mut analysis = OfficialAnalysis::default();
    let mut in_top = false;
    // Per fragment: sink target exchange ids and the fragment root.
    let mut fragments: Vec<(Vec<String>, Option<String>)> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    // Plan node whose metric lines follow; FINAL_SINK_ID for the result sink.
    let mut current: Option<String> = None;
    let mut current_has_time = false;
    let mut current_has_rows = false;
    let mut current_fragment: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "Top Most Time-consuming Nodes:" {
            in_top = true;
            continue;
        }
        if in_top {
            let is_item = trimmed
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            if is_item {
                let id = id_after(trimmed, "(id=").unwrap_or_else(|| FINAL_SINK_ID.to_owned());
                let bp = percent_to_bp(trimmed).expect("top node share");
                analysis.top_nodes.push((id, bp));
                continue;
            }
            in_top = false;
        }
        if let Some(number) = line.strip_prefix("Fragment ") {
            current_fragment = Some(number.trim().to_owned());
            fragments.push((Vec::new(), None));
            stack.clear();
            current = None;
            continue;
        }
        let marker = line
            .char_indices()
            .find(|(_, c)| *c == '└' || *c == '├')
            .map(|(byte, _)| byte);
        if let Some(byte) = marker
            && (line[byte..].starts_with("└──") || line[byte..].starts_with("├──"))
        {
            let depth = line[..byte].chars().count() / 3;
            let rest = line[byte..].trim_start_matches(['└', '├', '─']);
            let (role, title) = match rest.strip_prefix('<').and_then(|r| r.split_once("> ")) {
                Some((role, title)) => (Some(role.to_owned()), title),
                None => (None, rest),
            };
            let name: String = title
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                .collect();
            let fragment = fragments.last_mut().expect("operator inside a fragment");
            current_has_time = false;
            current_has_rows = false;
            if depth == 0 {
                // Fragment sink.
                if let Some(ids) = id_after(title, "(ids=[") {
                    fragment.0 = ids
                        .trim_end_matches(']')
                        .split(',')
                        .map(|id| id.trim().to_owned())
                        .filter(|id| !id.is_empty())
                        .collect();
                    current = None;
                } else if let Some(id) = id_after(title, "(id=") {
                    fragment.0 = vec![id];
                    current = None;
                } else {
                    current = Some(FINAL_SINK_ID.to_owned());
                    analysis
                        .nodes
                        .entry(FINAL_SINK_ID.to_owned())
                        .or_default()
                        .name = name;
                }
                stack.clear();
                continue;
            }
            let id = id_after(title, "(id=").expect("plan node id");
            stack.truncate(depth - 1);
            if let Some(parent) = stack.last() {
                analysis.edges.push(OfficialEdge {
                    parent: parent.clone(),
                    child: id.clone(),
                    role,
                    cross_fragment: false,
                });
            } else {
                assert!(fragment.1.is_none(), "one root per fragment");
                fragment.1 = Some(id.clone());
            }
            stack.push(id.clone());
            let node = analysis.nodes.entry(id.clone()).or_default();
            assert!(node.name.is_empty(), "plan node {id} printed twice");
            node.name = name;
            node.fragment = current_fragment.clone();
            current = Some(id);
            continue;
        }
        let Some(id) = current.as_ref() else {
            continue;
        };
        let metric = line.trim_start_matches(['│', ' ']);
        if !current_has_time && let Some(value) = metric.strip_prefix("TotalTime: ") {
            current_has_time = true;
            let head = value.split(" [").next().unwrap_or(value);
            analysis.nodes.get_mut(id).expect("node").share_bp = percent_to_bp(head);
        } else if !current_has_rows && let Some(value) = metric.strip_prefix("OutputRows: ") {
            current_has_rows = true;
            analysis.nodes.get_mut(id).expect("node").output_rows = parse_unit_counter(value);
        }
    }
    // Cross-fragment relation: the exchange named by a sink is the parent of the
    // producing fragment's root (ExplainAnalyzer prints ExchangeNode as a leaf).
    for (targets, root) in fragments {
        let root = root.expect("fragment root");
        if targets.len() > 1 {
            analysis.multicast_roots.insert(root.clone(), targets.len());
        }
        for target in targets {
            analysis.edges.push(OfficialEdge {
                parent: target,
                child: root.clone(),
                role: None,
                cross_fragment: true,
            });
        }
    }
    analysis
}

// ---------------------------------------------------------------------------
// Product pipeline (same order as doctor-api: adapt → resolve → rules → report).
// ---------------------------------------------------------------------------

fn product_output(profile: &str) -> AdapterOutput {
    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, profile.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .expect("oracle Profile adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("oracle Profile resolves");
    evaluate_rules(&mut output).expect("rules evaluate");
    output
}

fn product_report(output: AdapterOutput) -> Value {
    let report = assemble_public_report(output).expect("report assembles");
    serde_json::to_value(&report).expect("report serializes")
}

/// One line per rule evaluation: status plus the first missing prerequisite
/// (not applicable) or the last comparison (evaluated).
fn rule_lines(report: &Value) -> Vec<Value> {
    report["rule_evaluations"]
        .as_array()
        .expect("rule evaluations")
        .iter()
        .map(|evaluation| {
            let reason = evaluation["missing_prerequisites"]
                .as_array()
                .and_then(|items| items.first())
                .or_else(|| {
                    evaluation["comparisons"]
                        .as_array()
                        .and_then(|items| items.last())
                })
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "rule_id": evaluation["rule_id"],
                "subject_ref": evaluation["subject_ref"],
                "status": evaluation["status"],
                "reason": reason,
            })
        })
        .collect()
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn exact_integer(value: &Value) -> Option<u64> {
    if str_of(value, "state") != Some("exact") {
        return None;
    }
    value
        .get("canonical_number")
        .and_then(|number| str_of(number, "value"))
        .and_then(|number| number.parse().ok())
}

fn official_class(bp: i64) -> Option<&'static str> {
    // The FE classifies the unrounded value; within 0.01 of a boundary the
    // two-decimal text cannot decide the class (same rule as connected-smoke).
    if (bp - 3000).abs() <= 1 || (bp - 1500).abs() <= 1 {
        return None;
    }
    Some(if bp > 3000 {
        "most_consuming"
    } else if bp > 1500 {
        "second_most_consuming"
    } else {
        "none"
    })
}

fn case_diff(case_id: &str, official: &OfficialAnalysis, report: &Value) -> Value {
    let graph = &report["graph_view"];
    let mut plan_id_of: BTreeMap<String, String> = BTreeMap::new();
    let mut graph_plan_ids: BTreeSet<String> = BTreeSet::new();
    for node in graph["nodes"].as_array().expect("graph nodes") {
        if let (Some(node_id), Some(plan)) = (str_of(node, "node_id"), str_of(node, "plan_node_id"))
        {
            plan_id_of.insert(node_id.to_owned(), plan.to_owned());
            graph_plan_ids.insert(plan.to_owned());
        }
    }

    // Normalize product edges into (parent, child) plan-node pairs.
    let mut ours: BTreeMap<(String, String), (BTreeSet<String>, BTreeSet<String>)> =
        BTreeMap::new();
    let mut not_comparable_edges = Vec::new();
    for edge in graph["edges"].as_array().expect("graph edges") {
        let from = str_of(edge, "from").unwrap_or_default();
        let to = str_of(edge, "to").unwrap_or_default();
        let semantics = str_of(edge, "direction_semantics").unwrap_or_default();
        let relation = str_of(edge, "relation").unwrap_or_default();
        let (Some(from_plan), Some(to_plan)) = (plan_id_of.get(from), plan_id_of.get(to)) else {
            not_comparable_edges.push(
                json!({"edge_id": edge["edge_id"], "reason": "endpoint_without_plan_node_id"}),
            );
            continue;
        };
        let key = match semantics {
            "parent_to_child" => (from_plan.clone(), to_plan.clone()),
            // Data flows from the producing fragment up to the consuming exchange.
            "upstream_to_downstream" => (to_plan.clone(), from_plan.clone()),
            _ => {
                not_comparable_edges.push(json!({"edge_id": edge["edge_id"], "relation": relation, "direction_semantics": semantics}));
                continue;
            }
        };
        let entry = ours.entry(key).or_default();
        entry.0.insert(relation.to_owned());
        if let Some(role) = str_of(edge, "input_role") {
            entry.1.insert(role.to_owned());
        }
    }
    let theirs: BTreeSet<(String, String)> = official
        .edges
        .iter()
        .map(|edge| (edge.parent.clone(), edge.child.clone()))
        .collect();
    let unresolved: Vec<String> = graph["unresolved_relations"]
        .as_array()
        .expect("unresolved relations")
        .iter()
        .filter_map(|relation| str_of(relation, "relation_ref").map(str::to_owned))
        .collect();
    let unresolved_for = |parent: &str, child: &str| -> Option<String> {
        unresolved
            .iter()
            .find(|reference| {
                let suffix = reference.rsplit(':').next().unwrap_or_default();
                suffix == parent || suffix == child
            })
            .cloned()
    };

    let mut matched = 0;
    let mut missing = Vec::new();
    let mut reversed = Vec::new();
    let mut role_mismatches = Vec::new();
    let mut role_unlabeled = Vec::new();
    let mut cross_fragment_coverage: BTreeMap<String, usize> = BTreeMap::new();
    for edge in &official.edges {
        let (parent, child) = (&edge.parent, &edge.child);
        if let Some((relations, roles)) = ours.get(&(parent.clone(), child.clone())) {
            matched += 1;
            if edge.cross_fragment {
                let key = relations.iter().cloned().collect::<Vec<_>>().join("+");
                *cross_fragment_coverage.entry(key).or_default() += 1;
            }
            if let Some(official_role) = edge.role.as_deref().map(str::to_ascii_lowercase) {
                if roles.is_empty() {
                    // Gap: the edge is drawn without a PROBE/BUILD label.
                    role_unlabeled
                        .push(json!({"parent": parent, "child": child, "official": official_role}));
                } else if !(roles.len() == 1 && roles.contains(&official_role)) {
                    role_mismatches.push(json!({
                    "parent": parent, "child": child,
                        "official": official_role, "product": roles,
                    }));
                }
            }
        } else if ours.contains_key(&(child.clone(), parent.clone())) {
            reversed.push(json!({"official_parent": parent, "official_child": child}));
        } else {
            missing.push(json!({
                "parent": parent, "child": child,
                "parent_name": official.nodes.get(parent).map(|n| n.name.as_str()),
                "child_name": official.nodes.get(child).map(|n| n.name.as_str()),
                "declared_unresolved": unresolved_for(parent, child),
            }));
        }
    }
    let extra: Vec<Value> = ours
        .iter()
        .filter(|((p, c), _)| {
            !theirs.contains(&(p.clone(), c.clone())) && !theirs.contains(&(c.clone(), p.clone()))
        })
        .map(|((p, c), (relations, _))| json!({"parent": p, "child": c, "relations": relations}))
        .collect();
    // Each declared-unresolved cross-fragment link versus what is actually drawn.
    let unresolved_check: Vec<Value> = unresolved
        .iter()
        .map(|reference| {
            let exchange = reference.rsplit(':').next().unwrap_or_default();
            let official_links: Vec<&OfficialEdge> = official
                .edges
                .iter()
                .filter(|edge| edge.cross_fragment && edge.parent == exchange)
                .collect();
            let drawn = official_links
                .iter()
                .filter(|edge| ours.contains_key(&(edge.parent.clone(), edge.child.clone())))
                .count();
            let status = if official_links.is_empty() {
                "no_official_cross_fragment_link"
            } else if drawn == official_links.len() {
                "declared_unresolved_but_drawn_and_matches_official"
            } else if drawn == 0 {
                "declared_unresolved_and_not_drawn"
            } else {
                "declared_unresolved_partially_drawn"
            };
            json!({"relation_ref": reference, "official_links": official_links.len(), "status": status})
        })
        .collect();

    // Execution summary.
    let summary = &report["execution_summary"];
    let mut summary_nodes: BTreeMap<String, &Value> = BTreeMap::new();
    for node in summary["nodes"].as_array().expect("summary nodes") {
        if let Some(plan) = str_of(node, "plan_node_id") {
            summary_nodes.insert(plan.to_owned(), node);
        }
    }
    let mut share_compared = 0;
    let mut max_delta_bp = 0i64;
    let mut share_mismatches = Vec::new();
    let mut hotspot_mismatches = Vec::new();
    let mut hotspot_boundaries = Vec::new();
    let mut rows_compared = 0;
    let mut row_mismatches = Vec::new();
    let mut official_only = Vec::new();
    let mut product_without_share = Vec::new();
    let mut product_without_rows = Vec::new();
    let mut result_sink = Value::Null;
    for (id, node) in &official.nodes {
        if id == FINAL_SINK_ID {
            // The product has no plan-node summary for the pseudo RESULT_SINK;
            // its rows are `execution_summary.result_rows`.
            let product_rows = exact_integer(&summary["result_rows"]);
            if let (Some(official_rows), Some(product_rows)) = (node.output_rows, product_rows) {
                rows_compared += 1;
                if official_rows != product_rows {
                    row_mismatches.push(json!({"plan_node_id": id, "name": node.name, "official": official_rows, "product": product_rows}));
                }
            }
            result_sink = json!({
                "official_share_bp": node.share_bp,
                "product_share": "not_represented_in_execution_summary_nodes",
                "official_output_rows": node.output_rows,
                "product_result_rows": product_rows,
            });
            continue;
        }
        let Some(ours) = summary_nodes.get(id) else {
            official_only.push(json!({"plan_node_id": id, "name": node.name}));
            continue;
        };
        if let Some(official_bp) = node.share_bp {
            match ours.get("time_share_basis_points").and_then(Value::as_i64) {
                Some(product_bp) => {
                    share_compared += 1;
                    let delta = (product_bp - official_bp).abs();
                    max_delta_bp = max_delta_bp.max(delta);
                    if delta > SHARE_TOLERANCE_BP {
                        share_mismatches.push(json!({"plan_node_id": id, "name": node.name, "official_bp": official_bp, "product_bp": product_bp}));
                    }
                    let product_class = str_of(ours, "hotspot").unwrap_or_default();
                    if product_class == "boundary" {
                        // The text precision interval straddles a threshold; the
                        // official tier is one reading inside it, not a mismatch.
                        hotspot_boundaries.push(json!({"plan_node_id": id, "name": node.name, "official_bp": official_bp}));
                    } else if let Some(class) = official_class(official_bp)
                        && class != product_class
                    {
                        hotspot_mismatches.push(json!({"plan_node_id": id, "name": node.name, "official": class, "product": product_class}));
                    }
                }
                None => product_without_share.push(
                    json!({"plan_node_id": id, "name": node.name, "official_bp": official_bp}),
                ),
            }
        }
        if let Some(official_rows) = node.output_rows {
            match exact_integer(&ours["output_rows"]) {
                Some(product_rows) => {
                    rows_compared += 1;
                    if product_rows != official_rows {
                        row_mismatches.push(json!({"plan_node_id": id, "name": node.name, "official": official_rows, "product": product_rows}));
                    }
                }
                None => product_without_rows.push(json!({
                    "plan_node_id": id, "name": node.name, "official": official_rows,
                    "product_state": str_of(&ours["output_rows"], "state"),
                    "product_reasons": ours["output_rows"]["reasons"],
                })),
            }
        }
    }
    // Fragment attribution: the section each plan node is printed in.
    let mut fragment_compared = 0;
    let mut fragment_mismatches = Vec::new();
    let mut fragment_unattributed = Vec::new();
    for node in graph["nodes"].as_array().expect("graph nodes") {
        let Some(plan) = str_of(node, "plan_node_id") else {
            continue;
        };
        let Some(official_fragment) = official.nodes.get(plan).and_then(|n| n.fragment.as_ref())
        else {
            continue;
        };
        match str_of(node, "fragment_id") {
            Some(product) => {
                fragment_compared += 1;
                if product != official_fragment {
                    fragment_mismatches.push(json!({"plan_node_id": plan, "official": official_fragment, "product": product}));
                }
            }
            None => fragment_unattributed
                .push(json!({"plan_node_id": plan, "official": official_fragment})),
        }
    }

    let official_plan_ids: BTreeSet<&String> = official
        .nodes
        .keys()
        .filter(|id| *id != FINAL_SINK_ID)
        .collect();
    let product_only_nodes: Vec<&String> = graph_plan_ids
        .iter()
        .filter(|id| !official_plan_ids.contains(id))
        .collect();
    let graph_missing_nodes: Vec<&String> = official_plan_ids
        .iter()
        .filter(|id| !graph_plan_ids.contains(**id))
        .copied()
        .collect();

    json!({
        "case_id": case_id,
        "official_plan_nodes": official_plan_ids.len(),
        "product_graph_plan_nodes": graph_plan_ids.len(),
        "product_summary_nodes": summary_nodes.len(),
        "graph_state": graph["state"],
        "execution_summary_state": summary["state"],
        "nodes": {
            "official_missing_from_graph": graph_missing_nodes,
            "graph_only": product_only_nodes,
        },
        "edges": {
            "official": theirs.len(),
            "product_plan_pairs": ours.len(),
            "matched": matched,
            "missing": missing,
            "extra": extra,
            "reversed": reversed,
            "role_mismatches": role_mismatches,
            "role_unlabeled": role_unlabeled,
            "cross_fragment_matched_by_relation": cross_fragment_coverage,
            "not_comparable": not_comparable_edges,
            "unresolved_relations": unresolved_check,
        },
        "fragments": {
            "compared": fragment_compared,
            "mismatches": fragment_mismatches,
            "unattributed": fragment_unattributed,
        },
        "time_share": {
            "compared": share_compared,
            "max_delta_bp": max_delta_bp,
            "mismatches": share_mismatches,
            "hotspot_mismatches": hotspot_mismatches,
            "hotspot_boundaries": hotspot_boundaries,
            "product_without_share": product_without_share,
        },
        "output_rows": {
            "compared": rows_compared,
            "mismatches": row_mismatches,
            "product_without_rows": product_without_rows,
        },
        "result_sink": result_sink,
        "summary_missing_official_nodes": official_only,
        "rules": rule_lines(report),
    })
}

fn check_official_consistency(case_id: &str, official: &OfficialAnalysis) {
    assert!(!official.top_nodes.is_empty(), "{case_id}: no top nodes");
    assert!(
        official.nodes.contains_key(FINAL_SINK_ID),
        "{case_id}: no RESULT_SINK"
    );
    for (id, bp) in &official.top_nodes {
        let node = official
            .nodes
            .get(id)
            .unwrap_or_else(|| panic!("{case_id}: top node {id} absent from the tree"));
        assert_eq!(
            node.share_bp,
            Some(*bp),
            "{case_id}: top/detail share for {id}"
        );
    }
    // The official plan is one tree, except that a multicast (CTE) producer has
    // one parent exchange per consumer: every other node except the fragment-0
    // root has exactly one parent.
    let mut parents: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &official.edges {
        *parents.entry(edge.child.as_str()).or_default() += 1;
    }
    for (child, count) in &parents {
        let expected = official.multicast_roots.get(*child).copied().unwrap_or(1);
        assert_eq!(*count, expected, "{case_id}: parents of {child}");
    }
    let plan_nodes = official.nodes.len() - 1;
    assert_eq!(
        parents.len() + 1,
        plan_nodes,
        "{case_id}: official tree covers every plan node"
    );
}

#[test]
fn official_oracle_corpus_diff_is_pinned() {
    assert_pinned_diff(SINGLE_BE);
}

#[test]
fn multi_be_official_oracle_corpus_diff_is_pinned() {
    assert_pinned_diff(MULTI_BE);
}

#[test]
fn plan_shape_official_oracle_corpus_diff_is_pinned() {
    assert_pinned_diff(SHAPES);
}

#[test]
fn multi_be_plan_shape_official_oracle_corpus_diff_is_pinned() {
    assert_pinned_diff(MULTI_BE_SHAPES);
}

/// Every official answer analyzes the very run whose Profile is pinned, and a
/// case answers with `EXPLAIN ANALYZE` only where the FE refused `ANALYZE
/// PROFILE` for that query id (recorded verbatim at capture).
#[test]
fn every_official_answer_is_for_the_pinned_query() {
    let mut explain_analyze = 0;
    for corpus in CORPORA {
        let manifest = corpus.manifest();
        for case in manifest["cases"].as_array().expect("cases") {
            let case_id = str_of(case, "case_id").expect("case id");
            let analyze = strip_ansi(&corpus.read_verified(
                str_of(case, "analyze_file").expect("analyze file"),
                str_of(case, "analyze_sha256").expect("analyze hash"),
            ));
            let query_id = str_of(case, "query_id").expect("query id");
            assert!(
                analyze
                    .lines()
                    .any(|line| line.trim() == format!("QueryId: {query_id}")),
                "{case_id}: official answer is not for {query_id}"
            );
            match str_of(case, "oracle_statement") {
                None => assert!(case.get("analyze_profile_refusal").is_none()),
                Some("EXPLAIN ANALYZE") => {
                    explain_analyze += 1;
                    let refusal = str_of(case, "analyze_profile_refusal").expect("refusal");
                    // RuntimeProfileParser's Long.parseLong on a `%.3f` DOUBLE_VALUE
                    // (`SkewMemTableSkewRatio`); the value differs per run and BE count.
                    let value = refusal
                        .split_once("For input string: \"")
                        .and_then(|(_, rest)| rest.split_once('"'))
                        .map(|(value, _)| value)
                        .unwrap_or_default();
                    let (integer, fraction) = value.split_once('.').unwrap_or_default();
                    let integer = integer.strip_prefix('-').unwrap_or(integer);
                    assert!(
                        !integer.is_empty()
                            && integer.bytes().all(|b| b.is_ascii_digit())
                            && fraction.len() == 3
                            && fraction.bytes().all(|b| b.is_ascii_digit()),
                        "{case_id}: {refusal}"
                    );
                    let sql = corpus.read_verified(
                        str_of(case, "sql_file").expect("sql file"),
                        str_of(case, "sql_sha256").expect("sql hash"),
                    );
                    assert!(sql.contains("SET enable_spill=true;"), "{case_id}");
                }
                Some(other) => panic!("{case_id}: unexpected oracle statement {other}"),
            }
        }
    }
    assert_eq!(
        explain_analyze, 4,
        "only the two spill cases of each shapes corpus"
    );
}

fn assert_pinned_diff(corpus: Corpus) {
    let manifest = corpus.manifest();
    let read_verified = |name: &str, sha: &str| corpus.read_verified(name, sha);
    assert!(
        str_of(&manifest, "server_version").is_some_and(|v| v.starts_with("3.5.14-23a56ec")),
        "corpus server version"
    );
    let cases = manifest["cases"].as_array().expect("cases");
    assert!(
        cases.len() >= corpus.min_cases,
        "{}: oracle corpus has at least {} cases",
        corpus.dir,
        corpus.min_cases
    );

    let mut diffs = Vec::new();
    let mut totals = BTreeMap::<&str, i64>::new();
    let mut correctness = Vec::new();
    for case in cases {
        let case_id = str_of(case, "case_id").expect("case id");
        read_verified(
            str_of(case, "sql_file").expect("sql file"),
            str_of(case, "sql_sha256").expect("sql hash"),
        );
        let profile = read_verified(
            str_of(case, "profile_file").expect("profile file"),
            str_of(case, "profile_sha256").expect("profile hash"),
        );
        let analyze = read_verified(
            str_of(case, "analyze_file").expect("analyze file"),
            str_of(case, "analyze_sha256").expect("analyze hash"),
        );
        let official = parse_official(&analyze);
        check_official_consistency(case_id, &official);
        let diff = case_diff(
            case_id,
            &official,
            &product_report(product_output(&profile)),
        );

        let count = |path: &[&str]| -> i64 {
            let mut value = &diff;
            for key in path {
                value = &value[*key];
            }
            value.as_array().map_or(0, |items| items.len() as i64)
        };
        *totals.entry("official_edges").or_default() += diff["edges"]["official"].as_i64().unwrap();
        *totals.entry("matched_edges").or_default() += diff["edges"]["matched"].as_i64().unwrap();
        *totals.entry("missing_edges").or_default() += count(&["edges", "missing"]);
        *totals.entry("join_role_unlabeled").or_default() += count(&["edges", "role_unlabeled"]);
        *totals.entry("product_without_rows").or_default() +=
            count(&["output_rows", "product_without_rows"]);
        *totals.entry("product_without_share").or_default() +=
            count(&["time_share", "product_without_share"]);
        *totals.entry("share_compared").or_default() +=
            diff["time_share"]["compared"].as_i64().unwrap();
        *totals.entry("rows_compared").or_default() +=
            diff["output_rows"]["compared"].as_i64().unwrap();
        *totals
            .entry("unresolved_relations_already_drawn")
            .or_default() += diff["edges"]["unresolved_relations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["status"] == "declared_unresolved_but_drawn_and_matches_official")
            .count() as i64;
        *totals.entry("unresolved_relations").or_default() +=
            count(&["edges", "unresolved_relations"]);
        *totals.entry("fragment_compared").or_default() +=
            diff["fragments"]["compared"].as_i64().unwrap();
        *totals.entry("fragment_unattributed").or_default() +=
            count(&["fragments", "unattributed"]);
        *totals.entry("hotspot_boundaries").or_default() +=
            count(&["time_share", "hotspot_boundaries"]);
        for rule in diff["rules"].as_array().expect("rules") {
            let status = rule["status"].as_str().expect("status");
            *totals
                .entry(match status {
                    "triggered" => "rule_triggered",
                    "suspected" => "rule_suspected",
                    "suppressed" => "rule_suppressed",
                    "not_triggered" => "rule_not_triggered",
                    _ => "rule_not_applicable",
                })
                .or_default() += 1;
        }
        let extra = count(&["edges", "extra"]);
        let reversed = count(&["edges", "reversed"]);
        let shares = count(&["time_share", "mismatches"]);
        let hotspots = count(&["time_share", "hotspot_mismatches"]);
        let rows = count(&["output_rows", "mismatches"]);
        let roles = count(&["edges", "role_mismatches"]);
        let fragments = count(&["fragments", "mismatches"]);
        *totals.entry("role_mismatches").or_default() += roles;
        *totals.entry("fragment_mismatches").or_default() += fragments;
        *totals.entry("extra_edges").or_default() += extra;
        *totals.entry("reversed_edges").or_default() += reversed;
        *totals.entry("share_mismatches").or_default() += shares;
        *totals.entry("hotspot_mismatches").or_default() += hotspots;
        *totals.entry("row_mismatches").or_default() += rows;
        if extra + reversed + roles + fragments + shares + hotspots + rows > 0 {
            correctness.push(json!({
                "case_id": case_id, "extra_edges": extra, "reversed_edges": reversed,
                "role_mismatches": roles, "fragment_mismatches": fragments,
                "share_mismatches": shares, "hotspot_mismatches": hotspots, "row_mismatches": rows,
            }));
        }
        diffs.push(diff);
    }

    let rendered = serde_json::to_string_pretty(&json!({
        "schema": "sr-doctor.oracle-corpus-diff.v1",
        "oracle": "StarRocks 3.5.14-23a56ec FE ANALYZE PROFILE (ExplainAnalyzer)",
        "share_tolerance_bp": SHARE_TOLERANCE_BP,
        "totals": totals,
        // Extra/reversed edges and role/fragment/share/hotspot/row mismatches are
        // correctness bugs, not coverage gaps.
        "correctness_discrepancies": correctness,
        "cases": diffs,
    }))
    .expect("diff serializes")
        + "\n";

    let target = corpus.expected_path();

    if env::var_os("ORACLE_CORPUS_BLESS").is_some() {
        fs::write(&target, &rendered).expect("write expected diff");
        return;
    }
    let expected = fs::read_to_string(&target)
        .expect("expected oracle diff")
        .replace("\r\n", "\n");
    if expected != rendered {
        let actual = target.with_extension("actual.json");
        let _ = fs::write(&actual, &rendered);
        panic!(
            "official oracle diff changed; review {} against {} and re-bless with ORACLE_CORPUS_BLESS=1 only if the change is intended",
            actual.display(),
            target.display()
        );
    }
}

/// Every evaluated time-share rule's numerator is exactly the execution-summary
/// node time: the sum of the official readings of the counters the rule lists
/// as inputs (minus the query denominator) equals `total_time` of the node
/// behind the rule's subject group.
#[test]
fn time_share_rule_numerators_equal_execution_summary_node_times() {
    for corpus in CORPORA {
        assert_time_share_numerators(corpus);
    }
}

fn assert_time_share_numerators(corpus: Corpus) {
    let manifest = corpus.manifest();
    let cases = manifest["cases"].as_array().expect("cases");
    let mut compared = 0;
    for case in cases {
        let case_id = str_of(case, "case_id").expect("case id");
        let profile = corpus.read_verified(
            str_of(case, "profile_file").expect("profile file"),
            str_of(case, "profile_sha256").expect("profile hash"),
        );
        let output = product_output(&profile);
        let observations = output
            .ir
            .metric_observations
            .iter()
            .map(|observation| (observation.observation_id.clone(), observation.clone()))
            .collect::<BTreeMap<_, _>>();
        let groups = output.ir.operator_groups.clone();
        let evaluations = output.ir.rule_evaluations.clone();
        let report = product_report(output);
        for evaluation in evaluations.iter().filter(|evaluation| {
            evaluation.rule_id.ends_with("_TIME_SHARE")
                && evaluation.status != doctor_contracts::RuleEvaluationStatus::NotApplicable
        }) {
            let subject = evaluation.subject_ref.as_deref().expect("subject");
            let group = groups
                .iter()
                .find(|group| group.operator_group_id == subject)
                .expect("subject group");
            let node = report["execution_summary"]["nodes"]
                .as_array()
                .expect("summary nodes")
                .iter()
                .find(|node| {
                    str_of(node, "plan_node_id") == group.plan_node_id.as_deref()
                        && node["fragment_ref"].as_str() == group.fragment_ref.as_deref()
                })
                .unwrap_or_else(|| panic!("{case_id}: {subject} has a summary node"));
            let mut numerator = 0u128;
            for id in &evaluation.input_observation_refs {
                let observation = &observations[id];
                if observation.metric_id.as_deref() == Some("QueryCumulativeOperatorTime") {
                    continue;
                }
                let number = observation.canonical_number.as_ref().expect("number");
                assert!(number.scale.is_none(), "{case_id}: integral nanoseconds");
                numerator += number.value.parse::<u128>().expect("integer");
            }
            assert_eq!(
                Some(numerator),
                exact_integer(&node["total_time"]).map(u128::from),
                "{case_id}: {} numerator for {subject}",
                evaluation.rule_id
            );
            compared += 1;
        }
    }
    assert!(
        compared >= 5 * cases.len(),
        "{}: only {compared} time-share numerators compared",
        corpus.dir
    );
}

/// The execution summary's operator-derived `fragment_ref` of every plan node
/// equals the Topology-derived graph `fragment_id` (itself pinned against the
/// official `Fragment N` sections), including a node whose only native operator
/// is a sink (c27 `ASSERT_NUM_ROWS` → `ASSERT_NUM_ROWS_SINK`).
#[test]
fn summary_fragments_equal_graph_fragments() {
    let mut compared = 0;
    for corpus in CORPORA {
        let manifest = corpus.manifest();
        for case in manifest["cases"].as_array().expect("cases") {
            let case_id = str_of(case, "case_id").expect("case id");
            let profile = corpus.read_verified(
                str_of(case, "profile_file").expect("profile file"),
                str_of(case, "profile_sha256").expect("profile hash"),
            );
            let report = product_report(product_output(&profile));
            let graph: BTreeMap<&str, Option<&str>> = report["graph_view"]["nodes"]
                .as_array()
                .expect("graph nodes")
                .iter()
                .filter_map(|node| {
                    Some((str_of(node, "plan_node_id")?, str_of(node, "fragment_id")))
                })
                .collect();
            for node in report["execution_summary"]["nodes"]
                .as_array()
                .expect("summary nodes")
            {
                let plan = str_of(node, "plan_node_id").expect("plan node id");
                assert_eq!(
                    str_of(node, "fragment_ref"),
                    graph[plan],
                    "{case_id}: plan node {plan}"
                );
                compared += 1;
            }
        }
    }
    assert!(compared >= 500, "only {compared} nodes compared");
}

/// No real Profile in the corpus loses its rules to an unproven source stage:
/// c13 (LIMIT pushed next to EXCHANGE_SOURCE / OLAP_SCAN in one pipeline) is
/// evaluated like every other case.
#[test]
fn every_corpus_profile_passes_the_official_source_stage_proof() {
    for corpus in CORPORA {
        assert_source_stage_proof(corpus);
    }
}

fn assert_source_stage_proof(corpus: Corpus) {
    let manifest = corpus.manifest();
    for case in manifest["cases"].as_array().expect("cases") {
        let case_id = str_of(case, "case_id").expect("case id");
        let profile = corpus.read_verified(
            str_of(case, "profile_file").expect("profile file"),
            str_of(case, "profile_sha256").expect("profile hash"),
        );
        let output = product_output(&profile);
        for evaluation in &output.ir.rule_evaluations {
            assert!(
                !evaluation
                    .missing_prerequisites
                    .iter()
                    .any(|reason| matches!(
                        reason.as_str(),
                        "typed_metric_unavailable" | "operator_binding_unproven"
                    )),
                "{case_id}: {} {:?}",
                evaluation.rule_id,
                evaluation.missing_prerequisites
            );
        }
    }
}
