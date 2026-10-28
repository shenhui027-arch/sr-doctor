//! Deterministic, self-consistent report export for public analysis/comparison DTOs.

use std::{collections::BTreeMap, fmt};

use doctor_contracts::{
    AnalysisAttempt, AnalysisSnapshotReport, COMPARISON_SCHEMA_VERSION, Comparison,
    EXPORT_MEDIA_TYPE, ExportArtifact, ExportBindingScope, ExportCanonicalization, ExportEvidence,
    ExportFile, ExportIntegrity, ExportKind, ExportManifest, ExportRequest, ExportSchemaVersion,
    ExportSubject, ReceiptBinding, ReceiptComponent, SCHEMA_VERSION, SchemaReceipt,
    SubjectHashAttempt, SubjectHashes, canonical_hash, canonical_json,
};
use doctor_core::{
    analysis_key, comparison_key, facts_hash, report_facts, v01_analysis_receipt,
    v01_comparison_receipt,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const EXPORTER_ID: &str = "doctor-export-v0.1";
pub const EXPORTER_VERSION: &str = "0.1.0";
pub const EXPORT_DOWNLOAD_NAME: &str = "sr-doctor-report.json";

const ANALYSIS_SCHEMA: &[u8] =
    include_bytes!("../../../../contracts/schema/AnalysisSnapshotReport.schema.json");
const COMPARISON_SCHEMA: &[u8] =
    include_bytes!("../../../../contracts/schema/Comparison.schema.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportError {
    InvalidAnalysisReceipt,
    InvalidAnalysisFacts,
    InvalidAnalysisKey,
    InvalidComparisonReceipt,
    InvalidComparisonKey,
    InvalidManifest,
    ReportTooLarge,
    Serialization,
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidAnalysisReceipt => "analysis receipt is not the frozen v0.1 chain",
            Self::InvalidAnalysisFacts => "analysis facts hash is not self-consistent",
            Self::InvalidAnalysisKey => "analysis replay key is not self-consistent",
            Self::InvalidComparisonReceipt => "comparison receipt is not the frozen v0.1 chain",
            Self::InvalidComparisonKey => "comparison key is not self-consistent",
            Self::InvalidManifest => "export manifest is not self-consistent",
            Self::ReportTooLarge => "canonical report exceeds the v0.1 byte-length contract",
            Self::Serialization => "public report cannot be serialized",
        })
    }
}

impl std::error::Error for ExportError {}

pub fn assemble(request: ExportRequest) -> Result<ExportArtifact, ExportError> {
    let subject = match request {
        ExportRequest::Analysis { snapshot } => {
            validate_analysis(&snapshot)?;
            ExportSubject::Analysis {
                snapshot: redact_analysis(snapshot)?,
            }
        }
        ExportRequest::Comparison { snapshot } => {
            validate_comparison(&snapshot)?;
            ExportSubject::Comparison {
                snapshot: redact_comparison(snapshot)?,
            }
        }
    };
    let manifest = manifest_for(&subject)?;
    let artifact = ExportArtifact {
        schema_version: ExportSchemaVersion::V0_1,
        subject,
        manifest,
    };
    validate_artifact(&artifact)?;
    Ok(artifact)
}

pub fn artifact_bytes(artifact: &ExportArtifact) -> Result<Vec<u8>, ExportError> {
    validate_artifact(artifact)?;
    canonical_bytes(artifact)
}

pub fn validate_artifact(artifact: &ExportArtifact) -> Result<(), ExportError> {
    let expected = manifest_for(&artifact.subject)?;
    if artifact.manifest != expected {
        return Err(ExportError::InvalidManifest);
    }
    match &artifact.subject {
        ExportSubject::Analysis { snapshot } => validate_analysis(snapshot),
        ExportSubject::Comparison { snapshot } => validate_comparison(snapshot),
    }
}

fn validate_analysis(report: &AnalysisSnapshotReport) -> Result<(), ExportError> {
    if report.schema_version != SCHEMA_VERSION || report.analysis_receipt != v01_analysis_receipt()
    {
        return Err(ExportError::InvalidAnalysisReceipt);
    }
    if facts_hash(&report_facts(report)) != report.facts_hash {
        return Err(ExportError::InvalidAnalysisFacts);
    }
    let options = BTreeMap::from([
        (
            "requested_kind".to_owned(),
            requested_kind_name(&report.input.requested_kind).to_owned(),
        ),
        (
            "version_hint".to_owned(),
            report
                .version
                .user_declared_version
                .clone()
                .unwrap_or_else(|| "auto".to_owned()),
        ),
    ]);
    if analysis_key(
        &report.input.source_hash,
        &report.analysis_receipt,
        &options,
    ) != report.analysis_key
    {
        return Err(ExportError::InvalidAnalysisKey);
    }
    Ok(())
}

