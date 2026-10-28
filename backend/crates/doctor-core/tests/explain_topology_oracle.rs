//! Offline black-box comparison of the "topology only" EXPLAIN graph with the
//! official runtime plan of the same query (StarRocks 3.5.14).
//!
//! `fixtures/explain/starrocks-3.5.14/<corpus>/` holds the official
//! `EXPLAIN COSTS` text of every SQL in the single-BE runtime oracle corpora
//! (`fixtures/runtime/3.5.14/<corpus>/`, see `CORPORA`), captured on the same fixed-digest
//! allin1 fixture and datasets by `scripts/e2e/capture-explain-corpus.ps1`. Each
//! case's runtime Profile carries the FE's own `Topology` JSON
//! (`ProfilingExecPlan.Topology.buildFrom`): every plan node with its class-derived
//! name and ordered children, where an `EXCHANGE` / `MERGE_EXCHANGE` child is the
//! root of the fragment that sends to it.
//!
//! For every case the product EXPLAIN snapshot (`assemble_explain_snapshot`) is
//! compared with that Topology on:
//!
//! * plan nodes: `plan_node_id` and type (EXPLAIN label → runtime name, below);
//! * parent/child edges inside a fragment, including each parent's child order;
//! * Exchange relations: exchange node → root of the sending fragment;
//! * the result root;
//! * the EXPLAIN graph itself: `complete`, no limitation, no unresolved relation.
//!
//! A difference must either be fixed or be listed in `KNOWN_DIFFERENCES` with
//! its official reason; a stale entry fails as well. Print the per-case report
//! with `EXPLAIN_TOPOLOGY_ORACLE_REPORT=1 cargo test -p doctor-core --test
//! explain_topology_oracle -- --nocapture`.

use std::collections::{BTreeMap, BTreeSet};
use std::{env, fs, path::PathBuf};

use doctor_contracts::{DirectionSemantics, GraphRelation, GraphState, GraphView};
use doctor_core::{ExplainSnapshotInput, assemble_explain_snapshot};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// EXPLAIN label (product `plan_node_type`) → runtime Topology name.
///
/// EXPLAIN prints `PlanNode.planNodeName`, set by each node's constructor
/// (StarRocks 3.5.14@23a56ec, `fe/fe-core/src/main/java/com/starrocks/planner/`:
/// `AggregationNode` "AGGREGATE", `AnalyticEvalNode` "ANALYTIC",
/// `ExchangeNode` "EXCHANGE" / "MERGING-EXCHANGE", `HashJoinNode` "HASH JOIN",
/// `NestLoopJoinNode` "NESTLOOP JOIN", `SortNode` "TOP-N" / "SORT" /
/// "PARTITION-TOP-N", `SelectNode` "SELECT", `UnionNode` "UNION", `ExceptNode`
/// "EXCEPT", `IntersectNode` "INTERSECT", `AssertNumRowsNode` "ASSERT NUMBER OF
/// ROWS", `RepeatNode` "REPEAT_NODE", `TableFunctionNode` "TableValueFunction",
/// `MysqlScanNode` "SCAN MYSQL"; `FileScanNode` gets "FileScanNode" from
/// `sql/plan/PlanFragmentBuilder.java:4020-4021`). The Topology name is `ProfilingExecPlan.buildDisplayName`: the
/// class name without `Node`, upper snake case, `MERGE_` for a merging
/// exchange, and `TOP_N` / `PARTITION_TOP_N` for those `SortNode` forms.
fn runtime_name(explain_type: &str) -> Option<&'static str> {
    Some(match explain_type {
        "AGGREGATE" => "AGGREGATION",
        "ANALYTIC" => "ANALYTIC_EVAL",
        "ASSERT_NUMBER_OF_ROWS" => "ASSERT_NUM_ROWS",
        "DECODE" => "DECODE",
        "EXCEPT" => "EXCEPT",
        "EXCHANGE" => "EXCHANGE",
        "FILE_SCAN" => "FILE_SCAN",
        "HASH_JOIN" => "HASH_JOIN",
        "INTERSECT" => "INTERSECT",
        "MERGING_EXCHANGE" => "MERGE_EXCHANGE",
        "NEST_LOOP_JOIN" => "NEST_LOOP_JOIN",
        "OLAP_SCAN" => "OLAP_SCAN",
        "PARTITION_TOP_N" => "PARTITION_TOP_N",
        "PROJECT" => "PROJECT",
        "REPEAT" => "REPEAT",
        "SCAN_MYSQL" => "MYSQL_SCAN",
        "SELECT" => "SELECT",
        "SORT" => "SORT",
        "TABLE_VALUE_FUNCTION" => "TABLE_FUNCTION",
        "TOP_N" => "TOP_N",
        "UNION" => "UNION",
        _ => return None,
    })
}

