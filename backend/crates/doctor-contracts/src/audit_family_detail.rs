//! On-demand exact-family facts; neither SQL nor Profile text is carried here.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AuditClusterIdentity, AuditFamilyMembership, AuditOccurrencePresentation, AuditScope,
    AuditWorkloadVersion, TimeZoneEvidence,
};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyDetailReadRequest {
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    /// Opaque key already returned in this connection's current discovery.
    pub family_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
pub enum AuditFamilyDistributionAlgorithm {
    #[serde(rename = "audit-query-time-histogram.v1")]
    #[ts(rename = "audit-query-time-histogram.v1")]
    QueryTimeHistogramV1,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyHistogramBucket {
    pub bucket_id: String,
    pub lower_ms_inclusive: String,
    pub upper_ms_exclusive: String,
    // Historical wire name; human-readable labels use ms/s/min as appropriate.
    // Numeric interval semantics remain in lower_ms_inclusive/upper_ms_exclusive.
    pub display_label_seconds: String,
    pub count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyDistributionExclusions {
    pub sql_null_count: String,
    pub negative_count: String,
    pub overflow_count: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditFamilyExecutionDistribution {
    /// Exact only within the bounded parent-scope canonical family cohort.
    Exact {
        algorithm: AuditFamilyDistributionAlgorithm,
        population_count: String,
        eligible_count: String,
        exclusions: AuditFamilyDistributionExclusions,
        buckets: Vec<AuditFamilyHistogramBucket>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditFamilyRepresentativeUnavailableReason {
    EmptyCohort,
    NoEligibleQueryTime,
    NoMatchInBoundedProfileEvidence,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditProfileEvidence {
    ProfileList,
    SuccessfulFetchInSession,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum AuditFamilyRepresentative {
    Selected {
        occurrence: Box<AuditOccurrencePresentation>,
        profile_evidence: Option<AuditProfileEvidence>,
    },
    Unavailable {
        reason: AuditFamilyRepresentativeUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyRepresentatives {
    pub recent: AuditFamilyRepresentative,
    pub slowest: AuditFamilyRepresentative,
    pub profile_known: AuditFamilyRepresentative,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AuditProfileListCheck {
    Checked,
    CapabilityUnavailable,
    CapabilityForbidden,
    CapabilityUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditProfileEvidenceCoverage {
    pub profile_list_check: AuditProfileListCheck,
    pub profile_list_limit: u16,
    pub profile_list_returned: u16,
    pub profile_list_at_limit: bool,
    pub profile_list_excluded: u16,
    pub successful_cache_retained: u16,
    pub successful_cache_capacity: u16,
    pub successful_cache_evicted: bool,
    pub candidate_count: u16,
    pub candidate_limit: u16,
    pub candidate_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AuditFamilyDetailResponse {
    pub schema_version: AuditWorkloadVersion,
    pub connection_id: String,
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub family: AuditFamilyMembership,
    pub distribution: AuditFamilyExecutionDistribution,
    pub representatives: AuditFamilyRepresentatives,
    pub profile_evidence_coverage: AuditProfileEvidenceCoverage,
    pub cluster_identity: AuditClusterIdentity,
    pub audit_timestamp_time_zone: TimeZoneEvidence,
    pub limitations: Vec<String>,
}
