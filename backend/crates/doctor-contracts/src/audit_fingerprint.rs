//! Product SQL fingerprints for audit rows that carry no native digest. They
//! are never an official StarRocks digest and never share a family key, list
//! or ranking with exact native-digest families.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::{
    AuditAggregateFact, AuditClusterIdentity, AuditErrorProportion, AuditFamilyContext, AuditScope,
    AuditTopWindow, AuditWorkloadVersion, TimeZoneEvidence,
};

/// Bumped whenever a normalization rule changes; fingerprints of different
/// versions are never comparable.
pub const PRODUCT_SQL_FINGERPRINT_VERSION: &str = "sr-doctor-fingerprint.v3";

/// The most digest-missing executions one fingerprint read takes, newest first.
pub const AUDIT_FINGERPRINT_READ_LIMIT: u16 = 500;

/// The longest `stmt` prefix, in characters, a fingerprint read takes. A
/// longer statement is `partial` and is not grouped.
pub const AUDIT_FINGERPRINT_STATEMENT_CHARS: u16 = 8192;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum ProductSqlFingerprint {
    /// `template` keeps identifiers and structure; every literal is `?`.
    Exact {
        fingerprint: String,
        template: String,
    },
    Partial {
        reason: ProductSqlFingerprintPartialReason,
    },
    Unavailable {
        reason: ProductSqlFingerprintUnavailableReason,
    },
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ProductSqlFingerprintPartialReason {
    /// Only a prefix of the audited statement was read.
    StatementTruncated,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ProductSqlFingerprintUnavailableReason {
    StatementSqlNull,
    /// The statement read returned no row for this execution.
    StatementNotRead,
    /// Duplicate audit rows of one execution disagree on `stmt`.
    ConflictingSourceRows,
    EmptyStatement,
    /// Unterminated quote, comment or identifier, or a character outside the
    /// fixed-version lexer (for example `#`).
    LexicalError,
    UnbalancedParentheses,
    MultipleStatements,
    /// Only SELECT / WITH / INSERT / UPDATE / DELETE get an official digest.
    UnsupportedStatement,
    /// The official digest replaces 16+ flattened AND/OR terms with their
    /// column set; this product does not reproduce that rule.
    MassiveCompoundPredicate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ProductFingerprintAuthority {
    /// Made by this product; not a StarRocks digest.
    ProductUnofficial,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFingerprintFamilyPresentation {
    pub family_key: String,
    pub fingerprint: String,
    pub template: String,
    pub context: AuditFamilyContext,
    pub execution_count: String,
    pub error_count: String,
    pub error_proportion: AuditErrorProportion,
    pub aggregates: Vec<AuditAggregateFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditFingerprintSourceCoverage {
    /// Every eligible execution in the window was read.
    Complete,
    /// Only the newest `read_limit` eligible executions were read.
    Partial {
        reason: AuditFingerprintPartialReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditFingerprintPartialReason {
    ReadLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFingerprintUnavailableCount {
    pub reason: ProductSqlFingerprintUnavailableReason,
    pub count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFingerprintCoverage {
    /// Canonical executions in the scope with a complete context and no valid
    /// native digest. Executions missing context are not eligible.
    pub eligible_event_count: String,
    pub read_limit: u16,
    pub read_event_count: String,
    pub source: AuditFingerprintSourceCoverage,
    pub fingerprinted_event_count: String,
    /// Truncated statements: read, not grouped, not ranked.
    pub partial_event_count: String,
    pub unavailable: Vec<AuditFingerprintUnavailableCount>,
    pub family_count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFingerprintFamilyResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub scope: AuditScope,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub authority: ProductFingerprintAuthority,
    pub fingerprint_version: String,
    pub rows: Vec<AuditFingerprintFamilyPresentation>,
    pub coverage: AuditFingerprintCoverage,
    pub top: AuditTopWindow,
    pub limitations: Vec<String>,
}

/// A product family key: fingerprint version, fingerprint and the complete
/// context. Its prefix never equals an exact native-digest family key.
pub fn audit_fingerprint_family_key(fingerprint: &str, context: &AuditFamilyContext) -> String {
    let mut hasher = Sha256::new();
    for value in [
        PRODUCT_SQL_FINGERPRINT_VERSION,
        fingerprint,
        context.cluster.as_deref().unwrap_or_default(),
        context.engine_version.as_deref().unwrap_or_default(),
        context.catalog.as_deref().unwrap_or_default(),
        context.database.as_deref().unwrap_or_default(),
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("audit-fingerprint-v1:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_family_keys_are_context_scoped_and_never_native() {
        let context = AuditFamilyContext {
            cluster: Some("session:a".into()),
            engine_version: Some("3.5.14".into()),
            catalog: Some("default_catalog".into()),
            database: Some("db".into()),
        };
        let key = audit_fingerprint_family_key("ab", &context);
        assert!(key.starts_with("audit-fingerprint-v1:"));
        assert_eq!(key, audit_fingerprint_family_key("ab", &context.clone()));
        assert_ne!(
            key,
            audit_fingerprint_family_key(
                "ab",
                &AuditFamilyContext {
                    database: Some("other".into()),
                    ..context
                }
            )
        );
    }

    #[test]
    fn fingerprint_states_keep_distinct_wire_values() {
        for (value, wire) in [
            (
                ProductSqlFingerprint::Exact {
                    fingerprint: "ab".into(),
                    template: "SELECT ?".into(),
                },
                serde_json::json!({ "state": "exact", "fingerprint": "ab", "template": "SELECT ?" }),
            ),
            (
                ProductSqlFingerprint::Partial {
                    reason: ProductSqlFingerprintPartialReason::StatementTruncated,
                },
                serde_json::json!({ "state": "partial", "reason": "statement_truncated" }),
            ),
            (
                ProductSqlFingerprint::Unavailable {
                    reason: ProductSqlFingerprintUnavailableReason::MassiveCompoundPredicate,
                },
                serde_json::json!({ "state": "unavailable", "reason": "massive_compound_predicate" }),
            ),
        ] {
            assert_eq!(serde_json::to_value(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<ProductSqlFingerprint>(wire).unwrap(),
                value
            );
        }
        assert_eq!(
            serde_json::to_value(ProductFingerprintAuthority::ProductUnofficial).unwrap(),
            "product_unofficial"
        );
    }
}