/// (case, difference kind) → official reason. Empty while EXPLAIN and the
/// runtime plan agree on every case.
const KNOWN_DIFFERENCES: &[(&str, &str, &str)] = &[];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("workspace root")
}

/// Single-BE runtime corpora with captured EXPLAIN COSTS (same directory name
/// under `fixtures/explain/starrocks-3.5.14/` and `fixtures/runtime/3.5.14/`).
const CORPORA: [&str; 2] = ["oracle-corpus", "oracle-corpus-shapes"];

fn explain_root(corpus: &str) -> PathBuf {
    workspace_root()
        .join("fixtures/explain/starrocks-3.5.14")
        .join(corpus)
}

fn runtime_root(corpus: &str) -> PathBuf {
    workspace_root()
        .join("fixtures/runtime/3.5.14")
        .join(corpus)
}

fn read_json(path: PathBuf) -> Value {
    serde_json::from_str(&fs::read_to_string(&path).expect("read json")).expect("json")
}

fn read_verified(path: PathBuf, expected_sha256: &str) -> String {
    let bytes = fs::read(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        expected_sha256.to_ascii_lowercase(),
        "{}: SHA-256 drift from manifest",
        path.display()
    );
    String::from_utf8(bytes).expect("UTF-8 fixture")
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing string {key}"))
}

/// One plan as comparable facts: id → type, ordered children inside a
/// fragment, exchange → sending fragment roots, result root.
#[derive(Default)]
struct Plan {
    nodes: BTreeMap<u32, String>,
    children: BTreeMap<u32, Vec<u32>>,
    exchanges: BTreeSet<(u32, u32)>,
    root: Option<u32>,
}

fn is_exchange(name: &str) -> bool {
    matches!(name, "EXCHANGE" | "MERGE_EXCHANGE")
}

fn runtime_plan(profile: &str) -> Plan {
    let topology = profile
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("- Topology: "))
        .expect("runtime Profile Topology");
    let topology: Value = serde_json::from_str(topology).expect("Topology JSON");
    let id = |value: &Value| u32::try_from(value.as_u64().expect("id")).expect("u32 id");
    let mut plan = Plan {
        root: Some(id(&topology["rootId"])),
        ..Plan::default()
    };
    for node in topology["nodes"].as_array().expect("Topology nodes") {
        let node_id = id(&node["id"]);
        let name = str_of(node, "name").to_owned();
        let children: Vec<u32> = node["children"]
            .as_array()
            .expect("children")
            .iter()
            .map(id)
            .collect();
        if is_exchange(&name) {
            plan.exchanges
                .extend(children.iter().map(|child| (node_id, *child)));
        } else if !children.is_empty() {
            plan.children.insert(node_id, children);
        }
        assert!(
            plan.nodes.insert(node_id, name).is_none(),
            "Topology node {node_id} twice"
        );
    }
    plan
}

