use std::{collections::BTreeMap, fs, path::Path};

use doctor_contracts::{AnalysisSnapshotReport, ReceiptComponent, SourceKind, canonical_hash};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, MetricDefinition, MetricDimension, adapt_input,
    analysis_key, assemble_public_report, evaluate_rules, facts_hash, report_facts,
    resolve_version_and_graph, sr3514_metric_registry,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const PACK_PATH: &str = "backend/crates/version-packs/starrocks_3_5_14/version-pack.json";
const RULESET_PATH: &str = "backend/crates/rule-sets/doctor_v0_1/ruleset.json";
const COMPARATOR_MANIFEST_PATH: &str = "contracts/manifests/doctor-comparator-v0.1.json";
const COMPARE_CASES_PATH: &str = "fixtures/expected/3.5.14/sr3514_compare_contract_cases.json";
const FIXTURE_MANIFEST_PATH: &str = "fixtures/manifests/sr3514-contract-fixtures.json";
const COMPONENT_MANIFEST_PATH: &str = "contracts/manifests/task01-analysis-components.json";
const METRIC_CASES_PATH: &str = "fixtures/expected/3.5.14/sr3514_metric_contract_cases.json";
const GRAPH_CASES_PATH: &str = "fixtures/expected/3.5.14/sr3514_graph_contract_cases.json";
const ORDER_CASES_PATH: &str = "fixtures/expected/3.5.14/sr3514_order_recipe_contract_cases.json";
const RUNTIME_PROFILE_TEXT_PATH: &str =
    "fixtures/runtime/3.5.14/docker_information_schema_columns.profile.txt";
const PRESENTATION_REPORT_PATH: &str =
    "fixtures/expected/3.5.14/sr3514_execution_graph.report.json";
const CROSS_FRAGMENT_MEMBERSHIP_FIXTURE_PATH: &str =
    "fixtures/synthetic/3.5.14/sr3514_cross_fragment_exchange.profile.txt";
const P04_COMPLEX_PROFILE_MANIFEST_PATH: &str =
    "fixtures/manifests/sr3514-complex-profile-regression.json";
const P04_COMPLEX_PROFILE_FIXTURES: [P04ComplexProfileFixture; 5] = [
    P04ComplexProfileFixture {
        fixture_id: "short-chain",
        tier: "short-chain",
        provenance: "synthetic",
        input_path: "fixtures/runtime-profiles/starrocks-3.5.14/p04/short-chain.profile.txt",
        report_path: "fixtures/expected/3.5.14/p04/short-chain.report.json",
        limitations: &["synthetic_short_chain"],
        private: false,
    },
    P04ComplexProfileFixture {
        fixture_id: "cross-fragment-degradation",
        tier: "cross-fragment",
        provenance: "synthetic",
        input_path: CROSS_FRAGMENT_MEMBERSHIP_FIXTURE_PATH,
        report_path: "fixtures/expected/3.5.14/p04/cross-fragment-degradation.report.json",
        limitations: &["same_plan_node_id_cannot_prove_cross_fragment_exchange_link"],
        private: false,
    },
    P04ComplexProfileFixture {
        fixture_id: "runtime-driver-occurrences",
        tier: "multi-pipeline-driver-occurrences",
        provenance: "sanitized-synthetic",
        input_path: "fixtures/runtime-profiles/starrocks-3.5.14/p04/runtime-driver-occurrences.profile.txt",
        report_path: "fixtures/expected/3.5.14/p04/runtime-driver-occurrences.report.json",
        limitations: &[
            "sanitized_synthetic_driver_occurrence_fixture",
            "execution_chains_require_independent_final_merged_evidence",
        ],
        private: false,
    },
    P04ComplexProfileFixture {
        fixture_id: "production-derived-multi-pipeline",
        tier: "multi-pipeline",
        provenance: "sanitized-production-derived",
        input_path: "fixtures/runtime-profiles/starrocks-3.5.14/p04/production-derived-multi-pipeline.profile.txt",
        report_path: "fixtures/expected/3.5.14/p04/production-derived-multi-pipeline.report.json",
        limitations: &["runtime_pipeline_relation_unavailable_without_verified_driver_chain"],
        private: true,
    },
    P04ComplexProfileFixture {
        fixture_id: "production-complex-synthetic",
        tier: "production-complex",
        provenance: "synthetic",
        input_path: "fixtures/runtime-profiles/starrocks-3.5.14/p04/production-complex-synthetic.profile.txt",
        report_path: "fixtures/expected/3.5.14/p04/production-complex-synthetic.report.json",
        limitations: &["synthetic_visual_stress_fixture_not_a_production_capture"],
        private: true,
    },
];
const REPORT_INPUT_PAIRS: [(&str, &str); 2] = [
    (
        "fixtures/expected/3.5.14/sr3514_minimal.report.json",
        "fixtures/synthetic/3.5.14/sr3514_minimal.json",
    ),
    (
        "fixtures/expected/3.5.14/sr3514_minimal_text.report.json",
        "fixtures/synthetic/3.5.14/sr3514_minimal.txt",
    ),
];

