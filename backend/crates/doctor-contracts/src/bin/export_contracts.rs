use std::{fs, path::Path};

use doctor_contracts::*;
use schemars::schema_for;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ts_rs::TS;

fn write_if_changed(path: &Path, contents: String, check: bool) {
    if fs::read_to_string(path).ok().as_deref() != Some(contents.as_str()) {
        if check {
            panic!(
                "generated contract is stale: {}; run npm run contracts:generate",
                path.display()
            );
        }
        fs::write(path, contents).expect("write generated contract");
    }
}

/// Keep the generated connected golden structurally complete when a new
/// comparison projection is introduced.  The fixture contains a few retained
/// synthetic Plan comparisons that are intentionally not rebuilt by this
/// generator; they have no runtime graph payload and are therefore explicitly
/// not comparable for the runtime projection only.
fn backfill_runtime_projection(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(backfill_runtime_projection),
        Value::Object(object) => {
            if let Some(Value::Object(structure_delta)) = object.get_mut("structure_delta")
                && !structure_delta.contains_key("runtime_operators")
            {
                structure_delta.insert(
                    "runtime_operators".to_owned(),
                    json!({
                        "state": "not_comparable",
                        "reasons": ["runtime_projection_missing"],
                    }),
                );
            }
            object.values_mut().for_each(backfill_runtime_projection);
        }
        _ => {}
    }
}

fn export_schema<T: schemars::JsonSchema>(name: &str, check: bool) {
    let schema = schema_for!(T);
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("schema JSON")
    );
    write_if_changed(
        &Path::new("contracts/schema").join(format!("{name}.schema.json")),
        contents,
        check,
    );
}

/// Analysis reports use an independently versioned wire contract.  Keep the
/// generated schema aligned with the runtime validator instead of leaving the
/// version field as an unconstrained string.
fn export_analysis_snapshot_schema(check: bool) {
    let mut schema = serde_json::to_value(schema_for!(AnalysisSnapshotReport))
        .expect("AnalysisSnapshotReport schema JSON value");
    *schema
        .pointer_mut("/properties/schema_version")
        .expect("AnalysisSnapshotReport schema has schema_version") = json!({
        "type": "string",
        "const": SCHEMA_VERSION,
    });
    let required = schema
        .pointer_mut("/required")
        .and_then(Value::as_array_mut)
        .expect("AnalysisSnapshotReport schema has required fields");
    if !required.iter().any(|field| field == "graph_store_v2") {
        required.push(Value::String("graph_store_v2".into()));
        required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    }
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("AnalysisSnapshotReport schema JSON")
    );
    write_if_changed(
        Path::new("contracts/schema/AnalysisSnapshotReport.schema.json"),
        contents,
        check,
    );
}

/// The Explain receipt binds to the complete generated root schema, including
/// every transitive definition.  The checked-in manifest is deliberately
/// generated from the same Schemars value, so `--check` catches a DTO change
/// before a stale receipt can be shipped.
fn export_explain_snapshot_schema(check: bool) {
    let schema = schema_for!(ExplainSnapshot);
    let schema_value = serde_json::to_value(&schema).expect("Explain schema JSON value");
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("Explain schema JSON")
    );
    write_if_changed(
        Path::new("contracts/schema/ExplainSnapshot.schema.json"),
        contents.clone(),
        check,
    );
    let definitions = schema_value
        .get("definitions")
        .expect("ExplainSnapshot has transitive schema definitions");
    let manifest = json!({
        "id": "doctor-explain-schema",
        "version": "0.4",
        "explain_schema_version": EXPLAIN_SCHEMA_VERSION,
        "schema_id": "ExplainSnapshot",
        "schema_content_sha256": sha256_bytes(contents.as_bytes()),
        "schema_definitions_canonical_sha256": canonical_hash(definitions),
        "surface": "connected_explain_modes_snapshot",
    });
    write_if_changed(
        Path::new("contracts/manifests/doctor-explain-schema-v0.4.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("Explain schema manifest JSON")
        ),
        check,
    );
}

fn constrain_export_tagged_objects(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            values.iter_mut().for_each(constrain_export_tagged_objects)
        }
        serde_json::Value::Object(values) => {
            if let Some(properties) = values
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                let strict_subject =
                    properties.contains_key("kind") && properties.contains_key("snapshot");
                let strict_hash = properties.contains_key("kind")
                    && (properties.contains_key("analysis_key")
                        || properties.contains_key("comparison_key"));
                let strict_attempt = properties.contains_key("state")
                    && (properties.contains_key("analysis_key")
                        || properties.contains_key("error_state"));
                if strict_subject || strict_hash || strict_attempt {
                    values.insert("additionalProperties".to_owned(), json!(false));
                }
            }
            values
                .values_mut()
                .for_each(constrain_export_tagged_objects);
        }
        _ => {}
    }
}

fn export_strict_schema<T: schemars::JsonSchema>(name: &str, check: bool) {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("strict export schema JSON");
    constrain_export_tagged_objects(&mut schema);
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("strict export schema JSON")
    );
    write_if_changed(
        &Path::new("contracts/schema").join(format!("{name}.schema.json")),
        contents,
        check,
    );
}

fn constrain_connected_schema_version(schema: &mut serde_json::Value) {
    if let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
        && properties.contains_key("schema_version")
    {
        properties.insert(
            "schema_version".to_owned(),
            json!({
                "type": "string",
                "const": CONNECTED_SCHEMA_VERSION,
            }),
        );
    }
    match schema {
        serde_json::Value::Array(values) => values
            .iter_mut()
            .for_each(constrain_connected_schema_version),
        serde_json::Value::Object(values) => values
            .values_mut()
            .for_each(constrain_connected_schema_version),
        _ => {}
    }
}

fn export_connected_schema<T: schemars::JsonSchema>(name: &str, check: bool) {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("connected schema JSON");
    constrain_connected_schema_version(&mut schema);
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("connected schema JSON")
    );
    write_if_changed(
        &Path::new("contracts/schema").join(format!("{name}.schema.json")),
        contents,
        check,
    );
}

fn export_artifact_schema<T: schemars::JsonSchema>(name: &str, check: bool) {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("export schema JSON");
    constrain_export_tagged_objects(&mut schema);
    if let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
        && properties.contains_key("schema_version")
    {
        properties.insert(
            "schema_version".to_owned(),
            json!({
                "type": "string",
                "const": EXPORT_SCHEMA_VERSION,
            }),
        );
    }
    if name == "ExportManifest" {
        if let Some(files) = schema
            .pointer_mut("/properties/files")
            .and_then(serde_json::Value::as_object_mut)
        {
            files.insert("minItems".to_owned(), json!(1));
            files.insert("maxItems".to_owned(), json!(1));
        }
        if let Some(bindings) = schema
            .pointer_mut("/properties/receipt_bindings")
            .and_then(serde_json::Value::as_object_mut)
        {
            bindings.insert("minItems".to_owned(), json!(1));
        }
    }
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("export schema JSON")
    );
    write_if_changed(
        &Path::new("contracts/schema").join(format!("{name}.schema.json")),
        contents,
        check,
    );
}

fn export_comparison_schema(check: bool) {
    let mut schema = serde_json::to_value(schema_for!(Comparison)).expect("Comparison schema JSON");
    *schema
        .pointer_mut("/properties/schema_version")
        .expect("Comparison schema_version") = json!({
        "type": "string",
        "const": COMPARISON_SCHEMA_VERSION,
    });
    let components = schema
        .pointer_mut("/definitions/ComparisonReceipt/properties/components")
        .and_then(serde_json::Value::as_object_mut)
        .expect("ComparisonReceipt components schema");
    components.insert("minItems".to_owned(), json!(1));
    components.insert("maxItems".to_owned(), json!(1));
    for (field, constraint) in [
        ("id", json!({"minLength": 1})),
        ("version", json!({"minLength": 1})),
        ("manifest_hash", json!({"pattern": "^[0-9A-Fa-f]{64}$"})),
    ] {
        schema
            .pointer_mut(&format!("/definitions/ReceiptComponent/properties/{field}"))
            .and_then(serde_json::Value::as_object_mut)
            .expect("ReceiptComponent field schema")
            .extend(
                constraint
                    .as_object()
                    .expect("ReceiptComponent constraint")
                    .clone(),
            );
    }
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&schema).expect("Comparison schema JSON")
    );
    write_if_changed(
        &Path::new("contracts/schema").join("Comparison.schema.json"),
        contents,
        check,
    );
}

