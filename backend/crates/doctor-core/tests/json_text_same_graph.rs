//! The same statement, captured from the fixed-digest 3.5.14 FE once as the
//! default text Profile and once as a `JsonProfileFormatter` Profile
//! (`scripts/e2e/capture-format-pair.ps1`), must yield the same execution
//! graph: identical nodes and edges except provenance.
//!
//! Provenance that is format-specific by construction is removed before the
//! comparison: `source_refs` / `evidence_refs` (text line vs JSON pointer),
//! the opaque per-format operator ids inside `native_operator_refs` /
//! `subordinate_operator_refs` (their count, order and role still compare),
//! and metric observation ids (compared by count). Fragment ids, node types,
//! edge ids, relations, roles and restated-edge markers compare verbatim.

use std::{fs, path::PathBuf};

use doctor_contracts::{GraphState, RequestedSourceKind};
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    resolve_version_and_graph, v01_analysis_receipt,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn pair_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/runtime/3.5.14/format-pair")
}

fn read_verified(name: &str, sha256: &str) -> Vec<u8> {
    let bytes = fs::read(pair_dir().join(name)).unwrap_or_else(|_| panic!("read {name}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        sha256.to_ascii_lowercase(),
        "{name}: SHA-256 drift from manifest"
    );
    bytes
}

fn graph(kind: RequestedSourceKind, content: Vec<u8>) -> Value {
    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let mut output = adapt_input(
        InputEnvelope::new(kind, content),
        &AdapterLimits::default(),
        &context,
    )
    .expect("captured Profile adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("captured Profile resolves");
    let report = assemble_public_report(output).expect("report assembles");
    serde_json::to_value(&report.graph_view).expect("graph serializes")
}

/// Drops format-specific provenance; see the module comment.
fn without_provenance(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("source_refs");
            object.remove("evidence_refs");
            object.remove("operator_ref");
            if let Some(refs) = object.get_mut("metric_observation_refs") {
                let count = refs.as_array().map_or(0, Vec::len);
                *refs = Value::from(count);
            }
            object.values_mut().for_each(without_provenance);
        }
        Value::Array(items) => items.iter_mut().for_each(without_provenance),
        _ => {}
    }
}

#[test]
fn text_and_json_profiles_of_one_statement_give_the_same_graph() {
    let manifest: Value = serde_json::from_slice(
        &fs::read(pair_dir().join("manifest.json")).expect("format-pair manifest"),
    )
    .expect("manifest json");
    assert!(
        manifest["server_version"]
            .as_str()
            .is_some_and(|version| version.starts_with("3.5.14-23a56ec")),
        "pair server version"
    );
    let cases = manifest["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let case_id = case["case_id"].as_str().expect("case id");
        let text = read_verified(
            case["text_profile_file"].as_str().unwrap(),
            case["text_profile_sha256"].as_str().unwrap(),
        );
        let json = read_verified(
            case["json_profile_file"].as_str().unwrap(),
            case["json_profile_sha256"].as_str().unwrap(),
        );
        let mut from_text = graph(RequestedSourceKind::TextProfile, text);
        let mut from_json = graph(RequestedSourceKind::JsonProfile, json);
        assert_eq!(
            from_text["state"],
            serde_json::to_value(GraphState::Complete).unwrap(),
            "{case_id}: text graph state"
        );
        assert!(
            from_text["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|node| node["fragment_id"].is_string()),
            "{case_id}: every text node is bound to a fragment"
        );
        without_provenance(&mut from_text);
        without_provenance(&mut from_json);
        assert_eq!(
            from_text["nodes"], from_json["nodes"],
            "{case_id}: nodes differ between text and JSON"
        );
        assert_eq!(
            from_text["edges"], from_json["edges"],
            "{case_id}: edges differ between text and JSON"
        );
        assert_eq!(from_text, from_json, "{case_id}: graph differs");
    }
}