fn explain_plan(graph: &GraphView) -> (Plan, Vec<String>) {
    let mut plan = Plan::default();
    let mut unmapped = Vec::new();
    let mut plan_id_of = BTreeMap::new();
    for node in &graph.nodes {
        let plan_node_id: u32 = node
            .plan_node_id
            .as_deref()
            .expect("plan node id")
            .parse()
            .expect("numeric plan node id");
        let explain_type = node.plan_node_type.as_deref().expect("plan node type");
        let name = runtime_name(explain_type).unwrap_or_else(|| {
            unmapped.push(explain_type.to_owned());
            explain_type
        });
        plan_id_of.insert(node.node_id.clone(), plan_node_id);
        assert!(
            plan.nodes.insert(plan_node_id, name.to_owned()).is_none(),
            "EXPLAIN node {plan_node_id} twice"
        );
    }
    let mut edges: Vec<_> = graph.edges.iter().collect();
    edges.sort_by_key(|edge| edge.source_ordinal);
    for edge in edges {
        let (from, to) = (plan_id_of[&edge.from], plan_id_of[&edge.to]);
        match (&edge.relation, &edge.direction_semantics) {
            (GraphRelation::PlanChild, DirectionSemantics::ParentToChild) => {
                plan.children.entry(from).or_default().push(to);
            }
            // Data flows from the sending fragment root up to the exchange.
            (GraphRelation::ExchangeLink, DirectionSemantics::UpstreamToDownstream) => {
                plan.exchanges.insert((to, from));
            }
            other => panic!("unexpected EXPLAIN edge {other:?}"),
        }
    }
    if let [root] = graph.root_node_ids.as_slice() {
        plan.root = plan_id_of.get(root).copied();
    }
    (plan, unmapped)
}

/// Every difference between the two plans, as `kind → details`.
fn plan_diff(runtime: &Plan, explain: &Plan) -> BTreeMap<&'static str, Value> {
    let mut diff = BTreeMap::new();
    let mut put = |kind: &'static str, items: Vec<Value>| {
        if !items.is_empty() {
            diff.insert(kind, Value::Array(items));
        }
    };
    put(
        "runtime_only_nodes",
        runtime
            .nodes
            .iter()
            .filter(|(id, _)| !explain.nodes.contains_key(id))
            .map(|(id, name)| json!({"id": id, "name": name}))
            .collect(),
    );
    put(
        "explain_only_nodes",
        explain
            .nodes
            .iter()
            .filter(|(id, _)| !runtime.nodes.contains_key(id))
            .map(|(id, name)| json!({"id": id, "name": name}))
            .collect(),
    );
    put(
        "type_mismatches",
        runtime
            .nodes
            .iter()
            .filter_map(|(id, name)| {
                let ours = explain.nodes.get(id)?;
                (ours != name).then(|| json!({"id": id, "runtime": name, "explain": ours}))
            })
            .collect(),
    );
    let parents: BTreeSet<u32> = runtime
        .children
        .keys()
        .chain(explain.children.keys())
        .copied()
        .collect();
    put(
        "child_mismatches",
        parents
            .into_iter()
            .filter_map(|parent| {
                let theirs = runtime.children.get(&parent).cloned().unwrap_or_default();
                let ours = explain.children.get(&parent).cloned().unwrap_or_default();
                (theirs != ours)
                    .then(|| json!({"parent": parent, "runtime": theirs, "explain": ours}))
            })
            .collect(),
    );
    put(
        "runtime_only_exchange_links",
        runtime
            .exchanges
            .difference(&explain.exchanges)
            .map(|(exchange, root)| json!({"exchange": exchange, "sender_root": root}))
            .collect(),
    );
    put(
        "explain_only_exchange_links",
        explain
            .exchanges
            .difference(&runtime.exchanges)
            .map(|(exchange, root)| json!({"exchange": exchange, "sender_root": root}))
            .collect(),
    );
    if runtime.root != explain.root {
        put(
            "result_root",
            vec![json!({"runtime": runtime.root, "explain": explain.root})],
        );
    }
    diff
}