struct P04ComplexProfileFixture {
    fixture_id: &'static str,
    tier: &'static str,
    provenance: &'static str,
    input_path: &'static str,
    report_path: &'static str,
    limitations: &'static [&'static str],
    /// Production-derived captures stay in the private repository only; the
    /// public one has neither the profile nor its golden, so they are skipped.
    private: bool,
}

fn read_value(path: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path}: {error}")),
    )
    .unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

fn write_value(path: &str, value: &Value, check: bool) {
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("serialize JSON")
    );
    if fs::read_to_string(path).ok().as_deref() != Some(contents.as_str()) {
        if check {
            panic!("generated receipt is stale: {path}; run npm run contracts:generate");
        }
        fs::write(path, contents).unwrap_or_else(|error| panic!("write {path}: {error}"));
    }
}

fn object_without(value: &Value, excluded: &[&str]) -> Value {
    let mut object = value
        .as_object()
        .expect("manifest root is an object")
        .clone();
    for key in excluded {
        object.remove(*key);
    }
    Value::Object(object)
}

fn set_string(object: &mut Map<String, Value>, key: &str, value: String) {
    object.insert(key.to_owned(), Value::String(value));
}

fn set_material_hashes(component: &mut Map<String, Value>, values: &[(&str, String)]) {
    let material_hashes = values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String(value.clone())))
        .collect::<Map<String, Value>>();
    component.insert("material_hashes".to_owned(), Value::Object(material_hashes));
}

fn raw_sha256(path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path).unwrap_or_else(|error| panic!("read bytes {path}: {error}")));
    format!("{:x}", hasher.finalize())
}

fn metric_registry_entry(definition: MetricDefinition) -> Value {
    let (dimension, canonical_unit, fidelity_policy) = match definition.dimension {
        MetricDimension::Duration => ("duration", "ns", "preserve exact or conservative bounds"),
        MetricDimension::Bytes => (
            "bytes",
            "byte",
            "exact positive or conservative lower bound",
        ),
        MetricDimension::Count => ("count", "item", "prefer parenthetical raw exact count"),
        MetricDimension::Ratio => ("ratio", "ratio", "preserve exact decimal ratio"),
    };
    let ordered_source_contract = match definition.metric_id.as_str() {
        "QueryPlanningTime" => "Planner Total[1] text or Planner.Total JSON only",
        "QueryTotalTime" => "Summary Total text only",
        _ => "preserve_source_ordinal",
    };
    serde_json::json!({
        "analysis_aggregation": "non_additive",
        "canonical_unit": canonical_unit,
        "dimension": dimension,
        "fidelity_policy": fidelity_policy,
        "metric_id": definition.metric_id,
        "ordered_source_contract": ordered_source_contract,
        "valid_scopes": definition.valid_scopes,
    })
}

