//! Versioned, lossless AuditLoader workload contracts.
//!
//! Audit metric parsing deliberately remains in `doctor-core::metrics`.  This
//! module only preserves SQL NULL versus the source text and carries the
//! normalization result that the single parser/policy adapter produces.

use std::{error::Error, fmt};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::{CanonicalNumber, ParseState};

pub const AUDIT_WORKLOAD_CONTRACT_VERSION: &str = "audit-workload.v1";
/// The longest user-selectable AuditLoader window is the rolling thirty-day
/// month preset. Interval reconstruction may read a separately disclosed,
/// fixed lookback before this public window.
pub const MAX_AUDIT_WINDOW_SECONDS: u64 = 30 * 24 * 60 * 60;
/// `UNIX_TIMESTAMP` / `FROM_UNIXTIME` interoperability range accepted by the
/// fixed 3.5.14 connector path. `u32` also remains exact in a JSON number.
pub const MAX_AUDIT_EPOCH_SECONDS: u32 = 2_147_483_647;
pub const MAX_AUDIT_OCCURRENCE_LIMIT: u16 = 500;
pub const MAX_AUDIT_FAMILY_LIMIT: u16 = 200;
pub const MAX_AUDIT_FILTER_TEXT_BYTES: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditWindow {
    /// Inclusive Unix second. This is a `u32`, never a lossy JavaScript `u64`.
    pub start_seconds: u32,
    /// Exclusive Unix second. Adjacent windows therefore cannot overlap.
    pub end_seconds: u32,
}