/// Per-case comparison of one corpus, accumulated into `report` / `observed`.
fn compare_corpus(
    corpus: &str,
    report: &mut BTreeMap<String, Value>,
    observed: &mut BTreeSet<(String, &'static str)>,
    totals: &mut [usize; 3],
) {
    let manifest = read_json(explain_root(corpus).join("manifest.json"));
    let runtime_manifest = read_json(runtime_root(corpus).join("manifest.json"));
    assert!(str_of(&manifest, "server_version").starts_with("3.5.14-23a56ec"));
    assert_eq!(
        str_of(&manifest, "image_digest"),
        str_of(&runtime_manifest, "image_digest"),
        "EXPLAIN and runtime corpus share one fixed-digest image"
    );
    let runtime_cases: BTreeMap<&str, &Value> = runtime_manifest["cases"]
        .as_array()
        .expect("runtime cases")
        .iter()
        .map(|case| (str_of(case, "case_id"), case))
        .collect();
    let cases = manifest["cases"].as_array().expect("cases");
    assert_eq!(
        cases.len(),
        runtime_cases.len(),
        "{corpus}: every runtime case has EXPLAIN"
    );
    for case in cases {
        let case_id = str_of(case, "case_id");
        let runtime_case = runtime_cases[case_id];
        assert_eq!(
            str_of(case, "sql_sha256"),
            str_of(runtime_case, "sql_sha256"),
            "{case_id}: EXPLAIN captured for the runtime SQL"
        );
        let text = read_verified(
            explain_root(corpus).join(str_of(case, "explain_file")),
            str_of(case, "explain_sha256"),
        );
        let profile = read_verified(
            runtime_root(corpus).join(str_of(runtime_case, "profile_file")),
            str_of(runtime_case, "profile_sha256"),
        );
        let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
            rows: text
                .split_terminator('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
                .collect(),
            detected_version: "3.5.14-23a56ec".to_owned(),
            default_database: Some("oracle_corpus".to_owned()),
        });
        let graph = &snapshot.graph_view;
        let (explain, unmapped) = explain_plan(graph);
        assert!(
            unmapped.is_empty(),
            "{case_id}: unmapped types {unmapped:?}"
        );
        let runtime = runtime_plan(&profile);
        let mut diff = plan_diff(&runtime, &explain);
        if graph.state != GraphState::Complete
            || !graph.completeness.reasons.is_empty()
            || !graph.unresolved_relations.is_empty()
        {
            diff.insert(
                "explain_graph_incomplete",
                json!({
                    "state": graph.state,
                    "limitations": graph.completeness.reasons,
                    "unresolved_relations": graph.unresolved_relations.len(),
                }),
            );
        }
        observed.extend(diff.keys().map(|kind| (case_id.to_owned(), *kind)));
        totals[0] += runtime.nodes.len();
        totals[1] += runtime.children.values().map(Vec::len).sum::<usize>();
        totals[2] += runtime.exchanges.len();
        let previous = report.insert(
            case_id.to_owned(),
            json!({
                "corpus": corpus,
                "plan_nodes": runtime.nodes.len(),
                "plan_child_edges": runtime.children.values().map(Vec::len).sum::<usize>(),
                "exchange_links": runtime.exchanges.len(),
                "diff": diff,
            }),
        );
        assert!(previous.is_none(), "{case_id} in two corpora");
    }
}

#[test]
fn explain_costs_graph_matches_runtime_topology_on_every_corpus_case() {
    let mut report = BTreeMap::new();
    let mut observed = BTreeSet::new();
    let mut totals = [0usize; 3];
    for corpus in CORPORA {
        compare_corpus(corpus, &mut report, &mut observed, &mut totals);
    }
    if env::var_os("EXPLAIN_TOPOLOGY_ORACLE_REPORT").is_some() {
        println!("{}", serde_json::to_string_pretty(&report).expect("report"));
        println!(
            "cases={} plan_nodes={} plan_child_edges={} exchange_links={}",
            report.len(),
            totals[0],
            totals[1],
            totals[2]
        );
    }
    let declared: BTreeSet<(String, &str)> = KNOWN_DIFFERENCES
        .iter()
        .map(|(case, kind, _)| ((*case).to_owned(), *kind))
        .collect();
    assert_eq!(
        observed,
        declared,
        "EXPLAIN vs runtime Topology differences must be fixed or declared with their official reason:\n{}",
        serde_json::to_string_pretty(&report).expect("report")
    );
}

/// Text-side facts of one EXPLAIN COSTS print, read without the product
/// grammar: plan node ids from `N:Label` headers (including a join's second
/// child, printed after `|----`) and the exchange ids that fragment sinks name
/// (`EXCHANGE ID: N`).
fn text_facts(text: &str) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let mut nodes = BTreeSet::new();
    let mut sinks = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim_start_matches([' ', '|', '-']);
        if let Some(rest) = trimmed.strip_prefix("EXCHANGE ID: ") {
            if let Ok(id) = rest.trim().parse() {
                sinks.insert(id);
            }
            continue;
        }
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        let after = &trimmed[digits..];
        if digits > 0
            && after.starts_with(':')
            && after[1..].starts_with(|c: char| c.is_ascii_uppercase())
            && let Ok(id) = trimmed[..digits].parse()
        {
            nodes.insert(id);
        }
    }
    (nodes, sinks)
}

