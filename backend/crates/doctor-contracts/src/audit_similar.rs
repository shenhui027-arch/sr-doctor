//! A2 similar-SQL clusters: exact templates (native-digest families and
//! product-fingerprint groups) merged by an explained structural basis. Every
//! cluster states its basis; members can be detached for one request. The
//! structure rules are a product decision, not a StarRocks semantic.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AuditAggregateFact, AuditClusterIdentity, AuditErrorProportion, AuditFamilySort,
    AuditFingerprintCoverage, AuditScope, AuditTopWindow, AuditWorkloadValidationError,
    AuditWorkloadVersion, TimeZoneEvidence,
};

/// Bumped whenever a structure-extraction or clustering rule changes.
pub const SIMILAR_SQL_STRUCTURE_VERSION: &str = "sr-doctor-structure.v3";
pub const MAX_SIMILAR_SQL_DETACHED: usize = 100;
pub const MAX_SIMILAR_SQL_REFINED: usize = 50;
const MAX_SIMILAR_SQL_KEY_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SimilarSqlDimensions {
    pub tables: bool,
    pub join: bool,
    pub predicates: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditSimilarSqlRequest {
    pub scope: AuditScope,
    /// Ranks clusters, and selects which exact families are read.
    pub sort: AuditFamilySort,
    /// The most exact families read, as for the family list.
    pub limit: u16,
    pub dimensions: SimilarSqlDimensions,
    /// Member keys kept out of every cluster for this request only.
    pub detached_member_keys: Vec<String>,
    /// Clusters additionally split by predicate columns (when that dimension is off).
    pub refined_cluster_keys: Vec<String>,
}

impl AuditSimilarSqlRequest {
    pub fn validate(&self) -> Result<(), AuditWorkloadValidationError> {
        crate::AuditFamilyRequest {
            scope: self.scope.clone(),
            sort: self.sort.clone(),
            limit: self.limit,
        }
        .validate()?;
        let keys_ok = |keys: &[String], max: usize| {
            keys.len() <= max
                && keys.iter().all(|key| {
                    !key.is_empty()
                        && key.len() <= MAX_SIMILAR_SQL_KEY_BYTES
                        && key.bytes().all(|byte| byte.is_ascii_graphic())
                })
        };
        let dimensions = &self.dimensions;
        if !(dimensions.tables || dimensions.join || dimensions.predicates)
            || !keys_ok(&self.detached_member_keys, MAX_SIMILAR_SQL_DETACHED)
            || !keys_ok(&self.refined_cluster_keys, MAX_SIMILAR_SQL_REFINED)
        {
            return Err(AuditWorkloadValidationError::FilterText);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditSimilarSqlReadRequest {
    pub connection_id: String,
    pub request: AuditSimilarSqlRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum SimilarSqlMemberSource {
    OfficialDigest {
        native_digest: String,
    },
    /// Made by this product; not a StarRocks digest.
    ProductFingerprint {
        fingerprint: String,
    },
}

/// The explained basis. Lists are sorted and hold only enabled dimensions'
/// facts; a disabled dimension is an empty list plus `false` in `dimensions`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SimilarSqlBasis {
    pub tables: Vec<String>,
    /// e.g. `INNER JOIN customers.id = orders.cid`; empty when there is no join.
    pub joins: Vec<String>,
    pub predicate_columns: Vec<String>,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SimilarSqlUnavailableReason {
    /// The statement was truncated, unreadable, or not normalizable (A1 rules).
    TemplateUnavailable,
    NotSelect,
    CommonTableExpression,
    SetOperation,
    Subquery,
    DerivedTable,
    UnsupportedFromClause,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum SimilarSqlStructure {
    /// The member's full basis, independent of the enabled dimensions.
    Available {
        basis: SimilarSqlBasis,
    },
    Unavailable {
        reason: SimilarSqlUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SimilarSqlMember {
    pub member_key: String,
    pub source: SimilarSqlMemberSource,
    pub catalog: String,
    pub database: String,
    /// Literal-free template; for an official family, normalized from its
    /// most recent member's statement. `None` when it could not be made.
    pub template: Option<String>,
    pub structure: SimilarSqlStructure,
    pub detached: bool,
    pub execution_count: String,
    pub error_count: String,
    pub error_proportion: AuditErrorProportion,
    pub aggregates: Vec<AuditAggregateFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SimilarSqlCluster {
    pub cluster_key: String,
    pub catalog: String,
    pub database: String,
    pub basis: SimilarSqlBasis,
    /// Split by predicate columns at the user's request.
    pub refined: bool,
    pub members: Vec<SimilarSqlMember>,
    pub execution_count: String,
    pub error_count: String,
    pub error_proportion: AuditErrorProportion,
    /// Summed over members; `source_nonnegative_count` says how many
    /// executions carried each metric.
    pub aggregates: Vec<AuditAggregateFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum SimilarSqlFamilySource {
    Complete,
    /// Only the first `read` exact families by the requested sort were read.
    Partial {
        read: u16,
        in_window: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SimilarSqlCoverage {
    pub exact_families: SimilarSqlFamilySource,
    pub fingerprint: AuditFingerprintCoverage,
    pub member_count: String,
    pub clustered_member_count: String,
    pub independent_member_count: String,
    pub unavailable_member_count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditSimilarSqlResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub scope: AuditScope,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub structure_version: String,
    pub dimensions: SimilarSqlDimensions,
    pub clusters: Vec<SimilarSqlCluster>,
    /// Members with a structure but no similar peer, plus detached members.
    pub independent: Vec<SimilarSqlMember>,
    /// Members whose structure could not be extracted; never clustered.
    pub unavailable: Vec<SimilarSqlMember>,
    pub top: AuditTopWindow,
    pub coverage: SimilarSqlCoverage,
    pub limitations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuditStatementFilter, AuditWindow};

    fn request() -> AuditSimilarSqlRequest {
        AuditSimilarSqlRequest {
            scope: AuditScope {
                window: AuditWindow {
                    start_seconds: 1,
                    end_seconds: 60,
                },
                state: None,
                user: None,
                native_digest: None,
                catalog: None,
                database: None,
                resource_group: None,
                statement_filter: AuditStatementFilter::All,
            },
            sort: AuditFamilySort::CumulativeQueryTimeDesc,
            limit: 20,
            dimensions: SimilarSqlDimensions {
                tables: true,
                join: true,
                predicates: true,
            },
            detached_member_keys: vec![],
            refined_cluster_keys: vec![],
        }
    }

    #[test]
    fn requests_need_a_dimension_and_bounded_printable_keys() {
        assert!(request().validate().is_ok());
        let mut none = request();
        none.dimensions = SimilarSqlDimensions {
            tables: false,
            join: false,
            predicates: false,
        };
        assert!(none.validate().is_err());
        let mut long = request();
        long.detached_member_keys = vec!["k".repeat(129)];
        assert!(long.validate().is_err());
        let mut many = request();
        many.refined_cluster_keys = vec!["k".into(); MAX_SIMILAR_SQL_REFINED + 1];
        assert!(many.validate().is_err());
        let mut spaced = request();
        spaced.detached_member_keys = vec!["a b".into()];
        assert!(spaced.validate().is_err());
    }
}