fn validate_comparison(comparison: &Comparison) -> Result<(), ExportError> {
    if comparison.schema_version != COMPARISON_SCHEMA_VERSION
        || comparison.comparison_receipt != v01_comparison_receipt()
    {
        return Err(ExportError::InvalidComparisonReceipt);
    }
    for attempt in [&comparison.baseline, &comparison.candidate] {
        if let AnalysisAttempt::Outcome(outcome) = attempt {
            validate_analysis(&outcome.outcome.report)?;
        }
    }
    if comparison_key(comparison) != comparison.comparison_key {
        return Err(ExportError::InvalidComparisonKey);
    }
    Ok(())
}

fn redact_analysis(report: AnalysisSnapshotReport) -> Result<AnalysisSnapshotReport, ExportError> {
    let value = redact_value(serde_json::to_value(report).map_err(|_| ExportError::Serialization)?);
    let mut report: AnalysisSnapshotReport =
        serde_json::from_value(value).map_err(|_| ExportError::Serialization)?;
    mark_analysis_redacted(&mut report);
    validate_analysis(&report)?;
    Ok(report)
}

fn mark_analysis_redacted(report: &mut AnalysisSnapshotReport) {
    if !report
        .source_view
        .limitations
        .iter()
        .any(|item| item == "export_source_view_redacted")
    {
        report
            .source_view
            .limitations
            .push("export_source_view_redacted".to_owned());
    }
}

fn redact_comparison(comparison: Comparison) -> Result<Comparison, ExportError> {
    let value =
        redact_value(serde_json::to_value(comparison).map_err(|_| ExportError::Serialization)?);
    let mut comparison: Comparison =
        serde_json::from_value(value).map_err(|_| ExportError::Serialization)?;
    for attempt in [&mut comparison.baseline, &mut comparison.candidate] {
        if let AnalysisAttempt::Outcome(outcome) = attempt {
            mark_analysis_redacted(&mut outcome.outcome.report);
        }
    }
    validate_comparison(&comparison)?;
    Ok(comparison)
}

fn redact_value(mut value: Value) -> Value {
    match &mut value {
        Value::Array(items) => {
            for item in items {
                *item = redact_value(item.take());
            }
        }
        Value::Object(fields) => {
            // SQL excerpts are request-local UI material, not portable exports.
            fields.remove("sql_location");
            for (name, field) in fields.iter_mut() {
                match name.as_str() {
                    "filename" | "display_name" => *field = Value::Null,
                    "details" => *field = json!({}),
                    "plain_text_lines"
                    | "json_tokens"
                    | "source_display"
                    | "adapter_evidence"
                    | "input_observation_refs"
                    | "evidence" => *field = json!([]),
                    _ => *field = redact_value(field.take()),
                }
            }
        }
        _ => {}
    }
    value
}

fn manifest_for(subject: &ExportSubject) -> Result<ExportManifest, ExportError> {
    let (kind, report_bytes, schemas, receipt_bindings, subject_hashes) = match subject {
        ExportSubject::Analysis { snapshot } => {
            validate_analysis(snapshot)?;
            (
                ExportKind::Analysis,
                canonical_bytes(snapshot)?,
                vec![schema_receipt(
                    "AnalysisSnapshotReport",
                    SCHEMA_VERSION,
                    ANALYSIS_SCHEMA,
                )],
                analysis_bindings(ExportBindingScope::Analysis, snapshot),
                SubjectHashes::Analysis {
                    analysis_key: snapshot.analysis_key.clone(),
                    facts_hash: snapshot.facts_hash.clone(),
                },
            )
        }
        ExportSubject::Comparison { snapshot } => {
            validate_comparison(snapshot)?;
            let mut bindings = vec![binding(ExportBindingScope::Exporter, exporter_receipt())];
            bindings.extend(
                snapshot
                    .comparison_receipt
                    .components
                    .iter()
                    .cloned()
                    .map(|component| binding(ExportBindingScope::Comparison, component)),
            );
            bindings.extend(attempt_bindings(
                ExportBindingScope::Baseline,
                &snapshot.baseline,
            ));
            bindings.extend(attempt_bindings(
                ExportBindingScope::Candidate,
                &snapshot.candidate,
            ));
            (
                ExportKind::Comparison,
                canonical_bytes(snapshot)?,
                vec![
                    schema_receipt("Comparison", COMPARISON_SCHEMA_VERSION, COMPARISON_SCHEMA),
                    schema_receipt("AnalysisSnapshotReport", SCHEMA_VERSION, ANALYSIS_SCHEMA),
                ],
                bindings,
                SubjectHashes::Comparison {
                    comparison_key: snapshot.comparison_key.clone(),
                    baseline: attempt_hash(&snapshot.baseline),
                    candidate: attempt_hash(&snapshot.candidate),
                },
            )
        }
    };
    let byte_length = u32::try_from(report_bytes.len()).map_err(|_| ExportError::ReportTooLarge)?;
    let mut manifest = ExportManifest {
        schema_version: ExportSchemaVersion::V0_1,
        canonicalization: ExportCanonicalization::DoctorCanonicalJsonV1,
        integrity: ExportIntegrity::SelfConsistentNotSigned,
        kind,
        evidence: ExportEvidence::ProductRuntime,
        files: vec![ExportFile {
            path: "report.json".to_owned(),
            media_type: EXPORT_MEDIA_TYPE.to_owned(),
            byte_length,
            sha256: sha256(&report_bytes),
        }],
        schemas,
        receipt_bindings,
        subject_hashes,
        manifest_hash: String::new(),
    };
    manifest.manifest_hash = manifest_hash(&manifest)?;
    Ok(manifest)
}