/// `OutputRows` is a VersionPack-derived value, not a source counter.  Keep it
/// out of the parser registry so a similarly named raw field can never be
/// mistaken for ExplainAnalyzer's recipe-selected output cardinality.
fn output_rows_registry_entry() -> Value {
    serde_json::json!({
        "analysis_aggregation": "non_additive",
        "canonical_unit": "item",
        "dimension": "count",
        "fidelity_policy": "exact only when a registered output-row recipe selects unambiguous exact inputs",
        "metric_id": "OutputRows",
        "ordered_source_contract": "registered 3.5.14 output-row recipe only",
        "valid_scopes": ["operator"],
    })
}

fn comparison_attempt_key_material(attempt: &Value) -> Value {
    match attempt.get("kind").and_then(Value::as_str) {
        Some("outcome") => serde_json::json!({
            "outcome_facts_hash": attempt.get("facts_hash").cloned().expect("outcome facts hash"),
            "error_code": Value::Null,
            "error_stage": Value::Null,
        }),
        Some("error") => {
            let error = attempt.get("error").expect("error summary");
            serde_json::json!({
                "outcome_facts_hash": Value::Null,
                "error_code": error.get("code").cloned().expect("error code"),
                "error_stage": error.get("stage").cloned().expect("error stage"),
            })
        }
        _ => panic!("comparison attempt kind"),
    }
}

fn refresh_compare_cases(comparator_manifest: &Value, comparator_semantic_hash: &str, check: bool) {
    let comparator = comparator_manifest
        .as_object()
        .expect("comparator manifest object");
    let comparator_id = comparator
        .get("id")
        .and_then(Value::as_str)
        .expect("comparator id");
    let comparator_version = comparator
        .get("version")
        .and_then(Value::as_str)
        .expect("comparator version");
    let comparison_schema_version = comparator
        .get("comparison_schema_range")
        .and_then(Value::as_str)
        .expect("comparison schema version");
    let mut cases = read_value(COMPARE_CASES_PATH);
    for case in cases.as_array_mut().expect("comparison case array") {
        let expected = case
            .get_mut("expected")
            .and_then(Value::as_object_mut)
            .expect("comparison expected");
        let receipt = serde_json::json!({
            "components": [{
                "id": comparator_id,
                "version": comparator_version,
                "manifest_hash": comparator_semantic_hash,
            }],
        });
        expected.insert(
            "schema_version".to_owned(),
            Value::String(comparison_schema_version.to_owned()),
        );
        expected.insert("comparison_receipt".to_owned(), receipt.clone());
        expected
            .get_mut("finding_delta")
            .and_then(Value::as_object_mut)
            .expect("comparison finding delta")
            .entry("partial_evidence")
            .or_insert_with(|| serde_json::json!([]));
        let baseline = expected.get("baseline").expect("comparison baseline");
        let candidate = expected.get("candidate").expect("comparison candidate");
        let comparison_key = canonical_hash(&serde_json::json!({
            "schema_version": comparison_schema_version,
            "receipt": receipt,
            "baseline": comparison_attempt_key_material(baseline),
            "candidate": comparison_attempt_key_material(candidate),
        }));
        expected.insert("comparison_key".to_owned(), Value::String(comparison_key));
    }
    write_value(COMPARE_CASES_PATH, &cases, check);
}

fn p04_text_line_count(input: &[u8]) -> u32 {
    let text = std::str::from_utf8(input).expect("P04 text fixture is UTF-8");
    text.lines()
        .count()
        .try_into()
        .expect("P04 text fixture line count fits u32")
}

fn build_p04_complex_profile_report(
    fixture: &P04ComplexProfileFixture,
    receipt_components: &[ReceiptComponent],
) -> Value {
    let context = AdapterContext::new(doctor_contracts::AnalysisReceipt {
        components: receipt_components.to_vec(),
    })
    .expect("P04 receipt context");
    let input = fs::read(fixture.input_path).expect("P04 source fixture");
    let mut output = adapt_input(
        InputEnvelope::new(doctor_contracts::RequestedSourceKind::TextProfile, input),
        &AdapterLimits::default(),
        &context,
    )
    .expect("P04 fixture adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("P04 graph resolves");
    evaluate_rules(&mut output).expect("P04 rules evaluate");
    serde_json::to_value(assemble_public_report(output).expect("P04 public report"))
        .expect("P04 report JSON")
}

fn p04_metric_cell_state_counts<'a>(cells: impl Iterator<Item = &'a Value>) -> Value {
    let mut counts = BTreeMap::<String, u64>::new();
    for metric in cells {
        let Some(state) = metric["state"].as_str() else {
            continue;
        };
        *counts.entry(state.to_owned()).or_default() += 1;
    }
    serde_json::to_value(counts).expect("P04 metric state counts")
}