/// One external capture: the EXPLAIN graph's completeness, its agreement with
/// the print's own node ids and exchange sinks, and, when the same capture
/// has a runtime Profile, the same comparison as the corpus oracle.
fn external_case(text: &str, profile: Option<&str>) -> Value {
    let snapshot = assemble_explain_snapshot(ExplainSnapshotInput {
        rows: text.lines().map(str::to_owned).collect(),
        detected_version: "3.5.14-23a56ec".to_owned(),
        default_database: None,
    });
    let graph = &snapshot.graph_view;
    let (text_nodes, text_sinks) = text_facts(text);
    let graph_ids: BTreeSet<u32> = graph
        .nodes
        .iter()
        .filter_map(|node| node.plan_node_id.as_deref()?.parse().ok())
        .collect();
    let (explain, unmapped) = explain_plan(graph);
    let linked: BTreeSet<u32> = explain
        .exchanges
        .iter()
        .map(|(exchange, _)| *exchange)
        .collect();
    let mut out = json!({
        "state": graph.state,
        "limitations": graph.completeness.reasons,
        "unresolved_relations": graph.unresolved_relations.len(),
        "nodes": graph.nodes.len(),
        "edges": graph.edges.len(),
        "text_node_ids_not_in_graph": text_nodes.difference(&graph_ids).collect::<Vec<_>>(),
        "graph_node_ids_not_in_text": graph_ids.difference(&text_nodes).collect::<Vec<_>>(),
        "sink_exchange_ids_without_link": text_sinks.difference(&linked).collect::<Vec<_>>(),
        "unmapped_types": unmapped,
    });
    if let Some(profile) = profile {
        let runtime = runtime_plan(profile);
        out["paired"] = json!({
            "runtime_nodes": runtime.nodes.len(),
            "runtime_child_edges": runtime.children.values().map(Vec::len).sum::<usize>(),
            "runtime_exchange_links": runtime.exchanges.len(),
            "diff": plan_diff(&runtime, &explain),
        });
    }
    out
}

/// Production captures kept outside the repository (user-supplied StarRocks
/// 3.5.14 EXPLAIN COSTS prints, some with the runtime Profile of the same
/// query). `EXPLAIN_TOPOLOGY_EXTERNAL_DIR` holds `<case>.explain.txt` (one
/// EXPLAIN row per line) and optional `<case>.meta.json` ({"kind"}); with
/// `EXPLAIN_TOPOLOGY_EXTERNAL_PROFILES`, a `<case>.profile` there is compared
/// exactly like the corpus oracle. Prints counts, plan node ids, operator
/// types and difference kinds only, never plan text. It reports and does not
/// assert: external captures are evidence, not fixtures.
#[test]
#[ignore = "needs local production captures; see EXPLAIN_TOPOLOGY_EXTERNAL_DIR"]
fn external_production_captures_report() {
    let Some(dir) = env::var_os("EXPLAIN_TOPOLOGY_EXTERNAL_DIR").map(PathBuf::from) else {
        return;
    };
    let profiles = env::var_os("EXPLAIN_TOPOLOGY_EXTERNAL_PROFILES").map(PathBuf::from);
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("external dir")
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.to_string_lossy().ends_with(".explain.txt"))
        .collect();
    paths.sort();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut cases = BTreeMap::new();
    for path in paths {
        let file = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let case = file.trim_end_matches(".explain.txt").to_owned();
        let kind = fs::read_to_string(dir.join(format!("{case}.meta.json")))
            .ok()
            .and_then(|meta| serde_json::from_str::<Value>(&meta).ok())
            .and_then(|meta| meta["kind"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        let text = fs::read_to_string(&path).expect("read explain");
        let profile = profiles
            .as_ref()
            .map(|root| root.join(format!("{case}.profile")))
            .filter(|path| path.exists())
            .map(|path| fs::read_to_string(path).expect("read profile"));
        let mut result = std::panic::catch_unwind(|| external_case(&text, profile.as_deref()))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                    .unwrap_or_default();
                json!({ "panic": message })
            });
        result["kind"] = json!(kind);
        cases.insert(case, result);
    }
    std::panic::set_hook(hook);
    println!("{}", serde_json::to_string_pretty(&cases).expect("report"));
}
