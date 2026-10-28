//! Public audit workload projections. SQL text exists only in the explicit
//! lookup response; these list and family types never carry a statement.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AuditFamilyMembership, AuditFamilyRequest, AuditFamilySort, AuditMetricFact, AuditMetricId,
    AuditOccurrenceRequest, AuditOccurrenceSort, AuditOptionalFactUnavailableReason, AuditScope,
    ConnectedQueryRef, TimeZoneEvidence,
};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOccurrenceReadRequest {
    pub connection_id: String,
    pub request: AuditOccurrenceRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyReadRequest {
    pub connection_id: String,
    pub request: AuditFamilyRequest,
}

/// One fixed-window batch for the Overview.  The server owns every aggregate,
/// sort and chart coordinate; this request never accepts a SQL fragment or a
/// caller-selected bin size.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOverviewReadRequest {
    pub connection_id: String,
    pub request: AuditOverviewRequest,
}

/// Explicit historical in-flight drill. The selected time is decimal text so
/// JavaScript never rounds an epoch-millisecond coordinate before the server
/// validates it against the current Overview discovery.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditActiveAtReadRequest {
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub at_milliseconds: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOverviewRequest {
    pub scope: AuditScope,
    pub primary: AuditOverviewPrimary,
    pub family_top_sort: AuditOverviewFamilyTopSort,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditOverviewPrimary {
    Occurrences {
        sort: AuditOccurrenceSort,
        limit: u16,
    },
    Families {
        sort: AuditFamilySort,
        limit: u16,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditOverviewFamilyTopSort {
    CumulativeQueryTimeDesc,
    CumulativeScanBytesDesc,
    CumulativeCpuCostDesc,
    ExecutionCountDesc,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditSqlReadRequest {
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub query_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditWorkloadVersion {
    #[serde(rename = "audit-workload.v1")]
    #[ts(rename = "audit-workload.v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditClusterIdentity {
    /// No native cross-connection cluster identity has been established.
    SessionScoped {
        connection_id: String,
        detected_version: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditAggregation {
    Sum,
    Max,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditAggregateFact {
    pub aggregation: AuditAggregation,
    pub metric: AuditMetricFact,
    /// Source-side nonnegative numeric rows, not a second parser verdict.
    pub source_nonnegative_count: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditMetricCoverage {
    pub metric_id: AuditMetricId,
    pub sql_null_count: String,
    pub text_present_count: String,
    pub nonnegative_count: String,
    pub negative_count: String,
    pub aggregate: AuditAggregateFact,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditCoverage {
    // Counter strings are exact decimal text, never IEEE-754 JSON numbers.
    pub raw_row_count: String,
    pub invalid_query_id_count: String,
    pub exact_duplicate_row_count: String,
    pub conflicting_duplicate_row_count: String,
    pub retained_event_count: String,
    pub retained_family_count: String,
    /// Exact ERR count among the retained canonical events in this scope.
    pub error_count: String,
    /// Exact ERR / retained-event fraction. It is not a rounded percentage.
    pub error_proportion: AuditErrorProportion,
    pub missing_digest_count: String,
    pub missing_context_count: String,
    pub metric_inputs: Vec<AuditMetricCoverage>,
    /// Raw source rows in the same window and field filters that the explicit
    /// statement-class filter hid. Never folded into `raw_row_count`.
    pub statement_filter: AuditStatementFilterCoverage,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditStatementFilterCoverage {
    /// `statement_filter = all`: nothing was hidden, so no hidden count exists.
    NotApplied,
    Applied {
        /// Exact decimal count of rows whose `isQuery` is not 1 (including NULL).
        hidden_non_query_row_count: String,
        /// Exact decimal count of `isQuery = 1` rows carrying this tool's marker.
        hidden_doctor_row_count: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditTopWindow {
    pub requested_limit: u16,
    pub returned_count: u16,
    pub full_window_count: String,
    /// Based on the server's full-window count, not rows.len() == limit.
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOccurrencePresentation {
    pub query: ConnectedQueryRef,
    pub catalog: Option<String>,
    pub metrics: Vec<AuditMetricFact>,
    /// AuditLoader's optional `errorCode` source fact. An empty source string
    /// is deliberately not a SQL NULL or a missing source column.
    #[serde(default)]
    pub error_code: AuditErrorCodeFact,
    pub family: AuditFamilyMembership,
    /// This execution's own audited `stmt`, as the same bounded character
    /// prefix a family sample uses. Old fixtures omit it.
    #[serde(default)]
    #[schemars(default)]
    pub statement_sample: AuditStatementSample,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditErrorCodeFact {
    Unavailable {
        reason: AuditOptionalFactUnavailableReason,
    },
    SqlNull,
    Text {
        value: String,
    },
}

impl Default for AuditErrorCodeFact {
    fn default() -> Self {
        Self::Unavailable {
            reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        }
    }
}

/// Exact numerator/denominator facts for a family's source ERR proportion.
/// Decimal strings deliberately avoid a lossy percentage or JSON number.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditErrorProportion {
    Exact {
        numerator: String,
        denominator: String,
    },
    NotApplicable {
        numerator: String,
        denominator: String,
    },
    Unavailable {
        reason: AuditErrorProportionUnavailableReason,
    },
    Invalid {
        reason: AuditErrorProportionInvalidReason,
    },
}

impl Default for AuditErrorProportion {
    fn default() -> Self {
        Self::Unavailable {
            reason: AuditErrorProportionUnavailableReason::LegacyFieldAbsent,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditErrorProportionUnavailableReason {
    LegacyFieldAbsent,
    ErrorCountMissing,
    ExecutionCountMissing,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditErrorProportionInvalidReason {
    ErrorCountInvalid,
    ExecutionCountInvalid,
    NumeratorExceedsDenominator,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyPresentation {
    pub membership: AuditFamilyMembership,
    pub execution_count: Option<String>,
    pub error_count: Option<String>,
    /// Old retained golden fixtures omit this field. New API responses always
    /// emit a typed fact instead of inferring a rate from missing counters.
    #[serde(default)]
    #[schemars(default)]
    pub error_proportion: AuditErrorProportion,
    pub aggregates: Vec<AuditAggregateFact>,
    /// The audited `stmt` of the family's most recent member, as a bounded
    /// character prefix. It is one real member's source text, never a
    /// normalized or parameterized family SQL. Old fixtures omit it.
    #[serde(default)]
    #[schemars(default)]
    pub statement_sample: AuditStatementSample,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditStatementSample {
    /// `truncated` is true when the source text is longer than the prefix.
    Text {
        value: String,
        truncated: bool,
    },
    SqlNull,
    Unavailable {
        reason: AuditStatementSampleUnavailableReason,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditStatementSampleUnavailableReason {
    /// The probed audit schema has no `stmt` column.
    SourceColumnAbsent,
    /// This response did not read samples (Top chart rows, legacy fixtures).
    NotRead,
    /// Duplicate audit rows of one execution disagree on `stmt`.
    ConflictingSourceRows,
}

impl Default for AuditStatementSample {
    fn default() -> Self {
        Self::Unavailable {
            reason: AuditStatementSampleUnavailableReason::NotRead,
        }
    }
}

/// A bounded rendering coordinate, deliberately separate from the exact
/// decimal fact. `permyriad` is a layout value in [0, 10000], never a metric.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditPlotProjection {
    Exact {
        permyriad: u16,
    },
    NotApplicable {
        reason: AuditPlotNotApplicableReason,
    },
    Unavailable {
        reason: AuditPlotUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditPlotNotApplicableReason {
    ZeroSeriesMaximum,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditPlotUnavailableReason {
    MetricNotPlottable,
}

/// Decimal text endpoints retain exact epoch-millisecond coordinates even
/// when a caller's JavaScript Number would lose precision.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyWindow {
    pub start_milliseconds: String,
    pub end_milliseconds: String,
}

/// A server-labelled x-axis location. The label is presentation text emitted
/// in the source session's observed time zone; clients do not convert it.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyTimeTick {
    pub at_milliseconds: String,
    pub x_permyriad: u16,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyAxis {
    pub time_ticks: Vec<AuditConcurrencyTimeTick>,
}

/// One inclusive start for a constant historical-in-flight segment. The
/// right window endpoint is intentionally never a point. Exact count and
/// time stay text; permyriad coordinates are layout only.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyPoint {
    pub at_milliseconds: String,
    pub active_count: String,
    pub x_permyriad: u16,
    pub y_permyriad: u16,
    pub label: String,
}

/// A server-projected y-axis fact. The frontend never divides exact counts to
/// manufacture intermediate ticks.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyCountTick {
    pub active_count: String,
    pub y_permyriad: u16,
}

/// The bounded source used by both the historical line and an active-at
/// drill. This is deliberately partial: records that began before the fixed
/// lookback are not read and must never be interpreted as zero in-flight work.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditBoundedSourceCoverage {
    Partial {
        reason: AuditBoundedSourcePartialReason,
        /// Inclusive source lower bound B, in the same session-timezone
        /// timestamp domain as the scope bounds.
        source_start_seconds: u32,
        /// Exclusive source upper bound W1. It equals the public scope end.
        source_end_seconds: u32,
        /// Fixed configuration even when `source_start_seconds` saturates at
        /// Unix epoch zero; it is not a claimed observed maximum duration.
        lookback_seconds: u32,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditBoundedSourcePartialReason {
    BoundedLookback,
}

/// Source-side eligibility and exclusion facts for the bounded canonical
/// cohort. These are not inferred from returned points or participant rows.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyCoverage {
    pub canonical_bounded_event_count: String,
    pub interval_count: String,
    /// Active count at the inclusive public window start (W0).
    pub initial_active_count: String,
    /// Exact number of unique in-window transition timestamps, before the
    /// transport point budget is applied.
    pub transition_count: String,
    pub timestamp_missing_count: String,
    pub timestamp_invalid_count: String,
    pub non_terminal_count: String,
    pub query_time_sql_null_count: String,
    pub query_time_negative_count: String,
    pub query_time_overflow_count: String,
    pub zero_duration_count: String,
    pub endpoint_overflow_count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditConcurrencySeries {
    Exact {
        /// Largest exact active count across the returned full series. It is
        /// the y-projection denominator, never a UI-calculated number.
        maximum_active_count: String,
        count_ticks: Vec<AuditConcurrencyCountTick>,
        points: Vec<AuditConcurrencyPoint>,
    },
    Unavailable {
        reason: AuditConcurrencyUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditConcurrencyUnavailableReason {
    /// `SHOW COLUMNS` did not prove the fixed source arithmetic contract.
    SchemaTypeUnsupported,
    /// The exact source transition count exceeds the bounded transport shape;
    /// no partial line is returned.
    TransitionPointBudgetExceeded,
}

/// Historical terminal audit interval projection. It is not a live-running,
/// queueing, or blocking-query measurement.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditConcurrencyPresentation {
    pub window: AuditConcurrencyWindow,
    pub axis: AuditConcurrencyAxis,
    /// The exact bounded source range for this line. It remains present even
    /// when schema capability makes the line unavailable.
    pub source_coverage: AuditBoundedSourceCoverage,
    /// Absent when source schema types are unproven and the bounded source
    /// concurrency read was deliberately not issued. Absence never means 0.
    pub coverage: Option<AuditConcurrencyCoverage>,
    pub series: AuditConcurrencySeries,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOverviewFamilyTopRow {
    pub family: AuditFamilyPresentation,
    pub plot: AuditPlotProjection,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOverviewFamilyTopPresentation {
    pub sort: AuditOverviewFamilyTopSort,
    /// The projection denominator is the largest plottable returned Top row,
    /// not an unreturned family or the selected primary table.
    pub projection_scope: AuditOverviewProjectionScope,
    pub rows: Vec<AuditOverviewFamilyTopRow>,
    pub top: AuditTopWindow,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditOverviewProjectionScope {
    ReturnedTopRows,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditOverviewPrimaryPresentation {
    Occurrences {
        rows: Vec<AuditOccurrencePresentation>,
        top: AuditTopWindow,
    },
    Families {
        rows: Vec<AuditFamilyPresentation>,
        top: AuditTopWindow,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditActiveAtParticipants {
    Exact {
        rows: Vec<AuditActiveAtParticipant>,
    },
    /// The exact active count is retained on the outer response, but only the
    /// deterministic first 500 identities are returned and discoverable.
    Truncated {
        rows: Vec<AuditActiveAtParticipant>,
        omitted_count: String,
    },
}

/// A server-derived historical terminal interval for one active-at identity.
/// These values are not a frontend reconstruction from `queryTime`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditActiveAtParticipant {
    pub occurrence: AuditOccurrencePresentation,
    pub start_milliseconds: String,
    pub end_milliseconds: String,
    pub elapsed_milliseconds: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditActiveAtResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub at_milliseconds: String,
    /// The same fixed bounded source range used for the active-at interval
    /// reconstruction, rather than an implicit all-history read.
    pub source_coverage: AuditBoundedSourceCoverage,
    /// Source-side exact count at this point, independent of the bounded
    /// returned participant list.
    pub active_count: String,
    pub participants: AuditActiveAtParticipants,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOverviewResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub primary: AuditOverviewPrimaryPresentation,
    pub coverage: AuditCoverage,
    pub concurrency: AuditConcurrencyPresentation,
    pub family_top: AuditOverviewFamilyTopPresentation,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditOccurrenceResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub rows: Vec<AuditOccurrencePresentation>,
    pub coverage: AuditCoverage,
    pub top: AuditTopWindow,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: Option<TimeZoneEvidence>,
    pub rows: Vec<AuditFamilyPresentation>,
    pub coverage: AuditCoverage,
    pub top: AuditTopWindow,
    pub limitations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_sample_states_keep_distinct_wire_values() {
        let unavailable = |reason| AuditStatementSample::Unavailable { reason };
        for (sample, wire) in [
            (
                AuditStatementSample::Text {
                    value: String::new(),
                    truncated: false,
                },
                serde_json::json!({ "state": "text", "value": "", "truncated": false }),
            ),
            (
                AuditStatementSample::SqlNull,
                serde_json::json!({ "state": "sql_null" }),
            ),
            (
                unavailable(AuditStatementSampleUnavailableReason::SourceColumnAbsent),
                serde_json::json!({ "state": "unavailable", "reason": "source_column_absent" }),
            ),
            (
                unavailable(AuditStatementSampleUnavailableReason::NotRead),
                serde_json::json!({ "state": "unavailable", "reason": "not_read" }),
            ),
            (
                unavailable(AuditStatementSampleUnavailableReason::ConflictingSourceRows),
                serde_json::json!({ "state": "unavailable", "reason": "conflicting_source_rows" }),
            ),
        ] {
            assert_eq!(serde_json::to_value(&sample).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<AuditStatementSample>(wire).unwrap(),
                sample
            );
        }
        // A legacy occurrence without the field reads as not read, never empty text.
        assert_eq!(
            AuditStatementSample::default(),
            unavailable(AuditStatementSampleUnavailableReason::NotRead)
        );
    }
}
