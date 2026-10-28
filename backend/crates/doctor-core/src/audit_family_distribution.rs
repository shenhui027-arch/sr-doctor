//! Fixed histogram presentation and strict SQL count validation, not a metric parser.
use doctor_contracts::{
    AuditFamilyDistributionAlgorithm, AuditFamilyDistributionExclusions,
    AuditFamilyExecutionDistribution, AuditFamilyHistogramBucket,
};

use crate::MAX_AUDIT_QUERY_TIME_MS;

pub struct AuditFamilyHistogramBin {
    pub id: &'static str,
    pub lower_ms: i64,
    pub upper_ms: i64,
    pub label_seconds: &'static str,
}

pub const AUDIT_FAMILY_HISTOGRAM_BINS: [AuditFamilyHistogramBin; 8] = [
    AuditFamilyHistogramBin {
        id: "lt_10ms",
        lower_ms: 0,
        upper_ms: 10,
        label_seconds: "< 10 ms",
    },
    AuditFamilyHistogramBin {
        id: "10_100ms",
        lower_ms: 10,
        upper_ms: 100,
        label_seconds: "10–100 ms",
    },
    AuditFamilyHistogramBin {
        id: "100_1000ms",
        lower_ms: 100,
        upper_ms: 1000,
        label_seconds: "100 ms–1 s",
    },
    AuditFamilyHistogramBin {
        id: "1_10s",
        lower_ms: 1000,
        upper_ms: 10000,
        label_seconds: "1–10 s",
    },
    AuditFamilyHistogramBin {
        id: "10_60s",
        lower_ms: 10000,
        upper_ms: 60000,
        label_seconds: "10–60 s",
    },
    AuditFamilyHistogramBin {
        id: "1_5m",
        lower_ms: 60000,
        upper_ms: 300000,
        label_seconds: "1–5 min",
    },
    AuditFamilyHistogramBin {
        id: "5_60m",
        lower_ms: 300000,
        upper_ms: 3600000,
        label_seconds: "5–60 min",
    },
    AuditFamilyHistogramBin {
        id: "gte_60m",
        lower_ms: 3600000,
        upper_ms: MAX_AUDIT_QUERY_TIME_MS + 1,
        label_seconds: "≥ 60 min",
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilyDistributionSource {
    pub population_count: String,
    pub eligible_count: String,
    pub sql_null_count: String,
    pub negative_count: String,
    pub overflow_count: String,
    pub bucket_counts: [String; 8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditFamilyDistributionError {
    InvalidCount,
    CountOverflow,
    CoverageMismatch,
}

pub fn project_audit_family_distribution(
    source: &AuditFamilyDistributionSource,
) -> Result<AuditFamilyExecutionDistribution, AuditFamilyDistributionError> {
    let count = |value: &str| {
        if value.is_empty()
            || !value.bytes().all(|b| b.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(AuditFamilyDistributionError::InvalidCount);
        }
        value
            .parse::<u64>()
            .map_err(|_| AuditFamilyDistributionError::CountOverflow)
    };
    let sum = |values: &[String]| -> Result<u64, AuditFamilyDistributionError> {
        values.iter().try_fold(0u64, |acc, value| {
            acc.checked_add(count(value)?)
                .ok_or(AuditFamilyDistributionError::CountOverflow)
        })
    };
    let population = count(&source.population_count)?;
    let eligible = count(&source.eligible_count)?;
    let classified = sum(&[
        source.eligible_count.clone(),
        source.sql_null_count.clone(),
        source.negative_count.clone(),
        source.overflow_count.clone(),
    ])?;
    if population != classified || eligible != sum(&source.bucket_counts)? {
        return Err(AuditFamilyDistributionError::CoverageMismatch);
    }
    Ok(AuditFamilyExecutionDistribution::Exact {
        algorithm: AuditFamilyDistributionAlgorithm::QueryTimeHistogramV1,
        population_count: source.population_count.clone(),
        eligible_count: source.eligible_count.clone(),
        exclusions: AuditFamilyDistributionExclusions {
            sql_null_count: source.sql_null_count.clone(),
            negative_count: source.negative_count.clone(),
            overflow_count: source.overflow_count.clone(),
        },
        buckets: AUDIT_FAMILY_HISTOGRAM_BINS
            .iter()
            .zip(&source.bucket_counts)
            .map(|(bin, count)| AuditFamilyHistogramBucket {
                bucket_id: bin.id.into(),
                lower_ms_inclusive: bin.lower_ms.to_string(),
                upper_ms_exclusive: bin.upper_ms.to_string(),
                display_label_seconds: bin.label_seconds.into(),
                count: count.clone(),
            })
            .collect(),
    })
}
