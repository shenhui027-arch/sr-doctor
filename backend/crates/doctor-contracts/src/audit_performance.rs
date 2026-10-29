//! Window performance overview over the same canonical Audit cohort as the
//! cards and Top tables. Every number is a server fact; permyriad values are
//! layout projections only. Neither SQL nor Profile text is carried here.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AuditAggregateFact, AuditClusterIdentity, AuditConcurrencyTimeTick,
    AuditFamilyDistributionExclusions, AuditFamilyHistogramBucket, AuditMetricFact,
    AuditOptionalFactUnavailableReason, AuditScope, TimeZoneEvidence,
};

/// Fixed bucket widths. `Auto` picks the narrowest one that keeps the window
/// within [`AUDIT_PERFORMANCE_AUTO_BUCKET_LIMIT`] buckets.
pub const AUDIT_PERFORMANCE_BUCKET_SECONDS: [u32; 6] = [60, 300, 900, 3_600, 21_600, 86_400];
pub const AUDIT_PERFORMANCE_AUTO_BUCKET_LIMIT: u32 = 200;
pub const AUDIT_PERFORMANCE_MAX_BUCKETS: u32 = 1_440;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
pub enum AuditPerformanceVersion {
    #[serde(rename = "audit-performance.v1")]
    #[ts(rename = "audit-performance.v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditPerformanceBucketChoice {
    Auto,
    Fixed { seconds: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditPerformanceBucketError {
    UnsupportedWidth,
    TooManyBuckets,
}

impl AuditPerformanceBucketChoice {
    /// Resolve against an already validated window length.
    pub fn resolve(&self, window_seconds: u32) -> Result<u32, AuditPerformanceBucketError> {
        let count = |width: u32| window_seconds.div_ceil(width);
        match self {
            Self::Auto => Ok(AUDIT_PERFORMANCE_BUCKET_SECONDS
                .into_iter()
                .find(|width| count(*width) <= AUDIT_PERFORMANCE_AUTO_BUCKET_LIMIT)
                .unwrap_or(86_400)),
            Self::Fixed { seconds } => {
                if !AUDIT_PERFORMANCE_BUCKET_SECONDS.contains(seconds) {
                    return Err(AuditPerformanceBucketError::UnsupportedWidth);
                }
                if count(*seconds) > AUDIT_PERFORMANCE_MAX_BUCKETS {
                    return Err(AuditPerformanceBucketError::TooManyBuckets);
                }
                Ok(*seconds)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceReadRequest {
    pub connection_id: String,
    pub scope: AuditScope,
    pub bucket: AuditPerformanceBucketChoice,
}

/// Exact nearest-rank quantiles (`PERCENTILE_DISC`) over the eligible values:
/// `0 <= value <= ceiling`. Absent quantiles mean no eligible value, never 0.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceQuantiles {
    pub eligible_count: String,
    pub p50: Option<AuditMetricFact>,
    pub p95: Option<AuditMetricFact>,
    pub p99: Option<AuditMetricFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditPerformanceOptionalQuantiles {
    Available {
        quantiles: Box<AuditPerformanceQuantiles>,
        /// Executions whose duplicate source rows disagree on this field.
        conflicting_count: String,
    },
    Unavailable {
        reason: AuditOptionalFactUnavailableReason,
    },
}

/// A sum or maximum plus the executions it could not include (SQL NULL or a
/// negative sentinel). `excluded_count` is exact; it is never folded into 0.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceAggregate {
    pub aggregate: AuditAggregateFact,
    pub excluded_count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceSummary {
    pub execution_count: String,
    /// `state = ERR` executions.
    pub error_count: String,
    /// Failed share of the executions; `None` when there are none.
    pub error_share_permyriad: Option<u16>,
    pub query_time: AuditPerformanceQuantiles,
    pub query_time_sum: AuditPerformanceAggregate,
    pub scan_bytes_sum: AuditPerformanceAggregate,
    /// Summed CPU time, not elapsed time.
    pub cpu_cost_sum: AuditPerformanceAggregate,
    /// Largest single-execution memory, not concurrent total memory.
    pub memory_cost_max: AuditPerformanceAggregate,
    pub pending_time: AuditPerformanceOptionalQuantiles,
}

/// Layout-only heights. Count series share one linear axis (errors stack on
/// successes); `query_time_*` use the fixed log axis; others are linear to
/// their own server-chosen axis maximum. `None` means nothing to draw.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceProjection {
    pub execution_permyriad: u16,
    pub error_permyriad: u16,
    pub query_time_p50_permyriad: Option<u16>,
    pub query_time_p95_permyriad: Option<u16>,
    pub query_time_p99_permyriad: Option<u16>,
    pub scan_bytes_permyriad: Option<u16>,
    pub cpu_cost_permyriad: Option<u16>,
    pub memory_cost_permyriad: Option<u16>,
    pub pending_time_p95_permyriad: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceBucket {
    /// Reader-zone epoch coordinates, like every other Audit timestamp.
    pub start_milliseconds: String,
    pub end_milliseconds: String,
    pub x_permyriad: u16,
    pub width_permyriad: u16,
    pub summary: AuditPerformanceSummary,
    pub projection: AuditPerformanceProjection,
    /// The bucket ends within the AuditLoader ingestion grace of the read, so
    /// rows may still be arriving. It is drawn, never extrapolated.
    pub recent_ingestion: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum AuditPerformanceTickValue {
    Count { count: String },
    Metric { metric: AuditMetricFact },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceValueTick {
    pub y_permyriad: u16,
    pub value: AuditPerformanceTickValue,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceAxes {
    pub time_ticks: Vec<AuditConcurrencyTimeTick>,
    pub execution: Vec<AuditPerformanceValueTick>,
    pub query_time_log: Vec<AuditPerformanceValueTick>,
    pub scan_bytes: Vec<AuditPerformanceValueTick>,
    pub cpu_cost: Vec<AuditPerformanceValueTick>,
    pub memory_cost: Vec<AuditPerformanceValueTick>,
    pub pending_time: Vec<AuditPerformanceValueTick>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceHistogramBucket {
    pub bucket: AuditFamilyHistogramBucket,
    /// Share of the eligible executions.
    pub share_permyriad: u16,
    /// Bar length relative to the fullest bucket.
    pub width_permyriad: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceSlowCount {
    pub threshold_ms: String,
    pub count: String,
    pub share_permyriad: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceHistogram {
    pub eligible_count: String,
    pub exclusions: AuditFamilyDistributionExclusions,
    pub buckets: Vec<AuditPerformanceHistogramBucket>,
    /// Executions at or above each fixed bucket boundary. A marker, not a verdict.
    pub slow: Vec<AuditPerformanceSlowCount>,
}

/// A grouped source value. `SqlNull` and `Conflicting` (duplicate rows of one
/// execution disagree) stay distinct from every text value, including "".
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditPerformanceValue {
    Text { value: String },
    SqlNull,
    Conflicting,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceErrorRow {
    pub code: AuditPerformanceValue,
    pub count: String,
    /// Share of all failed executions in the window.
    pub share_permyriad: u16,
    pub last_event_milliseconds: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditPerformanceErrors {
    Available {
        error_count: String,
        rows: Vec<AuditPerformanceErrorRow>,
        /// Failed executions and distinct codes beyond the returned rows.
        remainder_count: String,
        remainder_code_count: String,
    },
    Unavailable {
        error_count: String,
        reason: AuditOptionalFactUnavailableReason,
    },
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditPerformanceDimensionId {
    User,
    Database,
    ResourceGroup,
    Catalog,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditPerformanceRankKey {
    ExecutionCount,
    QueryTimeSum,
    ErrorCount,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceDimensionRow {
    pub value: AuditPerformanceValue,
    pub execution_count: String,
    pub error_count: String,
    pub query_time_sum: AuditPerformanceAggregate,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceRankEntry {
    pub row_index: u16,
    /// Share of the window total for this key; `None` when that total is 0.
    pub share_permyriad: Option<u16>,
    pub width_permyriad: u16,
}

/// Exact Top-N for one key, ranked by the source over every value of the
/// dimension, plus what lies beyond it. Values with nothing to rank (0 or an
/// absent sum) are never listed for that key.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceRanking {
    pub key: AuditPerformanceRankKey,
    pub entries: Vec<AuditPerformanceRankEntry>,
    pub remainder_value_count: String,
    pub remainder_share_permyriad: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditPerformanceDimensionState {
    Available {
        distinct_value_count: String,
        rows: Vec<AuditPerformanceDimensionRow>,
        rankings: Vec<AuditPerformanceRanking>,
    },
    Unavailable {
        reason: AuditOptionalFactUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceDimension {
    pub dimension: AuditPerformanceDimensionId,
    pub state: AuditPerformanceDimensionState,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditPerformanceResponse {
    pub schema_version: AuditPerformanceVersion,
    pub connection_id: String,
    pub scope: AuditScope,
    pub bucket_seconds: u32,
    pub read_at_milliseconds: String,
    pub window: AuditPerformanceSummary,
    pub buckets: Vec<AuditPerformanceBucket>,
    pub axes: AuditPerformanceAxes,
    pub latency_histogram: AuditPerformanceHistogram,
    pub errors: AuditPerformanceErrors,
    pub dimensions: Vec<AuditPerformanceDimension>,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub limitations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_bucket_keeps_each_preset_window_readable() {
        let auto = AuditPerformanceBucketChoice::Auto;
        assert_eq!(auto.resolve(3_600), Ok(60));
        assert_eq!(auto.resolve(6 * 3_600), Ok(300));
        assert_eq!(auto.resolve(24 * 3_600), Ok(900));
        assert_eq!(auto.resolve(7 * 86_400), Ok(3_600));
        assert_eq!(auto.resolve(30 * 86_400), Ok(21_600));
    }

    #[test]
    fn fixed_bucket_rejects_unknown_widths_and_oversized_series() {
        assert_eq!(
            AuditPerformanceBucketChoice::Fixed { seconds: 120 }.resolve(3_600),
            Err(AuditPerformanceBucketError::UnsupportedWidth)
        );
        assert_eq!(
            AuditPerformanceBucketChoice::Fixed { seconds: 60 }.resolve(2 * 86_400),
            Err(AuditPerformanceBucketError::TooManyBuckets)
        );
        assert_eq!(
            AuditPerformanceBucketChoice::Fixed { seconds: 60 }.resolve(86_400),
            Ok(60)
        );
    }
}