fn export_ts<T: TS>(contents: &mut Vec<String>) {
    let declaration = T::decl().replacen("type ", "export type ", 1);
    contents.push(
        declaration
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn manifest_hash<T: Serialize>(value: &T) -> String {
    let mut value = serde_json::to_value(value).expect("manifest serializes");
    value
        .as_object_mut()
        .expect("manifest is an object")
        .remove("manifest_hash");
    canonical_hash(&value)
}

fn synthetic_receipt(id: &str, version: &str) -> ReceiptComponent {
    let semantic = json!({"id": id, "version": version, "status": "contract_only_not_ready"});
    ReceiptComponent {
        id: id.to_owned(),
        version: version.to_owned(),
        manifest_hash: canonical_hash(&semantic),
    }
}

fn binding(scope: ExportBindingScope, component: ReceiptComponent) -> ReceiptBinding {
    ReceiptBinding { scope, component }
}

fn redacted_report(path: &str) -> AnalysisSnapshotReport {
    let value: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("analysis report fixture"))
            .expect("analysis fixture JSON");
    let mut report: AnalysisSnapshotReport = serde_json::from_value(redact_fixture_sources(value))
        .expect("redacted analysis fixture follows Rust DTO");
    report
        .source_view
        .limitations
        .push("export_source_view_redacted".to_owned());
    report
}

// Fixture generation mirrors doctor-export without creating a crate dependency cycle.
// Runtime regression checks both successful comparison sides as well as analysis.
fn redact_fixture_sources(mut value: Value) -> Value {
    match &mut value {
        Value::Array(items) => {
            for item in items {
                *item = redact_fixture_sources(item.take());
            }
        }
        Value::Object(fields) => {
            for (name, field) in fields {
                match name.as_str() {
                    "filename" | "display_name" => *field = Value::Null,
                    "details" => *field = serde_json::json!({}),
                    "plain_text_lines"
                    | "json_tokens"
                    | "provenance_index"
                    | "source_display"
                    | "adapter_evidence"
                    | "source_refs"
                    | "evidence_refs"
                    | "input_observation_refs"
                    | "evidence" => *field = serde_json::json!([]),
                    "source_ref" => *field = Value::String("export:redacted".to_owned()),
                    _ => *field = redact_fixture_sources(field.take()),
                }
            }
        }
        _ => {}
    }
    value
}

fn export_manifest(
    kind: ExportKind,
    subject_hashes: SubjectHashes,
    report_bytes: &[u8],
    schema_id: &str,
    schema_version: &str,
    receipt_bindings: Vec<ReceiptBinding>,
) -> ExportManifest {
    let schema_path = Path::new("contracts/schema").join(format!("{schema_id}.schema.json"));
    let schema_hash = sha256_bytes(&fs::read(schema_path).expect("export subject schema"));
    let mut manifest = ExportManifest {
        schema_version: ExportSchemaVersion::V0_1,
        canonicalization: ExportCanonicalization::DoctorCanonicalJsonV1,
        integrity: ExportIntegrity::SelfConsistentNotSigned,
        kind,
        evidence: ExportEvidence::SyntheticContractOnlyNotDockerE2e,
        files: vec![ExportFile {
            path: "report.json".to_owned(),
            media_type: EXPORT_MEDIA_TYPE.to_owned(),
            byte_length: u32::try_from(report_bytes.len())
                .expect("synthetic report fits export byte-length contract"),
            sha256: sha256_bytes(report_bytes),
        }],
        schemas: vec![SchemaReceipt {
            schema_id: schema_id.to_owned(),
            version: schema_version.to_owned(),
            sha256: schema_hash,
        }],
        receipt_bindings,
        subject_hashes,
        manifest_hash: String::new(),
    };
    manifest.manifest_hash = manifest_hash(&manifest);
    manifest
}

fn failed_candidate_comparison(
    baseline: AnalysisSnapshotReport,
    comparator: ReceiptComponent,
) -> Comparison {
    Comparison {
        schema_version: COMPARISON_SCHEMA_VERSION.to_owned(),
        comparison_receipt: ComparisonReceipt {
            components: vec![comparator],
        },
        comparison_key: "comparison:synthetic-export-failed-candidate".to_owned(),
        baseline: AnalysisAttempt::Outcome(AnalysisOutcomeAttempt {
            outcome: AnalysisOutcome { report: baseline },
        }),
        candidate: AnalysisAttempt::Error(AnalysisErrorAttempt {
            error: ErrorSummary {
                code: "parse_failed".to_owned(),
                stage: "parse".to_owned(),
                message_id: "parse_failed".to_owned(),
                recoverability: "change_input".to_owned(),
                affected_side: Some("candidate".to_owned()),
            },
        }),
        state: ComparisonState::NotComparable,
        reasons: vec!["candidate_attempt_failed".to_owned()],
        compatibility_delta: Vec::new(),
        structure_delta: StructureDelta {
            state: "not_comparable".to_owned(),
            matches: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            changed: Vec::new(),
            edge_delta: Vec::new(),
            unresolved: Vec::new(),
        },
        metric_deltas: Vec::new(),
        finding_delta: FindingDelta {
            added: Vec::new(),
            resolved: Vec::new(),
            persisting: Vec::new(),
            changed: Vec::new(),
            incomparable: Vec::new(),
            algorithm_changed: Vec::new(),
            partial_evidence: Vec::new(),
            measurement_changes: Vec::new(),
        },
        diagnostics: Vec::new(),
        limitations: vec!["candidate_failed".to_owned()],
    }
}

/// Synthetic contract-only response for generated TypeScript consumers. Product
/// behavior is exercised in doctor-api, which fetches both reports and calls the
/// core comparator; this generator intentionally has no dependency on that
/// runtime crate.
fn synthetic_connected_profile_comparison(
    connection_id: &str,
    baseline_query_id: &str,
    candidate_query_id: &str,
    report: AnalysisSnapshotReport,
    comparator: ReceiptComponent,
) -> ConnectedProfileComparisonResponse {
    let comparison_receipt = ComparisonReceipt {
        components: vec![comparator],
    };
    let attempt_key = |facts_hash: &str| {
        json!({
            "outcome_facts_hash": facts_hash,
            "error_code": null,
            "error_stage": null,
        })
    };
    let comparison_key = canonical_hash(&json!({
        "schema_version": COMPARISON_SCHEMA_VERSION,
        "receipt": comparison_receipt.clone(),
        "baseline": attempt_key(&report.facts_hash),
        "candidate": attempt_key(&report.facts_hash),
    }));
    let side = |query_id: &str, input_origin: InputOrigin| ConnectedProfileComparisonSide {
        query: ConnectedQueryRef {
            connection_id: connection_id.to_owned(),
            profile_fetch: ConnectedProfileFetchAction::Available {
                reason: ProfileFetchAvailableReason::ProfileFetched,
            },
            plan_check: ConnectedPlanCheckAction::Unavailable {
                reason: PlanCheckUnavailableReason::QueryMetadataUnavailable,
            },
            query_id: query_id.to_owned(),
            discovery_source: None,
            discovery_generation: Some(1),
            observed_at_utc: None,
            observed_at_local: None,
            timestamp_state: ConnectedTimestampState::Unavailable,
            status: "unknown".to_owned(),
            digest: None,
            database: None,
            resource_group: None,
            query_time_ms: None,
            source_refs: vec!["profile:query_id".to_owned()],
        },
        input_origin,
    };
    ConnectedProfileComparisonResponse {
        baseline: side(baseline_query_id, InputOrigin::ConnectedProfileFetch),
        candidate: side(
            candidate_query_id,
            InputOrigin::CandidateProfileUserExternalRerun,
        ),
        comparison: Comparison {
            schema_version: COMPARISON_SCHEMA_VERSION.to_owned(),
            comparison_receipt,
            comparison_key,
            baseline: AnalysisAttempt::Outcome(AnalysisOutcomeAttempt {
                outcome: AnalysisOutcome {
                    report: report.clone(),
                },
            }),
            candidate: AnalysisAttempt::Outcome(AnalysisOutcomeAttempt {
                outcome: AnalysisOutcome { report },
            }),
            state: ComparisonState::PartiallyComparable,
            reasons: vec!["synthetic_contract_only_not_runtime_comparison".to_owned()],
            compatibility_delta: Vec::new(),
            structure_delta: StructureDelta {
                state: "unavailable".to_owned(),
                matches: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                changed: Vec::new(),
                edge_delta: Vec::new(),
                unresolved: vec!["synthetic_contract_only_not_runtime_comparison".to_owned()],
            },
            metric_deltas: Vec::new(),
            finding_delta: FindingDelta {
                added: Vec::new(),
                resolved: Vec::new(),
                persisting: Vec::new(),
                changed: Vec::new(),
                incomparable: Vec::new(),
                algorithm_changed: Vec::new(),
                partial_evidence: Vec::new(),
                measurement_changes: Vec::new(),
            },
            diagnostics: Vec::new(),
            limitations: vec!["synthetic_contract_only_not_runtime_comparison".to_owned()],
        },
        validation_context: ConnectedComparisonValidationContext {
            interpretation_state: ConnectedComparisonInterpretationState::ObservedOnly,
            same_connection_session: true,
            same_discovery_generation: true,
            fetch_order: "baseline_then_candidate".to_owned(),
            candidate_attestation: "user_external_controlled_rerun".to_owned(),
        },
        limitations: vec![
            "candidate_profile_user_external_rerun_is_user_responsibility".to_owned(),
            "raw_connector_profile_bytes_not_returned_by_connected_profile_comparison".to_owned(),
            "synthetic_contract_only_not_docker_e2e".to_owned(),
        ],
    }
}

fn explain_manifest_component(path: &str) -> ReceiptComponent {
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).expect("Explain component manifest"))
            .expect("Explain component manifest JSON");
    ReceiptComponent {
        id: manifest["id"]
            .as_str()
            .expect("Explain component ID")
            .to_owned(),
        version: manifest["version"]
            .as_str()
            .expect("Explain component version")
            .to_owned(),
        manifest_hash: canonical_hash(&manifest),
    }
}