fn p04_operator_metric_state_counts(report: &Value) -> Value {
    p04_metric_cell_state_counts(
        report["operator_rows"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|row| row["metric_cells"].as_array().into_iter().flatten()),
    )
}

fn p04_derived_metric_state_counts(report: &Value) -> Value {
    p04_metric_cell_state_counts(
        report["derived_metric_cells"]
            .as_array()
            .into_iter()
            .flatten(),
    )
}

fn p04_graph_view_counts(report: &Value) -> Value {
    let graph = &report["graph_view"];
    serde_json::json!({
        "state": graph["state"],
        "topology_source": graph["topology_source"],
        "node_count": graph["nodes"].as_array().map_or(0, Vec::len),
        "edge_count": graph["edges"].as_array().map_or(0, Vec::len),
        "root_count": graph["root_node_ids"].as_array().map_or(0, Vec::len),
        "unconnected_count": graph["unconnected_node_ids"].as_array().map_or(0, Vec::len),
        "unresolved_count": graph["unresolved_relations"].as_array().map_or(0, Vec::len),
    })
}

fn p04_execution_summary_counts(report: &Value) -> Value {
    let summary = &report["execution_summary"];
    let nodes = summary["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let exact = |field: &str| {
        nodes
            .iter()
            .filter(|node| node[field]["state"] == "exact")
            .count()
    };
    serde_json::json!({
        "state": summary["state"],
        "reasons": summary["reasons"],
        "node_count": nodes.len(),
        "exact_time_count": exact("total_time"),
        "exact_rows_count": exact("output_rows"),
        "hotspot_count": nodes.iter().filter(|node| node["hotspot"] == "most_consuming" || node["hotspot"] == "second_most_consuming").count(),
    })
}

fn refresh_p04_complex_profile_goldens(receipt_components: &[ReceiptComponent], check: bool) {
    let mut manifest = read_value(P04_COMPLEX_PROFILE_MANIFEST_PATH);
    let tiers = P04_COMPLEX_PROFILE_FIXTURES
        .iter()
        .filter(|fixture| !fixture.private || Path::new(fixture.input_path).exists())
        .map(|fixture| {
            let report = build_p04_complex_profile_report(fixture, receipt_components);
            write_value(fixture.report_path, &report, check);
            let input = fs::read(fixture.input_path).expect("P04 source fixture bytes");
            let diagnostics = report["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            serde_json::json!({
                "fixture_id": fixture.fixture_id,
                "tier": fixture.tier,
                "provenance": fixture.provenance,
                "limitations": fixture.limitations,
                "input_path": fixture.input_path,
                "input_sha256": raw_sha256(fixture.input_path),
                "input_bytes": input.len(),
                "input_lines": p04_text_line_count(&input),
                "report_path": fixture.report_path,
                "report_canonical_sha256": canonical_hash(&report),
                "analysis_state": report["analysis_state"],
                "facts_hash": report["facts_hash"],
                "analysis_key": report["analysis_key"],
                "diagnostic_codes": diagnostics,
                "graph_view": p04_graph_view_counts(&report),
                "operator_metric_state_counts": p04_operator_metric_state_counts(&report),
                "derived_metric_state_counts": p04_derived_metric_state_counts(&report),
                "execution_summary": p04_execution_summary_counts(&report),
                "receipt_components": report["analysis_receipt"]["components"],
            })
        })
        .collect::<Vec<_>>();
    manifest["tiers"] = Value::Array(tiers);
    let manifest_hash = canonical_hash(&object_without(&manifest, &["manifest_hash"]));
    set_string(
        manifest
            .as_object_mut()
            .expect("P04 complex manifest object"),
        "manifest_hash",
        manifest_hash,
    );
    write_value(P04_COMPLEX_PROFILE_MANIFEST_PATH, &manifest, check);
}