impl AuditWindow {
    pub fn validate(&self) -> Result<(), AuditWorkloadValidationError> {
        if self.start_seconds == 0 || self.end_seconds > MAX_AUDIT_EPOCH_SECONDS {
            return Err(AuditWorkloadValidationError::EpochRange);
        }
        if self.start_seconds >= self.end_seconds {
            return Err(AuditWorkloadValidationError::WindowOrder);
        }
        if u64::from(self.end_seconds - self.start_seconds) > MAX_AUDIT_WINDOW_SECONDS {
            return Err(AuditWorkloadValidationError::WindowTooWide);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditTerminalState {
    Finished,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditScope {
    pub window: AuditWindow,
    pub state: Option<AuditTerminalState>,
    /// Optional exact AuditLoader source-user filter. The source column is
    /// capability-probed; a requested value never falls back to an unfiltered
    /// read when that column is absent.
    pub user: Option<String>,
    pub native_digest: Option<String>,
    pub catalog: Option<String>,
    pub database: Option<String>,
    pub resource_group: Option<String>,
    /// Explicit, user-visible statement-class filter. It is part of the scope
    /// so every Audit read (window cards, Top tables, families, concurrency,
    /// SQL lookup) uses the same population; hidden rows stay countable.
    pub statement_filter: AuditStatementFilter,
}

/// Product-owned statement-class filter over AuditLoader rows.
///
/// `ExcludeDoctorAndNonQuery` keeps only rows whose FE-reported `isQuery` is
/// exactly 1 (official 3.5.14 `ConnectContext.isQueryStmt`: a `QueryStatement`
/// or EXECUTE of a prepared query; statements that fail to parse keep the
/// initial `true`) and whose `stmt` does not start with this product's fixed
/// statement marker. SET/SHOW/DDL/DML rows, rows with a NULL `isQuery`, and
/// this tool's own statements are hidden but counted.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditStatementFilter {
    // Default only where a field opts into `serde(default)` for compatibility
    // with earlier requests; `AuditScope` keeps the filter required.
    #[default]
    All,
    ExcludeDoctorAndNonQuery,
}

impl AuditStatementFilter {
    pub fn is_applied(self) -> bool {
        self == Self::ExcludeDoctorAndNonQuery
    }
}

impl AuditScope {
    pub fn validate(&self) -> Result<(), AuditWorkloadValidationError> {
        self.window.validate()?;
        if self
            .native_digest
            .as_deref()
            .is_some_and(|value| !valid_native_digest(value))
        {
            return Err(AuditWorkloadValidationError::NativeDigest);
        }
        for value in [
            &self.user,
            &self.catalog,
            &self.database,
            &self.resource_group,
        ]
        .into_iter()
        .flatten()
        {
            if !valid_filter_text(value) {
                return Err(AuditWorkloadValidationError::FilterText);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditOccurrenceSort {
    QueryTimeDesc,
    ScanBytesDesc,
    ScanRowsDesc,
    CpuCostDesc,
    MemoryCostDesc,
    NewestFirst,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOccurrenceRequest {
    pub scope: AuditScope,
    pub sort: AuditOccurrenceSort,
    pub limit: u16,
}

impl AuditOccurrenceRequest {
    pub fn validate(&self) -> Result<(), AuditWorkloadValidationError> {
        self.scope.validate()?;
        if !(1..=MAX_AUDIT_OCCURRENCE_LIMIT).contains(&self.limit) {
            return Err(AuditWorkloadValidationError::OccurrenceLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditFamilySort {
    CumulativeQueryTimeDesc,
    CumulativeScanBytesDesc,
    CumulativeCpuCostDesc,
    ExecutionCountDesc,
    ErrorCountDesc,
    ErrorProportionDesc,
    MaxQueryTimeDesc,
    MaxMemoryCostDesc,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyRequest {
    pub scope: AuditScope,
    pub sort: AuditFamilySort,
    pub limit: u16,
}

impl AuditFamilyRequest {
    pub fn validate(&self) -> Result<(), AuditWorkloadValidationError> {
        self.scope.validate()?;
        if !(1..=MAX_AUDIT_FAMILY_LIMIT).contains(&self.limit) {
            return Err(AuditWorkloadValidationError::FamilyLimit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditWorkloadValidationError {
    WindowOrder,
    WindowTooWide,
    EpochRange,
    NativeDigest,
    FilterText,
    OccurrenceLimit,
    FamilyLimit,
}

impl fmt::Display for AuditWorkloadValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WindowOrder => "audit_window_order",
            Self::WindowTooWide => "audit_window_too_wide",
            Self::EpochRange => "audit_epoch_range",
            Self::NativeDigest => "audit_native_digest",
            Self::FilterText => "audit_filter_text",
            Self::OccurrenceLimit => "audit_occurrence_limit",
            Self::FamilyLimit => "audit_family_limit",
        })
    }
}

impl Error for AuditWorkloadValidationError {}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditMetricId {
    QueryTime,
    PendingTime,
    ScanBytes,
    ScanRows,
    CpuCost,
    MemoryCost,
    ReturnRows,
}

/// The existing fixed schema probe did not prove an optional AuditLoader
/// source column. This is deliberately distinct from a present SQL NULL.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditOptionalFactUnavailableReason {
    SourceColumnAbsent,
    ConflictingSourceRows,
}

/// Lossless transport input. `Text("0")` is deliberately distinct from SQL
/// NULL and remains decimal text so JavaScript never receives a lossy number.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditMetricInput {
    SqlNull,
    Text {
        value: String,
    },
    Unavailable {
        reason: AuditOptionalFactUnavailableReason,
    },
}

impl AuditMetricInput {
    pub fn from_sql_cell(value: Option<String>) -> Self {
        match value {
            Some(value) => Self::Text { value },
            None => Self::SqlNull,
        }
    }
}

/// This is a result state, not a second parser. The core AuditLoader adapter
/// must map its one typed-parser outcome and audit-only negative policy here.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditMetricState {
    Exact,
    Missing,
    Invalid,
    NegativeSentinel,
    Overflow,
    PrecisionUnsafe,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditMetricFact {
    pub metric_id: AuditMetricId,
    pub input: AuditMetricInput,
    pub state: AuditMetricState,
    pub parser_state: Option<ParseState>,
    /// Decimal text only; never encode audit counters as a JSON number.
    pub canonical_number: Option<CanonicalNumber>,
    pub canonical_unit: Option<String>,
}

impl AuditMetricFact {
    pub fn validate(&self) -> bool {
        match self.state {
            AuditMetricState::Exact | AuditMetricState::PrecisionUnsafe => {
                self.canonical_number.is_some() && self.canonical_unit.is_some()
            }
            AuditMetricState::Missing
            | AuditMetricState::Invalid
            | AuditMetricState::NegativeSentinel
            | AuditMetricState::Overflow
            | AuditMetricState::Unavailable => {
                self.canonical_number.is_none() && self.canonical_unit.is_none()
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditQueryIdentityState {
    Unique,
    Duplicate,
    Missing,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyContext {
    pub cluster: Option<String>,
    pub engine_version: Option<String>,
    pub catalog: Option<String>,
    pub database: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditUngroupedReason {
    NativeDigestMissing,
    NativeDigestInvalid,
    ClusterMissing,
    ClusterInvalid,
    EngineVersionMissing,
    EngineVersionInvalid,
    CatalogMissing,
    CatalogInvalid,
    DatabaseMissing,
    DatabaseInvalid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditFamilyMembership {
    ExactNativeDigest {
        family_key: String,
        native_digest: String,
        context: AuditFamilyContext,
    },
    Ungrouped {
        reason: AuditUngroupedReason,
    },
}

/// The only V2-1 family key: source-native digest plus the complete context.
/// It never hashes SQL text and never falls back to a product-made digest.
pub fn audit_family_membership(
    native_digest: Option<&str>,
    context: AuditFamilyContext,
) -> AuditFamilyMembership {
    let Some(native_digest) = native_digest.filter(|value| !value.is_empty()) else {
        return AuditFamilyMembership::Ungrouped {
            reason: AuditUngroupedReason::NativeDigestMissing,
        };
    };
    if !valid_native_digest(native_digest) {
        return AuditFamilyMembership::Ungrouped {
            reason: AuditUngroupedReason::NativeDigestInvalid,
        };
    }
    let Some(cluster) = valid_context_value(context.cluster.as_deref()) else {
        return AuditFamilyMembership::Ungrouped {
            reason: if context
                .cluster
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                AuditUngroupedReason::ClusterInvalid
            } else {
                AuditUngroupedReason::ClusterMissing
            },
        };
    };
    let Some(engine_version) = valid_context_value(context.engine_version.as_deref()) else {
        return AuditFamilyMembership::Ungrouped {
            reason: if context
                .engine_version
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                AuditUngroupedReason::EngineVersionInvalid
            } else {
                AuditUngroupedReason::EngineVersionMissing
            },
        };
    };
    let Some(catalog) = valid_context_value(context.catalog.as_deref()) else {
        return AuditFamilyMembership::Ungrouped {
            reason: if context
                .catalog
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                AuditUngroupedReason::CatalogInvalid
            } else {
                AuditUngroupedReason::CatalogMissing
            },
        };
    };
    let Some(database) = valid_context_value(context.database.as_deref()) else {
        return AuditFamilyMembership::Ungrouped {
            reason: if context
                .database
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                AuditUngroupedReason::DatabaseInvalid
            } else {
                AuditUngroupedReason::DatabaseMissing
            },
        };
    };
    let mut hasher = Sha256::new();
    for value in [
        AUDIT_WORKLOAD_CONTRACT_VERSION,
        native_digest,
        cluster,
        engine_version,
        catalog,
        database,
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    AuditFamilyMembership::ExactNativeDigest {
        family_key: format!("audit-family-v1:{:x}", hasher.finalize()),
        native_digest: native_digest.to_owned(),
        context,
    }
}

pub fn valid_native_digest(value: &str) -> bool {
    (1..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_filter_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_AUDIT_FILTER_TEXT_BYTES
        && !value.as_bytes().contains(&0)
}

fn valid_context_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| valid_filter_text(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NumberKind;

    fn scope() -> AuditScope {
        AuditScope {
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
        }
    }

    #[test]
    fn audit_window_and_limits_are_bounded() {
        assert!(scope().validate().is_ok());
        assert!(
            AuditWindow {
                start_seconds: 1,
                end_seconds: 1 + MAX_AUDIT_WINDOW_SECONDS as u32,
            }
            .validate()
            .is_ok()
        );
        for user in [
            "",
            &"a".repeat(MAX_AUDIT_FILTER_TEXT_BYTES + 1),
            "reader\0name",
        ] {
            let mut invalid = scope();
            invalid.user = Some(user.to_owned());
            assert_eq!(
                invalid.validate().unwrap_err(),
                AuditWorkloadValidationError::FilterText
            );
        }
        let mut utf8_user = scope();
        utf8_user.user = Some("分析者".into());
        assert!(utf8_user.validate().is_ok());
        assert_eq!(
            AuditWindow {
                start_seconds: 2,
                end_seconds: 1
            }
            .validate()
            .unwrap_err(),
            AuditWorkloadValidationError::WindowOrder
        );
        assert_eq!(
            AuditWindow {
                start_seconds: 1,
                end_seconds: 1 + MAX_AUDIT_WINDOW_SECONDS as u32 + 1
            }
            .validate()
            .unwrap_err(),
            AuditWorkloadValidationError::WindowTooWide
        );
        assert_eq!(
            AuditWindow {
                start_seconds: 0,
                end_seconds: 1
            }
            .validate()
            .unwrap_err(),
            AuditWorkloadValidationError::EpochRange
        );
        assert_eq!(
            AuditWindow {
                start_seconds: 1,
                end_seconds: MAX_AUDIT_EPOCH_SECONDS + 1,
            }
            .validate()
            .unwrap_err(),
            AuditWorkloadValidationError::EpochRange
        );
        assert_eq!(
            AuditOccurrenceRequest {
                scope: scope(),
                sort: AuditOccurrenceSort::QueryTimeDesc,
                limit: MAX_AUDIT_OCCURRENCE_LIMIT + 1,
            }
            .validate()
            .unwrap_err(),
            AuditWorkloadValidationError::OccurrenceLimit
        );
    }

    #[test]
    fn metric_input_and_quality_states_preserve_zero_null_and_precision() {
        assert_ne!(
            AuditMetricInput::from_sql_cell(None),
            AuditMetricInput::from_sql_cell(Some("0".into()))
        );
        let exact = AuditMetricFact {
            metric_id: AuditMetricId::ScanBytes,
            input: AuditMetricInput::from_sql_cell(Some("0".into())),
            state: AuditMetricState::Exact,
            parser_state: Some(ParseState::Exact),
            canonical_number: Some(CanonicalNumber {
                kind: NumberKind::Integer,
                value: "0".into(),
                scale: None,
            }),
            canonical_unit: Some("byte".into()),
        };
        assert!(exact.validate());
        for state in [
            AuditMetricState::Invalid,
            AuditMetricState::NegativeSentinel,
            AuditMetricState::Overflow,
        ] {
            assert_ne!(state, AuditMetricState::Missing);
            assert!(
                AuditMetricFact {
                    metric_id: AuditMetricId::ScanBytes,
                    input: AuditMetricInput::from_sql_cell(Some("not-a-number".into())),
                    state,
                    parser_state: Some(ParseState::Invalid),
                    canonical_number: None,
                    canonical_unit: None,
                }
                .validate()
            );
        }
        let precision = AuditMetricFact {
            metric_id: AuditMetricId::CpuCost,
            input: AuditMetricInput::from_sql_cell(Some("9007199254740992".into())),
            state: AuditMetricState::PrecisionUnsafe,
            parser_state: Some(ParseState::Exact),
            canonical_number: Some(CanonicalNumber {
                kind: NumberKind::Integer,
                value: "9007199254740992".into(),
                scale: None,
            }),
            canonical_unit: Some("ns".into()),
        };
        assert!(precision.validate());
        assert_ne!(precision.canonical_number, exact.canonical_number);
    }

    #[test]
    fn exact_family_never_groups_missing_or_cross_context_digest() {
        let context = AuditFamilyContext {
            cluster: Some("cluster-a".into()),
            engine_version: Some("3.5.14".into()),
            catalog: Some("internal".into()),
            database: Some("analytics".into()),
        };
        let first = audit_family_membership(Some("a0b1"), context.clone());
        let second = audit_family_membership(Some("a0b1"), context.clone());
        assert_eq!(first, second);
        assert!(matches!(
            first,
            AuditFamilyMembership::ExactNativeDigest { .. }
        ));
        assert!(matches!(
            audit_family_membership(
                Some("a0b1"),
                AuditFamilyContext {
                    database: None,
                    ..context
                }
            ),
            AuditFamilyMembership::Ungrouped {
                reason: AuditUngroupedReason::DatabaseMissing
            }
        ));
        assert!(matches!(
            audit_family_membership(
                None,
                AuditFamilyContext {
                    cluster: Some("cluster-a".into()),
                    engine_version: Some("3.5.14".into()),
                    catalog: Some("internal".into()),
                    database: Some("analytics".into()),
                }
            ),
            AuditFamilyMembership::Ungrouped {
                reason: AuditUngroupedReason::NativeDigestMissing
            }
        ));
    }
}