/// Contract-only data for the generated connected fixture.  The real raw rows
/// belong to the API/core tests; this keeps the generated example from claiming
/// Docker evidence while still pinning the independently versioned shape.
fn synthetic_explain_snapshot() -> ExplainSnapshot {
    let plan_hash = canonical_hash(&Vec::<String>::new());
    let receipt = ExplainReceipt {
        schema: explain_manifest_component("contracts/manifests/doctor-explain-schema-v0.4.json"),
        adapter: explain_manifest_component(
            "contracts/manifests/sr3514-explain-costs-adapter.json",
        ),
        exchange_recipe: explain_manifest_component(
            "contracts/manifests/sr3514-explain-costs-exchange-recipe.json",
        ),
        card_facts_recipe: explain_manifest_component(
            "contracts/manifests/sr3514-explain-costs-card-facts-recipe.json",
        ),
        version_pack: Some(ReceiptComponent {
            id: "starrocks-3.5.14".to_owned(),
            version: "0.1.8".to_owned(),
            manifest_hash: "1c9c48730ae9de5ab3e81800f3304345a046e9a9a7414fd69b50cf8692f55fdf"
                .to_owned(),
        }),
    };
    let detected_version_hash = canonical_hash(&"3.5.14");
    let version = ExplainVersionContext {
        detected_release: Some("3.5.14".to_owned()),
        selected_pack: Some(SelectedVersionPack {
            pack_id: "starrocks-3.5.14".to_owned(),
            product_pack_version: "0.1.8".to_owned(),
            upstream_release: "3.5.14".to_owned(),
            upstream_commit: "23a56ec517db763ea9bc7ee50b19357acdee0303".to_owned(),
            manifest_hash: "1c9c48730ae9de5ab3e81800f3304345a046e9a9a7414fd69b50cf8692f55fdf"
                .to_owned(),
            release_deltas: None,
        }),
        pack_match: PackMatch::ExactPack,
        source_refs: vec![
            "explain:connection:detected_version".to_owned(),
            "explain:connection:default_database".to_owned(),
        ],
        limitations: vec!["default_database_fingerprint_not_comparability_proof".to_owned()],
    };
    let graph_view = GraphView {
        state: GraphState::Unavailable,
        topology_source: TopologySource::ExplainCosts,
        root_node_ids: Vec::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        unconnected_node_ids: Vec::new(),
        unresolved_relations: Vec::new(),
        source_refs: vec!["explain:result".to_owned()],
        completeness: DimensionCompleteness {
            state: CompletenessState::Unavailable,
            present_count: 0,
            expected_count: None,
            reasons: vec!["synthetic_contract_only_not_docker_e2e".to_owned()],
        },
    };
    let facts_hash = explain_facts_hash_v1(&graph_view, &[], &receipt);
    let snapshot_id = canonical_hash(&json!({
        "mode": ExplainMode::Costs,
        "plan_hash": plan_hash,
        "facts_hash": facts_hash,
        "receipt": receipt,
        "detected_version_evidence_hash": detected_version_hash,
        "default_database_evidence_hash": null,
    }));
    ExplainSnapshot {
        snapshot_id,
        explain_schema_version: ExplainSchemaVersion::V0_4,
        explain_level: ExplainLevel::Costs,
        input_origin: InputOrigin::ConnectedExplain,
        plan_hash,
        facts_hash,
        version,
        receipt,
        graph_view,
        runtime_state: ExplainRuntimeState::Unavailable,
        plan_node_fact_sets: Vec::new(),
        source_locators: vec![
            ExplainSourceLocator::Result {
                source_ref: "explain:result".to_owned(),
                row_count: 0,
                byte_length: 0,
            },
            ExplainSourceLocator::ConnectionMetadata {
                source_ref: "explain:connection:detected_version".to_owned(),
                field: "detected_version".to_owned(),
                availability: ExplainMetadataAvailability::Available,
                evidence_hash: Some(detected_version_hash),
            },
            ExplainSourceLocator::ConnectionMetadata {
                source_ref: "explain:connection:default_database".to_owned(),
                field: "default_database".to_owned(),
                availability: ExplainMetadataAvailability::Unavailable,
                evidence_hash: None,
            },
        ],
        source_view: ExplainSourceView {
            source_kind: ExplainSourceKind::ExplainCosts,
            lines: Vec::new(),
            display_state: ExplainSourceDisplayState::SafeStructureOnly,
            truncation_state: ExplainSourceTruncationState::NotTruncated,
            limitations: vec![
                "synthetic_contract_only_not_docker_e2e".to_owned(),
                "raw_explain_attributes_restricted".to_owned(),
            ],
        },
        diagnostics: Vec::new(),
        source_refs: vec![
            "explain:result".to_owned(),
            "explain:connection:detected_version".to_owned(),
            "explain:connection:default_database".to_owned(),
        ],
        limitations: vec![
            "synthetic_contract_only_not_docker_e2e".to_owned(),
            "explain_cannot_prove_runtime_improvement".to_owned(),
        ],
    }
}

fn write_export_fixture(path: &str, artifact: &ExportArtifact, check: bool) -> (String, String) {
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(artifact).expect("export artifact serializes")
    );
    write_if_changed(Path::new(path), contents.clone(), check);
    (path.to_owned(), sha256_bytes(contents.as_bytes()))
}

fn export_fixtures(check: bool) {
    let analysis_report = redacted_report("fixtures/expected/3.5.14/sr3514_minimal.report.json");
    let exporter = synthetic_receipt("doctor-export-v0.1", "0.1.0");
    let analysis_bindings =
        std::iter::once(binding(ExportBindingScope::Exporter, exporter.clone()))
            .chain(
                analysis_report
                    .analysis_receipt
                    .components
                    .iter()
                    .cloned()
                    .map(|component| binding(ExportBindingScope::Analysis, component)),
            )
            .collect::<Vec<_>>();
    let analysis_bytes = serde_json::to_vec(&analysis_report).expect("analysis report serializes");
    let analysis_hashes = SubjectHashes::Analysis {
        analysis_key: analysis_report.analysis_key.clone(),
        facts_hash: analysis_report.facts_hash.clone(),
    };
    let analysis_manifest = export_manifest(
        ExportKind::Analysis,
        analysis_hashes,
        &analysis_bytes,
        "AnalysisSnapshotReport",
        SCHEMA_VERSION,
        analysis_bindings,
    );
    let analysis_artifact = ExportArtifact {
        schema_version: ExportSchemaVersion::V0_1,
        subject: ExportSubject::Analysis {
            snapshot: analysis_report.clone(),
        },
        manifest: analysis_manifest,
    };
    let analysis_path = "fixtures/expected/3.5.14/sr3514_export_analysis.json";
    let (_, analysis_fixture_hash) = write_export_fixture(analysis_path, &analysis_artifact, check);

    let comparator_manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string("contracts/manifests/doctor-comparator-v0.1.json")
            .expect("comparator manifest"),
    )
    .expect("comparator manifest JSON");
    let comparator = ReceiptComponent {
        id: comparator_manifest["id"]
            .as_str()
            .expect("comparator id")
            .to_owned(),
        version: comparator_manifest["version"]
            .as_str()
            .expect("comparator version")
            .to_owned(),
        manifest_hash: comparator_manifest["manifest_hash"]
            .as_str()
            .expect("comparator hash")
            .to_owned(),
    };
    let comparison = failed_candidate_comparison(analysis_report.clone(), comparator.clone());
    let comparison_bindings = std::iter::once(binding(ExportBindingScope::Exporter, exporter))
        .chain(std::iter::once(binding(
            ExportBindingScope::Comparison,
            comparator,
        )))
        .chain(
            analysis_report
                .analysis_receipt
                .components
                .iter()
                .cloned()
                .map(|component| binding(ExportBindingScope::Baseline, component)),
        )
        .collect::<Vec<_>>();
    let comparison_bytes = serde_json::to_vec(&comparison).expect("comparison serializes");
    let comparison_hashes = SubjectHashes::Comparison {
        comparison_key: comparison.comparison_key.clone(),
        baseline: SubjectHashAttempt::Success {
            analysis_key: analysis_report.analysis_key.clone(),
            facts_hash: analysis_report.facts_hash.clone(),
        },
        candidate: SubjectHashAttempt::Failed {
            error_state: "parse_failed".to_owned(),
        },
    };
    let comparison_manifest = export_manifest(
        ExportKind::Comparison,
        comparison_hashes,
        &comparison_bytes,
        "Comparison",
        COMPARISON_SCHEMA_VERSION,
        comparison_bindings,
    );
    let comparison_artifact = ExportArtifact {
        schema_version: ExportSchemaVersion::V0_1,
        subject: ExportSubject::Comparison {
            snapshot: comparison,
        },
        manifest: comparison_manifest,
    };
    let comparison_path = "fixtures/expected/3.5.14/sr3514_export_comparison.json";
    let (_, comparison_fixture_hash) =
        write_export_fixture(comparison_path, &comparison_artifact, check);

    let mut fixture_manifest = json!({
        "schema_version": EXPORT_SCHEMA_VERSION,
        "fixture_family": "starrocks-3.5.14-synthetic-export-contract",
        "evidence": "synthetic_contract_only_not_docker_e2e",
        "hash_contract": "canonical object excluding manifest_hash",
        "fixtures": [
            {
                "fixture_id": "sr3514_export_analysis",
                "artifact_kind": "analysis",
                "fixture_path": analysis_path,
                "fixture_sha256": analysis_fixture_hash,
                "subject_hash_kind": "analysis"
            },
            {
                "fixture_id": "sr3514_export_comparison_failed_candidate",
                "artifact_kind": "comparison",
                "fixture_path": comparison_path,
                "fixture_sha256": comparison_fixture_hash,
                "subject_hash_kind": "comparison"
            }
        ]
    });
    let manifest_hash = manifest_hash(&fixture_manifest);
    fixture_manifest["manifest_hash"] = json!(manifest_hash);
    write_if_changed(
        Path::new("fixtures/manifests/sr3514-export-contract-fixtures.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&fixture_manifest)
                .expect("export fixture manifest serializes")
        ),
        check,
    );

    let generated = format!(
        "// Generated by doctor-contracts; do not edit.\nimport type {{ ExportArtifact }} from './doctor-contracts';\n\nexport const sr3514ExportAnalysis = {} satisfies ExportArtifact;\n\nexport const sr3514ExportComparisonFailedCandidate = {} satisfies ExportArtifact;\n",
        serde_json::to_string_pretty(&analysis_artifact).expect("analysis fixture serializes"),
        serde_json::to_string_pretty(&comparison_artifact).expect("comparison fixture serializes"),
    );
    write_if_changed(
        Path::new("contracts/generated/export-contract-fixtures.ts"),
        generated,
        check,
    );
}