fn analysis_bindings(
    scope: ExportBindingScope,
    report: &AnalysisSnapshotReport,
) -> Vec<ReceiptBinding> {
    std::iter::once(binding(ExportBindingScope::Exporter, exporter_receipt()))
        .chain(
            report
                .analysis_receipt
                .components
                .iter()
                .cloned()
                .map(|component| binding(scope.clone(), component)),
        )
        .collect()
}

fn attempt_bindings(scope: ExportBindingScope, attempt: &AnalysisAttempt) -> Vec<ReceiptBinding> {
    match attempt {
        AnalysisAttempt::Outcome(outcome) => outcome
            .outcome
            .report
            .analysis_receipt
            .components
            .iter()
            .cloned()
            .map(|component| binding(scope.clone(), component))
            .collect(),
        AnalysisAttempt::Error(_) => Vec::new(),
    }
}

fn attempt_hash(attempt: &AnalysisAttempt) -> SubjectHashAttempt {
    match attempt {
        AnalysisAttempt::Outcome(outcome) => SubjectHashAttempt::Success {
            analysis_key: outcome.outcome.report.analysis_key.clone(),
            facts_hash: outcome.outcome.report.facts_hash.clone(),
        },
        AnalysisAttempt::Error(error) => SubjectHashAttempt::Failed {
            error_state: error.error.code.clone(),
        },
    }
}

fn schema_receipt(id: &str, version: &str, bytes: &[u8]) -> SchemaReceipt {
    SchemaReceipt {
        schema_id: id.to_owned(),
        version: version.to_owned(),
        sha256: sha256(bytes),
    }
}

fn binding(scope: ExportBindingScope, component: ReceiptComponent) -> ReceiptBinding {
    ReceiptBinding { scope, component }
}

fn exporter_receipt() -> ReceiptComponent {
    let semantic = json!({
        "id": EXPORTER_ID,
        "version": EXPORTER_VERSION,
        "contract_version": "0.1",
        "canonicalization": "doctor-canonical-json-v1",
        "redaction": "typed_public_dto_recursive_v1"
    });
    ReceiptComponent {
        id: EXPORTER_ID.to_owned(),
        version: EXPORTER_VERSION.to_owned(),
        manifest_hash: canonical_hash(&semantic),
    }
}

fn manifest_hash(manifest: &ExportManifest) -> Result<String, ExportError> {
    let mut value = serde_json::to_value(manifest).map_err(|_| ExportError::Serialization)?;
    value
        .as_object_mut()
        .ok_or(ExportError::Serialization)?
        .remove("manifest_hash");
    Ok(canonical_hash(&value))
}

fn canonical_bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ExportError> {
    let value = serde_json::to_value(value).map_err(|_| ExportError::Serialization)?;
    Ok(canonical_json(&value).into_bytes())
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn requested_kind_name(kind: &doctor_contracts::RequestedSourceKind) -> &'static str {
    match kind {
        doctor_contracts::RequestedSourceKind::Auto => "auto",
        doctor_contracts::RequestedSourceKind::JsonProfile => "json_profile",
        doctor_contracts::RequestedSourceKind::TextProfile => "text_profile",
    }
}

#[cfg(test)]
mod sql_location_export_tests {
    #[test]
    fn request_local_sql_excerpt_is_removed_recursively() {
        let value = serde_json::json!({"graph_view":{"nodes":[{"node_id":"n", "sql_location":{"state":"located","highlight":"secret_sql"}}]},"graph_reading":{"nodes":[{"sql_location":{"state":"located","highlight":"secret_sql"}}]}});
        let redacted = super::redact_value(value);
        assert!(!redacted.to_string().contains("secret_sql"));
        assert!(!redacted.to_string().contains("sql_location"));
        assert_eq!(redacted["graph_view"]["nodes"][0]["node_id"], "n");
    }
}
