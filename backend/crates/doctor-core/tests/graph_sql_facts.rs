use doctor_contracts::{GraphSqlFactKind, RequestedSourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, resolve_version_and_graph,
    v01_analysis_receipt,
};
const CORPUS: &str =
    include_str!("../../../../fixtures/runtime/3.5.14/docker_runtime_corpus_complex.profile.txt");
fn tables(raw: &str) -> Vec<String> {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, raw.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).unwrap();
    let mut result = vec![];
    for node in &output.ir.graph.nodes {
        for fact in node.sql_facts.iter().flatten() {
            if fact.kind != GraphSqlFactKind::Table {
                continue;
            }
            assert_eq!(node.plan_node_type.as_deref(), Some("OLAP_SCAN"));
            assert!(!fact.source_refs.is_empty());
            for reference in &fact.source_refs {
                assert!(
                    output
                        .source_refs
                        .iter()
                        .any(|source| &source.ref_id == reference)
                );
            }
            result.push(fact.value.clone());
        }
    }
    result.sort();
    result
}
#[test]
fn explicit_runtime_scan_tables_have_provenance() {
    assert_eq!(
        tables(CORPUS),
        ["customers", "products", "returns", "sales"]
    );
}
#[test]
fn missing_wrong_scope_and_conflicting_tables_are_not_guessed() {
    assert!(tables(&CORPUS.replace("- Table:", "- IgnoredTable:")).is_empty());
    assert_eq!(
        tables(&CORPUS.replace(
            "- Table: products",
            "- Table: products\n             - Table: conflicting"
        )),
        ["customers", "returns", "sales"]
    );
    assert_eq!(
        tables(&CORPUS.replace("- Table: products", "- Table:")),
        ["customers", "returns", "sales"]
    );
    assert!(tables(&CORPUS.replace("UniqueMetrics:", "CommonMetrics:")).is_empty());
}
#[test]
#[ignore = "requires DOCTOR_LIVE_PROFILE_PATH fetched from an existing live query"]
fn supplied_live_profile_tables() {
    let path = std::env::var("DOCTOR_LIVE_PROFILE_PATH").expect("live Profile path");
    let actual = tables(&std::fs::read_to_string(path).unwrap());
    assert!(!actual.is_empty());
    eprintln!("live explicit tables: {actual:?}");
}

fn contexts(raw: &str) -> Vec<(GraphSqlFactKind, String)> {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, raw.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).unwrap();
    output
        .ir
        .graph
        .nodes
        .iter()
        .flat_map(|node| node.sql_facts.iter().flatten())
        .filter(|fact| fact.kind != GraphSqlFactKind::Table)
        .map(|fact| {
            assert!(fact.source_refs.iter().all(|reference| {
                output
                    .source_refs
                    .iter()
                    .any(|source| &source.ref_id == reference)
            }));
            (fact.kind.clone(), fact.value.clone())
        })
        .collect()
}
#[test]
fn runtime_predicates_are_source_bound_and_not_reconstructed() {
    let facts = contexts(CORPUS);
    assert!(
        facts
            .iter()
            .any(|(kind, value)| *kind == GraphSqlFactKind::Predicate
                && value.contains("product_id"))
    );
    assert!(
        facts
            .iter()
            .any(|(kind, value)| *kind == GraphSqlFactKind::JoinCondition
                && value.contains("customer_id"))
    );
    assert!(contexts(&CORPUS.replace("UniqueMetrics:", "CommonMetrics:")).is_empty());
    assert!(
        contexts(
            &CORPUS
                .replace("- Predicates:", "- IgnoredPredicates:")
                .replace("- JoinPredicates:", "- IgnoredJoinPredicates:")
        )
        .is_empty()
    );
    assert!(
        contexts(
            &CORPUS
                .replace("- Predicates:", "- Predicates: ...")
                .replace("- JoinPredicates:", "- JoinPredicates: ...")
        )
        .is_empty()
    );
}

#[test]
fn conflicting_runtime_predicate_is_not_selected_arbitrarily() {
    let expression = "12: product_id >= 1, 12: product_id < 5001";
    let conflicting = CORPUS.replace(
        &format!("- Predicates: {expression}"),
        &format!("- Predicates: {expression}\n             - Predicates: conflicting"),
    );
    assert!(
        !contexts(&conflicting)
            .iter()
            .any(|(_, value)| value == expression || value == "conflicting")
    );
}