fn main() {
    let check = std::env::args().any(|argument| argument == "--check");
    export_analysis_snapshot_schema(check);
    export_schema::<AnalysisOutcome>("AnalysisOutcome", check);
    export_schema::<AnalysisAttempt>("AnalysisAttempt", check);
    export_schema::<MetricCell>("MetricCell", check);
    export_schema::<BoundedInterval>("BoundedInterval", check);
    export_schema::<GraphView>("GraphView", check);
    export_schema::<ExecutionSummary>("ExecutionSummary", check);
    export_schema::<ExecutionShape>("ExecutionShape", check);
    let runtime_report: AnalysisSnapshotReport = serde_json::from_str(
        &fs::read_to_string("fixtures/expected/3.5.14/sr3514_execution_graph.report.json")
            .expect("runtime presentation golden"),
    )
    .expect("runtime presentation follows Rust DTO");
    write_if_changed(
        Path::new("contracts/generated/execution-graph-fixture.ts"),
        format!(
            "// Generated from the retained runtime capture; not a new live E2E. Do not edit.\nimport type {{ AnalysisSnapshotReport }} from './doctor-contracts';\nexport const sr3514ExecutionGraph = {} satisfies AnalysisSnapshotReport;\n",
            serde_json::to_string_pretty(&runtime_report).expect("runtime presentation TS fixture")
        ),
        check,
    );
    export_schema::<Finding>("Finding", check);
    export_schema::<RuleEvaluation>("RuleEvaluation", check);
    export_comparison_schema(check);
    export_schema::<Capabilities>("Capabilities", check);
    export_schema::<ErrorEnvelope>("ErrorEnvelope", check);
    export_schema::<InputOrigin>("InputOrigin", check);
    export_schema::<ConnectionCreateRequest>("ConnectionCreateRequest", check);
    export_schema::<AccessSessionStatus>("AccessSessionStatus", check);
    export_schema::<AccessLoginRequest>("AccessLoginRequest", check);
    export_schema::<ConnectionCapabilityReport>("ConnectionCapabilityReport", check);
    export_schema::<TimeZoneEvidence>("TimeZoneEvidence", check);
    export_schema::<ConnectionSessionView>("ConnectionSessionView", check);
    export_schema::<ConnectedQueryRef>("ConnectedQueryRef", check);
    export_schema::<ConnectedQueryListRequest>("ConnectedQueryListRequest", check);
    export_schema::<ConnectedQueryListResponse>("ConnectedQueryListResponse", check);
    export_schema::<ConnectedQuerySqlRequest>("ConnectedQuerySqlRequest", check);
    export_schema::<ConnectedQuerySqlResponse>("ConnectedQuerySqlResponse", check);
    export_schema::<AuditOccurrenceReadRequest>("AuditOccurrenceReadRequest", check);
    export_schema::<AuditFamilyReadRequest>("AuditFamilyReadRequest", check);
    export_schema::<AuditOverviewReadRequest>("AuditOverviewReadRequest", check);
    export_schema::<AuditActiveAtReadRequest>("AuditActiveAtReadRequest", check);
    export_schema::<AuditOverviewResponse>("AuditOverviewResponse", check);
    export_schema::<AuditActiveAtResponse>("AuditActiveAtResponse", check);
    export_schema::<AuditSqlReadRequest>("AuditSqlReadRequest", check);
    export_schema::<AuditErrorCodeFact>("AuditErrorCodeFact", check);
    export_schema::<AuditStatementSample>("AuditStatementSample", check);
    export_schema::<AuditOptionalFactUnavailableReason>(
        "AuditOptionalFactUnavailableReason",
        check,
    );
    export_schema::<AuditErrorProportion>("AuditErrorProportion", check);
    export_schema::<AuditOccurrenceResponse>("AuditOccurrenceResponse", check);
    export_schema::<AuditFamilyResponse>("AuditFamilyResponse", check);
    export_schema::<AuditFamilyDetailReadRequest>("AuditFamilyDetailReadRequest", check);
    export_schema::<AuditFamilyDetailResponse>("AuditFamilyDetailResponse", check);
    export_schema::<AuditFingerprintFamilyResponse>("AuditFingerprintFamilyResponse", check);
    export_schema::<ProductSqlFingerprint>("ProductSqlFingerprint", check);
    export_schema::<AuditSimilarSqlReadRequest>("AuditSimilarSqlReadRequest", check);
    export_schema::<AuditSimilarSqlResponse>("AuditSimilarSqlResponse", check);
    export_schema::<HistoryStatusResponse>("HistoryStatusResponse", check);
    export_schema::<HistoryTrendsRequest>("HistoryTrendsRequest", check);
    export_schema::<HistoryTrendsResponse>("HistoryTrendsResponse", check);
    export_schema::<HistorySeriesRequest>("HistorySeriesRequest", check);
    export_schema::<HistorySeriesResponse>("HistorySeriesResponse", check);
    export_schema::<HistoryAlertsRequest>("HistoryAlertsRequest", check);
    export_schema::<HistoryAlertsResponse>("HistoryAlertsResponse", check);
    export_schema::<AlertActionRequest>("AlertActionRequest", check);
    export_schema::<GovernanceListRequest>("GovernanceListRequest", check);
    export_schema::<GovernanceListResponse>("GovernanceListResponse", check);
    export_schema::<GovernanceCreateRequest>("GovernanceCreateRequest", check);
    export_schema::<GovernanceTransitionRequest>("GovernanceTransitionRequest", check);
    export_schema::<GovernanceItemRequest>("GovernanceItemRequest", check);
    export_schema::<GovernanceChangeRequest>("GovernanceChangeRequest", check);
    export_schema::<GovernanceItem>("GovernanceItem", check);
    export_schema::<HistoryCollectionRequest>("HistoryCollectionRequest", check);
    export_schema::<ProfileFetchRequest>("ProfileFetchRequest", check);
    export_schema::<OfficialProfileReadRequest>("OfficialProfileReadRequest", check);
    export_schema::<OfficialProfileIdentity>("OfficialProfileIdentity", check);
    export_schema::<OfficialProfileEligibility>("OfficialProfileEligibility", check);
    export_schema::<OfficialProfileEligibilityMethod>("OfficialProfileEligibilityMethod", check);
    export_schema::<OfficialProfileReadResponse>("OfficialProfileReadResponse", check);
    export_schema::<ProfileFetchResponse>("ProfileFetchResponse", check);
    export_schema::<RunningProfileCandidateListRequest>(
        "RunningProfileCandidateListRequest",
        check,
    );
    export_schema::<RunningProfileCandidateListResponse>(
        "RunningProfileCandidateListResponse",
        check,
    );
    export_schema::<RunningProfileSnapshotRequest>("RunningProfileSnapshotRequest", check);
    export_schema::<RunningProfileSnapshotResponse>("RunningProfileSnapshotResponse", check);
    export_schema::<ExecutionAcquisitionMethod>("ExecutionAcquisitionMethod", check);
    export_schema::<ConnectedProfileComparisonRequest>("ConnectedProfileComparisonRequest", check);
    export_schema::<ConnectedProfileComparisonSide>("ConnectedProfileComparisonSide", check);
    export_schema::<ConnectedProfileComparisonResponse>(
        "ConnectedProfileComparisonResponse",
        check,
    );
    export_schema::<ExplainRequest>("ExplainRequest", check);
    export_schema::<ExplainMode>("ExplainMode", check);
    export_schema::<ExplainVersionContext>("ExplainVersionContext", check);
    export_schema::<ExplainReceipt>("ExplainReceipt", check);
    export_schema::<ExplainFactKind>("ExplainFactKind", check);
    export_schema::<ExplainFactEvidenceKind>("ExplainFactEvidenceKind", check);
    export_schema::<ExplainCardinalityNumberKind>("ExplainCardinalityNumberKind", check);
    export_schema::<ExplainCardinalityEstimate>("ExplainCardinalityEstimate", check);
    export_schema::<ExplainFactValue>("ExplainFactValue", check);
    export_schema::<ExplainDistribution>("ExplainDistribution", check);
    export_schema::<ExplainJoinStrategy>("ExplainJoinStrategy", check);
    export_schema::<ExplainAggregatePhase>("ExplainAggregatePhase", check);
    export_schema::<ExplainCardSlotId>("ExplainCardSlotId", check);
    export_schema::<ExplainCardFactSlotState>("ExplainCardFactSlotState", check);
    export_schema::<ExplainCardFactSlot>("ExplainCardFactSlot", check);
    export_schema::<ExplainNodeFact>("ExplainNodeFact", check);
    export_schema::<ExplainNodeFactSet>("ExplainNodeFactSet", check);
    export_schema::<ExplainSourceLocator>("ExplainSourceLocator", check);
    export_schema::<ExplainSourceView>("ExplainSourceView", check);
    export_explain_snapshot_schema(check);
    export_schema::<ExplainResponse>("ExplainResponse", check);
    export_schema::<ConnectedRecommendation>("ConnectedRecommendation", check);
    export_schema::<PlanAdvisorRequest>("PlanAdvisorRequest", check);
    export_schema::<PlanAdvisorResponse>("PlanAdvisorResponse", check);
    export_schema::<CorrelationEvidence>("CorrelationEvidence", check);
    export_connected_schema::<PlanComparison>("PlanComparison", check);
    export_connected_schema::<OptimizationExperimentContext>(
        "OptimizationExperimentContext",
        check,
    );
    export_connected_schema::<ConnectedContractGolden>("ConnectedContractGolden", check);
    export_strict_schema::<ExportRequest>("ExportRequest", check);
    export_strict_schema::<ExportSubject>("ExportSubject", check);
    export_artifact_schema::<ExportArtifact>("ExportArtifact", check);
    export_artifact_schema::<ExportManifest>("ExportManifest", check);
    export_schema::<ExportFile>("ExportFile", check);
    export_schema::<SchemaReceipt>("SchemaReceipt", check);
    export_schema::<ReceiptBinding>("ReceiptBinding", check);
    export_strict_schema::<SubjectHashes>("SubjectHashes", check);
    let mut types = Vec::new();
    export_ts::<AnalysisState>(&mut types);
    export_ts::<ParseState>(&mut types);
    export_ts::<MetricCellState>(&mut types);
    export_ts::<Fidelity>(&mut types);
    export_ts::<NumberKind>(&mut types);
    export_ts::<GraphState>(&mut types);
    export_ts::<ComparisonState>(&mut types);
    export_ts::<MetricDeltaState>(&mut types);
    export_ts::<PercentageState>(&mut types);
    export_ts::<VersionSource>(&mut types);
    export_ts::<PackMatch>(&mut types);
    export_ts::<Compatibility>(&mut types);
    export_ts::<CompletenessState>(&mut types);
    export_ts::<SourceKind>(&mut types);
    export_ts::<ProfileLevel>(&mut types);
    export_ts::<QueryState>(&mut types);
    export_ts::<SnapshotKind>(&mut types);
    export_ts::<MergeState>(&mut types);
    export_ts::<RuleEvaluationStatus>(&mut types);
    export_ts::<FindingConfidence>(&mut types);
    export_ts::<EvidenceCompleteness>(&mut types);
    export_ts::<TopologySource>(&mut types);
    export_ts::<GraphEntityKind>(&mut types);
    export_ts::<GraphRelation>(&mut types);
    export_ts::<DirectionSemantics>(&mut types);
    export_ts::<EdgeSource>(&mut types);
    export_ts::<SourceQuality>(&mut types);
    export_ts::<InputRole>(&mut types);
    export_ts::<RequestedSourceKind>(&mut types);
    export_ts::<SupportStatus>(&mut types);
    export_ts::<CompatibilityCheckState>(&mut types);
    export_ts::<SourceRole>(&mut types);
    export_ts::<CanonicalNumber>(&mut types);
    export_ts::<BoundedInterval>(&mut types);
    export_ts::<LosslessFidelity>(&mut types);
    export_ts::<BoundedFidelity>(&mut types);
    export_ts::<UnknownFidelity>(&mut types);
    export_ts::<ObservedMetricFidelity>(&mut types);
    export_ts::<MetricCellValue>(&mut types);
    export_ts::<MetricCell>(&mut types);
    export_ts::<OrderedOperatorRef>(&mut types);
    export_ts::<RecipeRef>(&mut types);
    export_ts::<GraphNode>(&mut types);
    export_ts::<GraphSqlFact>(&mut types);
    export_ts::<GraphSqlLocation>(&mut types);
    export_ts::<GraphSqlFactKind>(&mut types);
    export_ts::<GraphEdge>(&mut types);
    export_ts::<UnresolvedRelation>(&mut types);
    export_ts::<GraphView>(&mut types);
    export_ts::<RuntimeInventoryFormatterShape>(&mut types);
    export_ts::<RuntimeInventoryAcquisitionShape>(&mut types);
    export_ts::<RuntimeSourceRole>(&mut types);
    export_ts::<ReceiptComponent>(&mut types);
    export_ts::<AnalysisReceipt>(&mut types);
    export_ts::<InputSummary>(&mut types);
    export_ts::<VersionEvidence>(&mut types);
    export_ts::<SelectedVersionPack>(&mut types);
    export_ts::<ReleaseDeltas>(&mut types);
    export_ts::<CompatibilityCheck>(&mut types);
    export_ts::<VersionContext>(&mut types);
    export_ts::<ProfileContext>(&mut types);
    export_ts::<DimensionCompleteness>(&mut types);
    export_ts::<CompletenessReport>(&mut types);
    export_ts::<OperatorRow>(&mut types);
    export_ts::<ExecutionSummaryState>(&mut types);
    export_ts::<SummaryValueState>(&mut types);
    export_ts::<SummaryValue>(&mut types);
    export_ts::<HotspotTier>(&mut types);
    export_ts::<SummaryOperatorRole>(&mut types);
    export_ts::<PlanNodeOperatorSummary>(&mut types);
    export_ts::<PlanNodeSummary>(&mut types);
    export_ts::<ExecutionSummary>(&mut types);
    export_ts::<ExecutionShapeState>(&mut types);
    export_ts::<FragmentInstanceCount>(&mut types);
    export_ts::<ExecutionShape>(&mut types);
    export_ts::<QuerySummary>(&mut types);
    export_ts::<RecommendationKind>(&mut types);
    export_ts::<RecommendationRiskLevel>(&mut types);
    export_ts::<RecommendationResultEffect>(&mut types);
    export_ts::<RecommendationStep>(&mut types);
    export_ts::<ExpectedChangeDirection>(&mut types);
    export_ts::<ExpectedMetricChange>(&mut types);
    export_ts::<Recommendation>(&mut types);
    export_ts::<FindingMeasurement>(&mut types);
    export_ts::<Finding>(&mut types);
    export_ts::<RuleEvaluation>(&mut types);
    export_ts::<Diagnostic>(&mut types);
    export_ts::<SourceLine>(&mut types);
    export_ts::<SourceToken>(&mut types);
    export_ts::<SourceLineRef>(&mut types);
    export_ts::<SourceView>(&mut types);
    export_ts::<AnalysisSnapshotReport>(&mut types);
    export_ts::<AnalysisOutcome>(&mut types);
    export_ts::<ErrorSummary>(&mut types);
    export_ts::<ErrorEnvelope>(&mut types);
    export_ts::<AnalysisOutcomeAttempt>(&mut types);
    export_ts::<AnalysisErrorAttempt>(&mut types);
    types.push("export type AnalysisAttempt = ({ outcome: AnalysisOutcome, error?: never }) | ({ error: ErrorSummary, outcome?: never });".to_owned());
    export_ts::<MetricDelta>(&mut types);
    export_ts::<ComparisonReceipt>(&mut types);
    export_ts::<StructureDelta>(&mut types);
    export_ts::<FindingMeasurementChange>(&mut types);
    export_ts::<FindingDelta>(&mut types);
    export_ts::<Comparison>(&mut types);
    export_ts::<VersionPackCapability>(&mut types);
    export_ts::<CsrfCapability>(&mut types);
    export_ts::<BuildCapability>(&mut types);
    export_ts::<Capabilities>(&mut types);
    export_ts::<InputOrigin>(&mut types);
    export_ts::<TlsMode>(&mut types);
    export_ts::<ConnectionSessionState>(&mut types);
    export_ts::<ConnectionCapabilityState>(&mut types);
    export_ts::<QueryDiscoverySource>(&mut types);
    export_ts::<ConnectedTimestampState>(&mut types);
    export_ts::<ProfileFetchAvailableReason>(&mut types);
    export_ts::<ProfileFetchUnavailableReason>(&mut types);
    export_ts::<PlanCheckAvailableReason>(&mut types);
    export_ts::<PlanCheckUnavailableReason>(&mut types);
    export_ts::<QueryFilterScope>(&mut types);
    export_ts::<ConnectedComparisonInterpretationState>(&mut types);
    export_ts::<ProfileFetchKind>(&mut types);
    export_ts::<ProfileFetchState>(&mut types);
    export_ts::<RecommendationSource>(&mut types);
    export_ts::<AdvisorProvenance>(&mut types);
    export_ts::<ExplainRuntimeState>(&mut types);
    export_ts::<ExplainSchemaVersion>(&mut types);
    export_ts::<ExplainLevel>(&mut types);
    export_ts::<ExplainMode>(&mut types);
    export_ts::<ExplainSourceKind>(&mut types);
    export_ts::<ExplainMetadataAvailability>(&mut types);
    export_ts::<ExplainSourceDisplayState>(&mut types);
    export_ts::<ExplainSourceTruncationState>(&mut types);
    export_ts::<PlanComparisonState>(&mut types);
    export_ts::<RuntimeDeltaState>(&mut types);
    export_ts::<CandidateProfileResponsibility>(&mut types);
    export_ts::<CandidateProfileState>(&mut types);
    export_ts::<ConnectedGoldenEvidence>(&mut types);
    export_ts::<ConnectedSchemaVersion>(&mut types);
    export_ts::<ConnectionCreateRequest>(&mut types);
    export_ts::<AccessSessionStatus>(&mut types);
    export_ts::<AccessLoginRequest>(&mut types);
    export_ts::<ConnectionCapabilityReport>(&mut types);
    export_ts::<TimeZoneEvidence>(&mut types);
    export_ts::<ConnectionSessionView>(&mut types);
    export_ts::<ConnectedProfileFetchAction>(&mut types);
    export_ts::<ConnectedPlanCheckAction>(&mut types);
    export_ts::<ConnectedQueryRef>(&mut types);
    export_ts::<ConnectedQueryListRequest>(&mut types);
    export_ts::<QueryListStatementFilterCoverage>(&mut types);
    export_ts::<ConnectedQueryListResponse>(&mut types);
    export_ts::<ConnectedQuerySqlRequest>(&mut types);
    export_ts::<ConnectedQuerySqlResponse>(&mut types);
    export_ts::<AuditWindow>(&mut types);
    export_ts::<AuditTerminalState>(&mut types);
    export_ts::<AuditStatementFilter>(&mut types);
    export_ts::<AuditScope>(&mut types);
    export_ts::<AuditOccurrenceSort>(&mut types);
    export_ts::<AuditOccurrenceRequest>(&mut types);
    export_ts::<AuditFamilySort>(&mut types);
    export_ts::<AuditFamilyRequest>(&mut types);
    export_ts::<AuditMetricId>(&mut types);
    export_ts::<AuditOptionalFactUnavailableReason>(&mut types);
    export_ts::<AuditMetricInput>(&mut types);
    export_ts::<AuditMetricState>(&mut types);
    export_ts::<AuditMetricFact>(&mut types);
    export_ts::<AuditQueryIdentityState>(&mut types);
    export_ts::<AuditFamilyContext>(&mut types);
    export_ts::<AuditUngroupedReason>(&mut types);
    export_ts::<AuditFamilyMembership>(&mut types);
    export_ts::<AuditOccurrenceReadRequest>(&mut types);
    export_ts::<AuditFamilyReadRequest>(&mut types);
    export_ts::<AuditOverviewReadRequest>(&mut types);
    export_ts::<AuditActiveAtReadRequest>(&mut types);
    export_ts::<AuditOverviewRequest>(&mut types);
    export_ts::<AuditOverviewPrimary>(&mut types);
    export_ts::<AuditOverviewFamilyTopSort>(&mut types);
    export_ts::<AuditSqlReadRequest>(&mut types);
    export_ts::<AuditWorkloadVersion>(&mut types);
    export_ts::<AuditClusterIdentity>(&mut types);
    export_ts::<AuditAggregation>(&mut types);
    export_ts::<AuditAggregateFact>(&mut types);
    export_ts::<AuditErrorProportion>(&mut types);
    export_ts::<AuditErrorProportionUnavailableReason>(&mut types);
    export_ts::<AuditErrorProportionInvalidReason>(&mut types);
    export_ts::<AuditMetricCoverage>(&mut types);
    export_ts::<AuditStatementFilterCoverage>(&mut types);
    export_ts::<AuditCoverage>(&mut types);
    export_ts::<AuditTopWindow>(&mut types);
    export_ts::<AuditErrorCodeFact>(&mut types);
    export_ts::<AuditStatementSampleUnavailableReason>(&mut types);
    export_ts::<AuditStatementSample>(&mut types);
    export_ts::<AuditOccurrencePresentation>(&mut types);
    export_ts::<AuditFamilyPresentation>(&mut types);
    export_ts::<AuditPlotProjection>(&mut types);
    export_ts::<AuditPlotNotApplicableReason>(&mut types);
    export_ts::<AuditPlotUnavailableReason>(&mut types);
    export_ts::<AuditConcurrencyWindow>(&mut types);
    export_ts::<AuditConcurrencyTimeTick>(&mut types);
    export_ts::<AuditConcurrencyAxis>(&mut types);
    export_ts::<AuditConcurrencyPoint>(&mut types);
    export_ts::<AuditConcurrencyCountTick>(&mut types);
    export_ts::<AuditBoundedSourceCoverage>(&mut types);
    export_ts::<AuditBoundedSourcePartialReason>(&mut types);
    export_ts::<AuditConcurrencyCoverage>(&mut types);
    export_ts::<AuditConcurrencySeries>(&mut types);
    export_ts::<AuditConcurrencyUnavailableReason>(&mut types);
    export_ts::<AuditConcurrencyPresentation>(&mut types);
    export_ts::<AuditOverviewFamilyTopRow>(&mut types);
    export_ts::<AuditOverviewProjectionScope>(&mut types);
    export_ts::<AuditOverviewFamilyTopPresentation>(&mut types);
    export_ts::<AuditOverviewPrimaryPresentation>(&mut types);
    export_ts::<AuditOccurrenceResponse>(&mut types);
    export_ts::<AuditFamilyResponse>(&mut types);
    export_ts::<AuditFamilyDetailReadRequest>(&mut types);
    export_ts::<AuditFamilyDistributionAlgorithm>(&mut types);
    export_ts::<AuditFamilyHistogramBucket>(&mut types);
    export_ts::<AuditFamilyDistributionExclusions>(&mut types);
    export_ts::<AuditFamilyExecutionDistribution>(&mut types);
    export_ts::<AuditFamilyRepresentativeUnavailableReason>(&mut types);
    export_ts::<AuditProfileEvidence>(&mut types);
    export_ts::<AuditFamilyRepresentative>(&mut types);
    export_ts::<AuditFamilyRepresentatives>(&mut types);
    export_ts::<AuditProfileListCheck>(&mut types);
    export_ts::<AuditProfileEvidenceCoverage>(&mut types);
    export_ts::<AuditFamilyDetailResponse>(&mut types);
    export_ts::<ProductSqlFingerprint>(&mut types);
    export_ts::<ProductSqlFingerprintPartialReason>(&mut types);
    export_ts::<ProductSqlFingerprintUnavailableReason>(&mut types);
    export_ts::<ProductFingerprintAuthority>(&mut types);
    export_ts::<AuditFingerprintFamilyPresentation>(&mut types);
    export_ts::<AuditFingerprintSourceCoverage>(&mut types);
    export_ts::<AuditFingerprintPartialReason>(&mut types);
    export_ts::<AuditFingerprintUnavailableCount>(&mut types);
    export_ts::<AuditFingerprintCoverage>(&mut types);
    export_ts::<AuditFingerprintFamilyResponse>(&mut types);
    export_ts::<SimilarSqlDimensions>(&mut types);
    export_ts::<AuditSimilarSqlRequest>(&mut types);
    export_ts::<AuditSimilarSqlReadRequest>(&mut types);
    export_ts::<SimilarSqlMemberSource>(&mut types);
    export_ts::<SimilarSqlBasis>(&mut types);
    export_ts::<SimilarSqlUnavailableReason>(&mut types);
    export_ts::<SimilarSqlStructure>(&mut types);
    export_ts::<SimilarSqlMember>(&mut types);
    export_ts::<SimilarSqlCluster>(&mut types);
    export_ts::<SimilarSqlFamilySource>(&mut types);
    export_ts::<SimilarSqlCoverage>(&mut types);
    export_ts::<AuditSimilarSqlResponse>(&mut types);
    export_ts::<HistoryStatusRequest>(&mut types);
    export_ts::<HistoryCollectionAction>(&mut types);
    export_ts::<HistoryCollectionRequest>(&mut types);
    export_ts::<HistoryStoreState>(&mut types);
    export_ts::<HistoryStoreUnavailableReason>(&mut types);
    export_ts::<CollectionStoppedReason>(&mut types);
    export_ts::<CollectionState>(&mut types);
    export_ts::<CollectionBudget>(&mut types);
    export_ts::<CollectionRunState>(&mut types);
    export_ts::<CollectionRun>(&mut types);
    export_ts::<CoverageHourState>(&mut types);
    export_ts::<CoverageHour>(&mut types);
    export_ts::<HistoryStatusResponse>(&mut types);
    export_ts::<TrendSeverity>(&mut types);
    export_ts::<TrendConfidence>(&mut types);
    export_ts::<WorkloadSignal>(&mut types);
    export_ts::<TrendUnscoredReason>(&mut types);
    export_ts::<TrendNotComparableReason>(&mut types);
    export_ts::<TrendVerdict>(&mut types);
    export_ts::<TrendSide>(&mut types);
    export_ts::<TrendDirection>(&mut types);
    export_ts::<TrendChange>(&mut types);
    export_ts::<TrendComparability>(&mut types);
    export_ts::<TrendRow>(&mut types);
    export_ts::<TrendRule>(&mut types);
    export_ts::<TrendCounts>(&mut types);
    export_ts::<TrendWindow>(&mut types);
    export_ts::<TrendPoint>(&mut types);
    export_ts::<TrendExemplar>(&mut types);
    export_ts::<HistoryTrendsRequest>(&mut types);
    export_ts::<HistoryTrendsResponse>(&mut types);
    export_ts::<HistorySeriesRequest>(&mut types);
    export_ts::<HistorySeriesResponse>(&mut types);
    export_ts::<AlertState>(&mut types);
    export_ts::<AlertAction>(&mut types);
    export_ts::<AlertView>(&mut types);
    export_ts::<AlertCounts>(&mut types);
    export_ts::<AlertChannel>(&mut types);
    export_ts::<AlertRule>(&mut types);
    export_ts::<HistoryAlertsRequest>(&mut types);
    export_ts::<HistoryAlertsResponse>(&mut types);
    export_ts::<AlertActionRequest>(&mut types);
    export_ts::<GovernanceState>(&mut types);
    export_ts::<GovernanceOrigin>(&mut types);
    export_ts::<GovernanceSnapshot>(&mut types);
    export_ts::<VerificationOutcome>(&mut types);
    export_ts::<VerificationMetric>(&mut types);
    export_ts::<GovernanceVerification>(&mut types);
    export_ts::<GovernanceEvent>(&mut types);
    export_ts::<GovernanceItem>(&mut types);
    export_ts::<GovernanceListRequest>(&mut types);
    export_ts::<GovernanceListResponse>(&mut types);
    export_ts::<GovernanceCreateRequest>(&mut types);
    export_ts::<GovernanceTransitionRequest>(&mut types);
    export_ts::<GovernanceItemRequest>(&mut types);
    export_ts::<GovernanceChangeRequest>(&mut types);
    export_ts::<AuditOverviewResponse>(&mut types);
    export_ts::<AuditActiveAtParticipants>(&mut types);
    export_ts::<AuditActiveAtParticipant>(&mut types);
    export_ts::<AuditActiveAtResponse>(&mut types);
    export_ts::<ProfileFetchRequest>(&mut types);
    export_ts::<OfficialProfileArtifactKind>(&mut types);
    export_ts::<OfficialProfileDisplayNormalization>(&mut types);
    export_ts::<OfficialProfileReadRequest>(&mut types);
    export_ts::<OfficialProfileIdentity>(&mut types);
    export_ts::<OfficialProfileEligibility>(&mut types);
    export_ts::<OfficialProfileEligibilityMethod>(&mut types);
    export_ts::<OfficialProfileReadResponse>(&mut types);
    export_ts::<ProfileFetchResponse>(&mut types);
    export_ts::<RunningProfileCandidateListRequest>(&mut types);
    export_ts::<RunningProfileCandidateListResponse>(&mut types);
    export_ts::<RunningProfileSnapshotRequest>(&mut types);
    export_ts::<RunningProfileSnapshotResponse>(&mut types);
    export_ts::<RuntimeProfileRequest>(&mut types);
    export_ts::<ExecutionAcquisitionMethod>(&mut types);
    export_ts::<ConnectedProfileComparisonRequest>(&mut types);
    export_ts::<ConnectedProfileComparisonSide>(&mut types);
    export_ts::<ConnectedComparisonValidationContext>(&mut types);
    export_ts::<ConnectedProfileComparisonResponse>(&mut types);
    export_ts::<ExplainRequest>(&mut types);
    export_ts::<ExplainVersionContext>(&mut types);
    export_ts::<ExplainReceipt>(&mut types);
    export_ts::<ExplainFactKind>(&mut types);
    export_ts::<ExplainFactEvidenceKind>(&mut types);
    export_ts::<ExplainCardinalityNumberKind>(&mut types);
    export_ts::<ExplainCardinalityEstimate>(&mut types);
    export_ts::<ExplainFactValue>(&mut types);
    export_ts::<ExplainDistribution>(&mut types);
    export_ts::<ExplainJoinStrategy>(&mut types);
    export_ts::<ExplainAggregatePhase>(&mut types);
    export_ts::<ExplainCardSlotId>(&mut types);
    export_ts::<ExplainCardFactSlotState>(&mut types);
    export_ts::<ExplainCardFactSlot>(&mut types);
    export_ts::<ExplainNodeFact>(&mut types);
    export_ts::<ExplainNodeFactSet>(&mut types);
    export_ts::<ExplainSourceLocator>(&mut types);
    export_ts::<ExplainSourceView>(&mut types);
    export_ts::<ExplainSnapshot>(&mut types);
    export_ts::<ExplainResponse>(&mut types);
    export_ts::<ConnectedRecommendation>(&mut types);
    export_ts::<PlanAdvisorRequest>(&mut types);
    export_ts::<PlanAdvisorResponse>(&mut types);
    export_ts::<CorrelationEvidence>(&mut types);
    export_ts::<PlanComparison>(&mut types);
    export_ts::<OptimizationExperimentContext>(&mut types);
    export_ts::<ConnectedContractGolden>(&mut types);
    export_ts::<ExportSchemaVersion>(&mut types);
    export_ts::<ExportKind>(&mut types);
    export_ts::<ExportCanonicalization>(&mut types);
    export_ts::<ExportIntegrity>(&mut types);
    export_ts::<ExportBindingScope>(&mut types);
    export_ts::<ExportEvidence>(&mut types);
    export_ts::<ExportFile>(&mut types);
    export_ts::<SchemaReceipt>(&mut types);
    export_ts::<ReceiptBinding>(&mut types);
    export_ts::<SubjectHashAttempt>(&mut types);
    export_ts::<SubjectHashes>(&mut types);
    export_ts::<ExportManifest>(&mut types);
    export_ts::<ExportRequest>(&mut types);
    export_ts::<ExportSubject>(&mut types);
    export_ts::<ExportArtifact>(&mut types);
    write_if_changed(
        Path::new("contracts/generated/doctor-contracts.ts"),
        format!(
            "// Generated by doctor-contracts; do not edit.\n{}\n",
            types.join("\n")
        ),
        check,
    );

    let json_report: AnalysisSnapshotReport = serde_json::from_str(
        &fs::read_to_string("fixtures/expected/3.5.14/sr3514_minimal.report.json")
            .expect("JSON report fixture"),
    )
    .expect("JSON report fixture follows Rust DTO");
    let text_report: AnalysisSnapshotReport = serde_json::from_str(
        &fs::read_to_string("fixtures/expected/3.5.14/sr3514_minimal_text.report.json")
            .expect("Text report fixture"),
    )
    .expect("Text report fixture follows Rust DTO");
    let fixture_types = format!(
        "// Generated by doctor-contracts; do not edit.\nimport type {{ AnalysisSnapshotReport }} from './doctor-contracts';\n\nexport const sr3514JsonMinimal = {} satisfies AnalysisSnapshotReport;\n\nexport const sr3514TextMinimal = {} satisfies AnalysisSnapshotReport;\n",
        serde_json::to_string_pretty(&json_report).expect("JSON report serializes"),
        serde_json::to_string_pretty(&text_report).expect("Text report serializes"),
    );
    write_if_changed(
        Path::new("contracts/generated/contract-fixtures.ts"),
        fixture_types,
        check,
    );

    let connected_fixture_path =
        Path::new("fixtures/expected/3.5.14/sr3514_connected_contract_golden.json");
    let mut connected_value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(connected_fixture_path).expect("connected contract fixture"),
    )
    .expect("connected contract fixture is JSON");
    connected_value["connection"]["tls_mode"] = json!("docker_loopback_plaintext");
    if let Some(connection) = connected_value["connection"].as_object_mut() {
        let idle_timeout = connection
            .remove("expires_in_seconds")
            .unwrap_or_else(|| json!(900));
        connection.insert("idle_timeout_seconds".to_owned(), idle_timeout);
    }
    connected_value["query_response"]["discovery_generation"] = json!(1);
    connected_value["query_response"]["filter_scope"] = json!("audit_window");
    connected_value["query_response"]["limitations"] = json!([
        "query_text_omitted_by_contract",
        "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch"
    ]);
    connected_value["query_request"]["statement_filter"] = json!("exclude_doctor_and_non_query");
    connected_value["query_response"]["statement_filter"] = json!({
        "state": "applied",
        "hidden_non_query_row_count": "5",
        "hidden_doctor_row_count": "2"
    });
    let synthetic_plan_ref = format!("plan-{}", "0".repeat(48));
    if let Some(queries) = connected_value["query_response"]["queries"].as_array_mut() {
        for query in queries {
            query["discovery_generation"] = json!(1);
            query["observed_at_local"] = json!("2026-08-30 00:00:00");
            query["timestamp_state"] = json!("reader_session_interpreted");
            query["profile_fetch"] = json!({
                "state": "available",
                "reason": "can_be_attempted"
            });
            query["plan_check"] = json!({
                "state": "available",
                "reason": "statement_retained",
                "plan_sql_ref": synthetic_plan_ref
            });
        }
    }
    connected_value["profile_fetch_request"]["discovery_generation"] = json!(1);
    let profile_fetch = connected_value
        .get_mut("profile_fetch_response")
        .and_then(serde_json::Value::as_object_mut)
        .expect("connected fixture has profile_fetch_response");
    let fetched_query = profile_fetch
        .get_mut("query")
        .and_then(serde_json::Value::as_object_mut)
        .expect("connected fetch fixture has query reference");
    fetched_query.insert("discovery_source".to_owned(), json!("audit"));
    fetched_query.insert("discovery_generation".to_owned(), json!(1));
    fetched_query.insert("observed_at_utc".to_owned(), json!("2026-08-30T00:00:00Z"));
    fetched_query.insert("observed_at_local".to_owned(), json!("2026-08-30 00:00:00"));
    fetched_query.insert(
        "timestamp_state".to_owned(),
        json!("reader_session_interpreted"),
    );
    fetched_query.insert("status".to_owned(), json!("finished"));
    fetched_query.insert("digest".to_owned(), json!("sha256:synthetic-query-digest"));
    fetched_query.insert("database".to_owned(), json!("demo"));
    fetched_query.insert("resource_group".to_owned(), serde_json::Value::Null);
    fetched_query.insert("query_time_ms".to_owned(), json!(500));
    fetched_query.insert(
        "profile_fetch".to_owned(),
        json!({ "state": "available", "reason": "profile_fetched" }),
    );
    fetched_query.insert(
        "plan_check".to_owned(),
        json!({
            "state": "available",
            "reason": "statement_retained",
            "plan_sql_ref": synthetic_plan_ref
        }),
    );
    fetched_query.insert(
        "source_refs".to_owned(),
        json!([
            "audit:row:0:timestamp",
            "audit:row:0:epoch",
            "profile:query_id"
        ]),
    );
    profile_fetch.insert(
        "source_kind".to_owned(),
        serde_json::to_value(text_report.input.detected_kind.clone())
            .expect("source kind serializes"),
    );
    profile_fetch.insert(
        "source_hash".to_owned(),
        serde_json::to_value(text_report.input.source_hash.clone())
            .expect("source hash serializes"),
    );
    profile_fetch.insert(
        "analysis_key".to_owned(),
        serde_json::to_value(text_report.analysis_key.clone()).expect("analysis key serializes"),
    );
    profile_fetch.insert(
        "analysis_receipt".to_owned(),
        serde_json::to_value(text_report.analysis_receipt.clone())
            .expect("analysis receipt serializes"),
    );
    profile_fetch.insert(
        "outcome".to_owned(),
        serde_json::to_value(AnalysisOutcome {
            report: text_report.clone(),
        })
        .expect("analysis outcome serializes"),
    );
    let connection_id = connected_value["connection"]["connection_id"]
        .as_str()
        .expect("connected fixture has connection ID")
        .to_owned();
    let baseline_query_id = connected_value["profile_fetch_request"]["query_id"]
        .as_str()
        .expect("connected fixture has baseline query ID")
        .to_owned();
    let candidate_query_id = "00000000-0000-0000-0000-000000003515".to_owned();
    let query_sql_request = ConnectedQuerySqlRequest {
        connection_id: connection_id.clone(),
        query_id: baseline_query_id.clone(),
        discovery_generation: 1,
    };
    let query_sql_response = ConnectedQuerySqlResponse {
        query_id: baseline_query_id.clone(),
        discovery_generation: 1,
        sql: "SELECT synthetic_query FROM demo".to_owned(),
    };
    let running_query = ConnectedQueryRef {
        connection_id: connection_id.clone(),
        query_id: candidate_query_id.clone(),
        discovery_source: Some(QueryDiscoverySource::ProfileList),
        discovery_generation: Some(2),
        observed_at_utc: None,
        observed_at_local: Some("2026-09-06 00:00:00".to_owned()),
        timestamp_state: ConnectedTimestampState::RawWallTimeOnly,
        status: "Running".to_owned(),
        digest: None,
        database: None,
        resource_group: None,
        query_time_ms: None,
        profile_fetch: ConnectedProfileFetchAction::Unavailable {
            reason: ProfileFetchUnavailableReason::QueryNotFinished,
        },
        plan_check: ConnectedPlanCheckAction::Unavailable {
            reason: PlanCheckUnavailableReason::StatementMissingOrNotSelect,
        },
        source_refs: vec!["profile_list:row:0".to_owned()],
    };
    let running_profile_candidate_list_request = RunningProfileCandidateListRequest {
        connection_id: connection_id.clone(),
        limit: 25,
        statement_filter: AuditStatementFilter::ExcludeDoctorAndNonQuery,
    };
    let running_profile_candidate_list_response = RunningProfileCandidateListResponse {
        source: QueryDiscoverySource::ProfileList,
        discovery_generation: 2,
        limit: 25,
        returned: 1,
        at_limit: false,
        local_truncated: false,
        rejected: 0,
        read_at_utc: "2026-09-06T00:00:01Z".to_owned(),
        profile_observed_at_utc: None,
        candidates: vec![running_query.clone()],
        limitations: vec![
            "query_text_omitted_by_contract".to_owned(),
            "profile_list_observation_time_unknown".to_owned(),
        ],
        statement_filter: QueryListStatementFilterCoverage::DoctorOnlyApplied {
            hidden_doctor_row_count: "1".to_owned(),
        },
    };
    let running_profile_snapshot_request = RunningProfileSnapshotRequest {
        connection_id: connection_id.clone(),
        query_id: candidate_query_id.clone(),
        discovery_generation: 2,
    };
    let running_profile_snapshot_response = RunningProfileSnapshotResponse {
        query: running_query,
        list_observed_state: "Running".to_owned(),
        profile_lifecycle: text_report.profile_context.query_state.clone(),
        read_at_utc: "2026-09-06T00:00:02Z".to_owned(),
        profile_observed_at_utc: None,
        input_origin: InputOrigin::ConnectedProfileFetch,
        source_kind: text_report.input.detected_kind.clone(),
        source_hash: text_report.input.source_hash.clone(),
        analysis_key: text_report.analysis_key.clone(),
        analysis_receipt: text_report.analysis_receipt.clone(),
        outcome: AnalysisOutcome {
            report: text_report.clone(),
        },
        source_refs: vec![
            "profile:query_id".to_owned(),
            "profile_list:running_snapshot".to_owned(),
        ],
        limitations: vec![
            "profile_raw_text_omitted_by_contract".to_owned(),
            "profile_observation_time_unknown".to_owned(),
            "list_observed_state_and_profile_lifecycle_are_independent".to_owned(),
        ],
    };
    let comparator_manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string("contracts/manifests/doctor-comparator-v0.1.json")
            .expect("comparator manifest"),
    )
    .expect("comparator manifest JSON");
    let comparator = ReceiptComponent {
        id: comparator_manifest["id"]
            .as_str()
            .expect("comparator id")
            .to_owned(),
        version: comparator_manifest["version"]
            .as_str()
            .expect("comparator version")
            .to_owned(),
        manifest_hash: comparator_manifest["manifest_hash"]
            .as_str()
            .expect("comparator hash")
            .to_owned(),
    };
    let profile_comparison_request = ConnectedProfileComparisonRequest {
        connection_id: connection_id.clone(),
        baseline_query_id: baseline_query_id.clone(),
        candidate_query_id: candidate_query_id.clone(),
        discovery_generation: 1,
    };
    let profile_comparison_response = synthetic_connected_profile_comparison(
        &connection_id,
        &baseline_query_id,
        &candidate_query_id,
        text_report.clone(),
        comparator,
    );
    let connected_fixture = connected_value
        .as_object_mut()
        .expect("connected fixture is an object");
    connected_fixture.insert(
        "profile_comparison_request".to_owned(),
        serde_json::to_value(profile_comparison_request)
            .expect("connected profile comparison request serializes"),
    );
    connected_fixture.insert(
        "running_profile_candidate_list_request".to_owned(),
        serde_json::to_value(running_profile_candidate_list_request)
            .expect("running candidate request serializes"),
    );
    connected_fixture.insert(
        "running_profile_candidate_list_response".to_owned(),
        serde_json::to_value(running_profile_candidate_list_response)
            .expect("running candidate response serializes"),
    );
    connected_fixture.insert(
        "running_profile_snapshot_request".to_owned(),
        serde_json::to_value(running_profile_snapshot_request)
            .expect("running snapshot request serializes"),
    );
    connected_fixture.insert(
        "running_profile_snapshot_response".to_owned(),
        serde_json::to_value(running_profile_snapshot_response)
            .expect("running snapshot response serializes"),
    );
    connected_fixture.insert(
        "query_sql_request".to_owned(),
        serde_json::to_value(query_sql_request).expect("connected query SQL request serializes"),
    );
    connected_fixture.insert(
        "query_sql_response".to_owned(),
        serde_json::to_value(query_sql_response).expect("connected query SQL response serializes"),
    );
    connected_fixture.insert(
        "profile_comparison_response".to_owned(),
        serde_json::to_value(profile_comparison_response)
            .expect("connected profile comparison response serializes"),
    );
    connected_fixture.insert(
        "explain_response".to_owned(),
        serde_json::to_value(ExplainResponse {
            snapshot: synthetic_explain_snapshot(),
        })
        .expect("connected Explain response serializes"),
    );
    backfill_runtime_projection(&mut connected_value);
    let connected_golden: ConnectedContractGolden = serde_json::from_value(connected_value)
        .expect("connected contract fixture follows Rust DTO");
    let connected_fixture_json = format!(
        "{}\n",
        serde_json::to_string_pretty(&connected_golden).expect("connected fixture serializes")
    );
    let connected_fixture_sha256 = sha256_bytes(connected_fixture_json.as_bytes());
    write_if_changed(connected_fixture_path, connected_fixture_json, check);
    let mut connected_manifest = json!({
        "schema_version": "0.1",
        "fixture_id": "sr3514_connected_contract_golden",
        "upstream_release": "3.5.14",
        "evidence": "synthetic_contract_only_not_docker_e2e",
        "fixture_path": "fixtures/expected/3.5.14/sr3514_connected_contract_golden.json",
        "fixture_sha256": connected_fixture_sha256,
        "hash_contract": "canonical object excluding manifest_hash"
    });
    connected_manifest["manifest_hash"] = json!(manifest_hash(&connected_manifest));
    write_if_changed(
        Path::new("fixtures/manifests/sr3514-connected-contract-fixtures.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&connected_manifest)
                .expect("connected fixture manifest serializes")
        ),
        check,
    );
    let connected_fixture_types = format!(
        "// Generated by doctor-contracts; do not edit.\nimport type {{ ConnectedContractGolden }} from './doctor-contracts';\n\nexport const sr3514ConnectedContractGolden = {} satisfies ConnectedContractGolden;\n",
        serde_json::to_string_pretty(&connected_golden).expect("connected fixture serializes"),
    );
    write_if_changed(
        Path::new("contracts/generated/connected-contract-fixtures.ts"),
        connected_fixture_types,
        check,
    );
    export_fixtures(check);
}