fn main() {
    let check = std::env::args().any(|argument| argument == "--check");
    let mut pack = read_value(PACK_PATH);
    let registry_metrics = sr3514_metric_registry(SourceKind::JsonProfile)
        .into_iter()
        .map(metric_registry_entry)
        .chain(std::iter::once(output_rows_registry_entry()))
        .collect::<Vec<_>>();
    pack.get_mut("metric_registry")
        .and_then(Value::as_object_mut)
        .expect("metric registry object")
        .insert("metrics".to_owned(), Value::Array(registry_metrics));
    let pack_product_version = pack
        .get("product_pack_version")
        .and_then(Value::as_str)
        .expect("pack product version")
        .to_owned();
    let metric_registry_version = pack
        .get("metric_registry")
        .and_then(|registry| registry.get("version"))
        .and_then(Value::as_str)
        .expect("metric registry version")
        .to_owned();
    let metric_registry_id = pack
        .get("metric_registry")
        .and_then(|registry| registry.get("registry_id"))
        .and_then(Value::as_str)
        .expect("metric registry id");
    let applicable_registry = format!("{metric_registry_id}@{metric_registry_version}");
    let recipe = pack
        .get_mut("metric_recipe_manifest")
        .expect("recipe manifest");
    let recipe_hash = canonical_hash(&object_without(recipe, &["manifest_hash"]));
    set_string(
        recipe.as_object_mut().expect("recipe object"),
        "manifest_hash",
        recipe_hash,
    );

    let semantic_material = object_without(
        &pack,
        &[
            "manifest_hash",
            "semantic_manifest_hash",
            "golden_manifest_path",
            "golden_manifest_hash",
            "golden_manifest_hash_scope",
        ],
    );
    let pack_semantic_hash = canonical_hash(&semantic_material);
    set_string(
        pack.as_object_mut().expect("pack object"),
        "semantic_manifest_hash",
        pack_semantic_hash.clone(),
    );

    let mut ruleset = read_value(RULESET_PATH);
    ruleset["registry_requirements"] = serde_json::json!([applicable_registry]);
    let ruleset_version = ruleset
        .get("version")
        .and_then(Value::as_str)
        .expect("ruleset version")
        .to_owned();
    let ruleset_semantic_hash = canonical_hash(&object_without(
        &ruleset,
        &["manifest_hash", "semantic_manifest_hash"],
    ));
    set_string(
        ruleset.as_object_mut().expect("ruleset object"),
        "semantic_manifest_hash",
        ruleset_semantic_hash.clone(),
    );
    let ruleset_hash = canonical_hash(&object_without(&ruleset, &["manifest_hash"]));
    set_string(
        ruleset.as_object_mut().expect("ruleset object"),
        "manifest_hash",
        ruleset_hash,
    );
    write_value(RULESET_PATH, &ruleset, check);

    let mut comparator_manifest = read_value(COMPARATOR_MANIFEST_PATH);
    let comparator_semantic_hash = canonical_hash(&object_without(
        &comparator_manifest,
        &[
            "evidence",
            "status",
            "golden_case_ids",
            "manifest_hash",
            "semantic_manifest_hash",
        ],
    ));
    set_string(
        comparator_manifest
            .as_object_mut()
            .expect("comparator manifest object"),
        "semantic_manifest_hash",
        comparator_semantic_hash.clone(),
    );
    let comparator_manifest_hash =
        canonical_hash(&object_without(&comparator_manifest, &["manifest_hash"]));
    set_string(
        comparator_manifest
            .as_object_mut()
            .expect("comparator manifest object"),
        "manifest_hash",
        comparator_manifest_hash.clone(),
    );
    write_value(COMPARATOR_MANIFEST_PATH, &comparator_manifest, check);
    refresh_compare_cases(&comparator_manifest, &comparator_semantic_hash, check);

    let mut component_manifest = read_value(COMPONENT_MANIFEST_PATH);
    let metric_cases_hash = canonical_hash(&read_value(METRIC_CASES_PATH));
    let graph_cases_hash = canonical_hash(&read_value(GRAPH_CASES_PATH));
    let order_cases_hash = canonical_hash(&read_value(ORDER_CASES_PATH));
    let registry_hash = canonical_hash(pack.get("metric_registry").expect("metric registry"));
    let recipe_semantic_hash = canonical_hash(&object_without(
        pack.get("metric_recipe_manifest")
            .expect("metric recipe manifest"),
        &["manifest_hash"],
    ));
    let runtime_profile_text_hash = raw_sha256(RUNTIME_PROFILE_TEXT_PATH);
    let cross_fragment_membership_fixture_hash = raw_sha256(CROSS_FRAGMENT_MEMBERSHIP_FIXTURE_PATH);
    let adapter_contract_hash = canonical_hash(&serde_json::json!({
        "accepted_source_kinds": ["json_profile", "text_profile"],
        "equivalence": "canonical_facts_exclude_source_view_and_provenance",
        "official_formatter_shapes": ["RuntimeProfile.JsonProfileFormatter", "RuntimeProfile.prettyPrint"],
        "runtime_profile_fixture_sha256": runtime_profile_text_hash,
        "execution_summary": {
            "recipe": doctor_contracts::EXECUTION_SUMMARY_RECIPE_ID,
            "operator_role": "absent CommonMetrics.IsSubordinate is native (ExplainAnalyzer.NodeIterator)",
            "time": "sum max(OperatorTotalTime) native+subordinate, plus first native NetworkTime or ScanTime",
            "share": "QueryCumulativeOperatorTime; strict >30% most, >15% second",
            "rows": "version-pack OutputRows recipe, producing (non-sink) fragment group"
        },
        "physical_operator_membership": {
            "identity": "typed_pipeline_container_reference_captured_at_parse_time",
            "occurrence": "source_order_per_fragment_pipeline",
            "missing": "explicit_none_no_opaque_id_inference",
            "drivers": "share_enclosing_pipeline_container_not_plan_group_first_pipeline"
        },
        "public_evidence_refs": {
            "rule_input_projection": "observation_id_to_metric_cell_ref",
            "missing_or_ambiguous": "projection_error",
            "collision_suffix": "canonical_observation_id_hash"
        },
        "fixture_only": false
    }));
    let mut receipt_components = Vec::new();
    for component in component_manifest
        .get_mut("components")
        .and_then(Value::as_array_mut)
        .expect("analysis components")
    {
        let component = component
            .as_object_mut()
            .expect("analysis component object");
        let id = component
            .get("id")
            .and_then(Value::as_str)
            .expect("component id")
            .to_owned();
        match id.as_str() {
            "sr3514-runtime-profile-adapter" => set_material_hashes(
                component,
                &[
                    ("adapter_contract", adapter_contract_hash.clone()),
                    ("runtime_profile_text", runtime_profile_text_hash.clone()),
                ],
            ),
            "strict-metric-parser-contract" => {
                set_material_hashes(component, &[("metric_cases", metric_cases_hash.clone())])
            }
            "sr3514-metric-registry" => {
                set_material_hashes(component, &[("metric_registry", registry_hash.clone())]);
                set_string(component, "version", metric_registry_version.clone());
            }
            "sr3514-graph-resolver-contract" => {
                set_string(component, "version", "0.10.0".to_owned());
                set_string(
                    component,
                    "implementation_status",
                    "implemented_pending_e2e".to_owned(),
                );
                // Evidence paths are provenance, not inputs: they are kept as
                // listed even where a checkout (the public one) lacks the
                // research documents, so the component identity stays the same.
                let evidence_paths = component
                    .get_mut("evidence_paths")
                    .and_then(Value::as_array_mut)
                    .expect("graph evidence paths");
                evidence_paths.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
                component.remove("material_hashes");
                set_material_hashes(
                    component,
                    &[
                        ("graph_cases", graph_cases_hash.clone()),
                        ("order_recipe_cases", order_cases_hash.clone()),
                        ("recipe_manifest", recipe_semantic_hash.clone()),
                        (
                            "cross_fragment_membership_fixture",
                            cross_fragment_membership_fixture_hash.clone(),
                        ),
                    ],
                );
            }
            "starrocks-3.5.14" => {
                set_material_hashes(
                    component,
                    &[("version_pack_semantic", pack_semantic_hash.clone())],
                );
                set_string(component, "version", pack_product_version.clone());
            }
            "doctor-v0.1" => {
                set_material_hashes(
                    component,
                    &[("rule_set_semantic", ruleset_semantic_hash.clone())],
                );
                set_string(component, "version", ruleset_version.clone());
            }
            _ => panic!("unknown analysis component: {id}"),
        }
        let component_hash = canonical_hash(&object_without(
            &Value::Object(component.clone()),
            &["manifest_hash"],
        ));
        set_string(component, "manifest_hash", component_hash.clone());
        let receipt_hash = match id.as_str() {
            "starrocks-3.5.14" => pack_semantic_hash.clone(),
            "doctor-v0.1" => ruleset_semantic_hash.clone(),
            _ => component_hash,
        };
        receipt_components.push(ReceiptComponent {
            id,
            version: component
                .get("version")
                .and_then(Value::as_str)
                .expect("component version")
                .to_owned(),
            manifest_hash: receipt_hash,
        });
    }
    let component_manifest_hash =
        canonical_hash(&object_without(&component_manifest, &["manifest_hash"]));
    set_string(
        component_manifest
            .as_object_mut()
            .expect("component manifest object"),
        "manifest_hash",
        component_manifest_hash.clone(),
    );
    write_value(COMPONENT_MANIFEST_PATH, &component_manifest, check);

    for (path, input_path) in REPORT_INPUT_PAIRS {
        let fixture = read_value(path);
        let mut report: AnalysisSnapshotReport = serde_json::from_value(fixture)
            .unwrap_or_else(|error| panic!("typed report {path}: {error}"));
        let input_bytes =
            fs::read(input_path).unwrap_or_else(|error| panic!("read {input_path}: {error}"));
        report.input.byte_length = input_bytes
            .len()
            .try_into()
            .expect("fixture byte length fits u32");
        report.input.source_hash = raw_sha256(input_path);
        if report.input.detected_kind == doctor_contracts::SourceKind::TextProfile {
            let input_text = String::from_utf8(input_bytes).expect("text fixture is UTF-8");
            report.input.line_count = Some(
                input_text
                    .lines()
                    .count()
                    .try_into()
                    .expect("fixture line count fits u32"),
            );
        } else {
            report.input.line_count = None;
        }
        report
            .analysis_receipt
            .components
            .clone_from(&receipt_components);
        report.schema_version = doctor_contracts::SCHEMA_VERSION.to_owned();
        if let Some(selected_pack) = &mut report.version.selected_pack {
            selected_pack
                .product_pack_version
                .clone_from(&pack_product_version);
            selected_pack.manifest_hash.clone_from(&pack_semantic_hash);
        }
        let options = BTreeMap::from([
            ("requested_kind".to_owned(), "auto".to_owned()),
            ("version_hint".to_owned(), "auto".to_owned()),
        ]);
        report.analysis_key = analysis_key(
            &report.input.source_hash,
            &report.analysis_receipt,
            &options,
        );
        report.facts_hash = facts_hash(&report_facts(&report));
        write_value(
            path,
            &serde_json::to_value(report).expect("report value"),
            check,
        );
    }

    // Unlike the two historical synthetic examples, this is a real production
    // assembly replay of the retained Docker-captured input, not a new live E2E.
    let context = AdapterContext::new(doctor_contracts::AnalysisReceipt {
        components: receipt_components.clone(),
    })
    .expect("generated receipt context");
    let runtime_input = fs::read(RUNTIME_PROFILE_TEXT_PATH).expect("runtime source fixture");
    let mut output = adapt_input(
        InputEnvelope::new(
            doctor_contracts::RequestedSourceKind::TextProfile,
            runtime_input,
        ),
        &AdapterLimits::default(),
        &context,
    )
    .expect("runtime fixture adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("runtime graph resolves");
    evaluate_rules(&mut output).expect("runtime rules evaluate");
    let runtime_report = assemble_public_report(output).expect("runtime presentation projects");
    write_value(
        PRESENTATION_REPORT_PATH,
        &serde_json::to_value(runtime_report).expect("runtime report JSON"),
        check,
    );
    write_value(
        "fixtures/manifests/sr3514-execution-graph.json",
        &serde_json::json!({
            "fixture_id":"sr3514_execution_graph",
            "evidence":"offline_production_replay_of_retained_docker_capture_not_new_live_e2e",
            "source_path":RUNTIME_PROFILE_TEXT_PATH,"source_sha256":raw_sha256(RUNTIME_PROFILE_TEXT_PATH),
            "report_path":PRESENTATION_REPORT_PATH,"report_sha256":raw_sha256(PRESENTATION_REPORT_PATH)
        }),
        check,
    );

    refresh_p04_complex_profile_goldens(&receipt_components, check);

    let mut fixture_manifest = read_value(FIXTURE_MANIFEST_PATH);
    set_string(
        fixture_manifest
            .get_mut("analysis_component_manifest")
            .and_then(Value::as_object_mut)
            .expect("analysis component manifest link"),
        "sha256",
        component_manifest_hash,
    );
    set_string(
        fixture_manifest
            .get_mut("comparison_component_manifest")
            .and_then(Value::as_object_mut)
            .expect("comparison component manifest link"),
        "sha256",
        comparator_manifest_hash,
    );
    for fixture in fixture_manifest
        .get_mut("fixtures")
        .and_then(Value::as_array_mut)
        .expect("fixtures array")
    {
        let fixture = fixture.as_object_mut().expect("fixture object");
        set_string(fixture, "applicable_registry", applicable_registry.clone());
        let input_path = fixture
            .get("input_path")
            .and_then(Value::as_str)
            .expect("input path")
            .to_owned();
        let expected_path = fixture
            .get("expected_path")
            .and_then(Value::as_str)
            .expect("expected path")
            .to_owned();
        set_string(fixture, "input_sha256", raw_sha256(&input_path));
        set_string(
            fixture,
            "expected_sha256",
            canonical_hash(&read_value(&expected_path)),
        );
    }
    for table in fixture_manifest
        .get_mut("contract_case_tables")
        .and_then(Value::as_array_mut)
        .expect("case tables")
    {
        let table = table.as_object_mut().expect("case table object");
        let path = table
            .get("path")
            .and_then(Value::as_str)
            .expect("case table path")
            .to_owned();
        set_string(table, "sha256", canonical_hash(&read_value(&path)));
    }
    let fixture_manifest_hash =
        canonical_hash(&object_without(&fixture_manifest, &["manifest_hash"]));
    set_string(
        fixture_manifest
            .as_object_mut()
            .expect("fixture manifest object"),
        "manifest_hash",
        fixture_manifest_hash.clone(),
    );
    write_value(FIXTURE_MANIFEST_PATH, &fixture_manifest, check);

    set_string(
        pack.as_object_mut().expect("pack object"),
        "golden_manifest_hash",
        fixture_manifest_hash,
    );
    let pack_hash = canonical_hash(&object_without(&pack, &["manifest_hash"]));
    set_string(
        pack.as_object_mut().expect("pack object"),
        "manifest_hash",
        pack_hash,
    );
    write_value(PACK_PATH, &pack, check);

    for path in [
        PACK_PATH,
        RULESET_PATH,
        COMPARATOR_MANIFEST_PATH,
        COMPARE_CASES_PATH,
        FIXTURE_MANIFEST_PATH,
        COMPONENT_MANIFEST_PATH,
    ] {
        assert!(Path::new(path).is_file(), "receipt target missing: {path}");
    }
    for (path, _) in REPORT_INPUT_PAIRS {
        assert!(Path::new(path).is_file(), "receipt target missing: {path}");
    }
}
