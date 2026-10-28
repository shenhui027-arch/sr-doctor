//! Stage B: one collected slice of audit executions becomes literal-free
//! template rows and hourly bucket deltas. Official digests key their
//! template directly; digest-missing rows use the A1 product fingerprint;
//! truncated or unreliable statements are counted, never stored as templates.
use std::collections::BTreeMap;

use doctor_contracts::{AuditMetricId, AuditMetricInput, ProductSqlFingerprint};
use sha2::{Digest, Sha256};

use crate::audit_fingerprint_families::nonnegative;
use crate::{
    AuditFingerprintStatement, LogHistogram, SqlFingerprintInput, normalize_audit_metric,
    product_sql_fingerprint,
};

pub const HISTORY_BUCKET_SECONDS: i64 = 3600;
/// Slowest executions kept per template hour (query ID only).
pub const HISTORY_EXEMPLARS_PER_BUCKET: usize = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectedExecution {
    pub query_id: Option<String>,
    pub catalog: Option<String>,
    pub database: Option<String>,
    pub native_digest: Option<String>,
    pub statement: AuditFingerprintStatement,
    pub error: bool,
    pub event_epoch_seconds: Option<i64>,
    pub metrics: Vec<(AuditMetricId, AuditMetricInput)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum HistoryTemplateKind {
    OfficialDigest,
    ProductFingerprint,
}

impl HistoryTemplateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OfficialDigest => "official_digest",
            Self::ProductFingerprint => "product_fingerprint",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTemplate {
    pub template_key: String,
    pub kind: HistoryTemplateKind,
    /// The native digest or the product fingerprint.
    pub identity: String,
    pub catalog: String,
    pub database: String,
    /// Literal-free template; `None` when no reliable statement was read.
    pub template: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HistoryMetric {
    pub valid_count: u64,
    pub sum: u128,
    pub max: Option<u128>,
    pub histogram: LogHistogram,
}

impl HistoryMetric {
    pub(crate) fn add(&mut self, value: u128) {
        self.valid_count += 1;
        self.sum += value;
        self.max = Some(self.max.map_or(value, |max| max.max(value)));
        self.histogram
            .record(u64::try_from(value).unwrap_or(u64::MAX));
    }

    pub fn merge(&mut self, other: &HistoryMetric) {
        self.valid_count += other.valid_count;
        self.sum += other.sum;
        self.max = match (self.max, other.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.histogram.merge(&other.histogram);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryExemplar {
    pub query_id: String,
    /// Audit source units (ms).
    pub query_time: u64,
    pub event_seconds: i64,
}

fn keep_slowest(exemplars: &mut Vec<HistoryExemplar>) {
    exemplars.sort_by(|a, b| {
        b.query_time
            .cmp(&a.query_time)
            .then_with(|| a.query_id.cmp(&b.query_id))
    });
    exemplars.dedup_by(|a, b| a.query_id == b.query_id);
    exemplars.truncate(HISTORY_EXEMPLARS_PER_BUCKET);
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HistoryBucketDelta {
    pub template_key: String,
    pub bucket_start: i64,
    pub executions: u64,
    pub errors: u64,
    pub query_time: HistoryMetric,
    pub scan_bytes: HistoryMetric,
    pub cpu_cost: HistoryMetric,
    pub memory_cost: HistoryMetric,
    pub exemplars: Vec<HistoryExemplar>,
}

impl HistoryBucketDelta {
    pub fn merge(&mut self, other: &HistoryBucketDelta) {
        self.executions += other.executions;
        self.errors += other.errors;
        self.query_time.merge(&other.query_time);
        self.scan_bytes.merge(&other.scan_bytes);
        self.cpu_cost.merge(&other.cpu_cost);
        self.memory_cost.merge(&other.memory_cost);
        self.exemplars.extend(other.exemplars.iter().cloned());
        keep_slowest(&mut self.exemplars);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollectionTally {
    pub read: u64,
    pub attributed: u64,
    /// Statements longer than the read prefix (no digest to key them).
    pub partial: u64,
    pub unavailable: u64,
    pub missing_context: u64,
    pub missing_time: u64,
}

pub fn history_template_key(
    target: &str,
    kind: HistoryTemplateKind,
    identity: &str,
    catalog: &str,
    database: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        "history-template-v1",
        target,
        kind.as_str(),
        identity,
        catalog,
        database,
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("history-template-v1:{:x}", hasher.finalize())
}

pub fn aggregate_collection(
    target: &str,
    rows: &[CollectedExecution],
) -> (
    Vec<HistoryTemplate>,
    Vec<HistoryBucketDelta>,
    CollectionTally,
) {
    let mut tally = CollectionTally::default();
    let mut templates: BTreeMap<String, HistoryTemplate> = BTreeMap::new();
    let mut buckets: BTreeMap<(String, i64), HistoryBucketDelta> = BTreeMap::new();
    for row in rows {
        tally.read += 1;
        let (Some(catalog), Some(database)) = (
            row.catalog.as_deref().filter(|v| !v.is_empty()),
            row.database.as_deref().filter(|v| !v.is_empty()),
        ) else {
            tally.missing_context += 1;
            continue;
        };
        let Some(epoch) = row.event_epoch_seconds else {
            tally.missing_time += 1;
            continue;
        };
        let fingerprint = match &row.statement {
            AuditFingerprintStatement::Text { value, truncated } => {
                product_sql_fingerprint(SqlFingerprintInput {
                    statement: Some(value),
                    truncated: *truncated,
                    catalog: Some(catalog),
                    database: Some(database),
                })
            }
            _ => ProductSqlFingerprint::Unavailable {
                reason: doctor_contracts::ProductSqlFingerprintUnavailableReason::StatementNotRead,
            },
        };
        let digest = row.native_digest.as_deref().filter(|digest| {
            (1..=128).contains(&digest.len()) && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        let (kind, identity, template) = match (digest, fingerprint) {
            (Some(digest), ProductSqlFingerprint::Exact { template, .. }) => (
                HistoryTemplateKind::OfficialDigest,
                digest.to_ascii_lowercase(),
                Some(template),
            ),
            (Some(digest), _) => (
                HistoryTemplateKind::OfficialDigest,
                digest.to_ascii_lowercase(),
                None,
            ),
            (
                None,
                ProductSqlFingerprint::Exact {
                    fingerprint,
                    template,
                },
            ) => (
                HistoryTemplateKind::ProductFingerprint,
                fingerprint,
                Some(template),
            ),
            (None, ProductSqlFingerprint::Partial { .. }) => {
                tally.partial += 1;
                continue;
            }
            (None, ProductSqlFingerprint::Unavailable { .. }) => {
                tally.unavailable += 1;
                continue;
            }
        };
        tally.attributed += 1;
        let key = history_template_key(target, kind, &identity, catalog, database);
        let entry = templates
            .entry(key.clone())
            .or_insert_with(|| HistoryTemplate {
                template_key: key.clone(),
                kind,
                identity,
                catalog: catalog.to_owned(),
                database: database.to_owned(),
                template: None,
            });
        if entry.template.is_none() {
            entry.template = template;
        }
        let bucket_start = epoch.div_euclid(HISTORY_BUCKET_SECONDS) * HISTORY_BUCKET_SECONDS;
        let bucket = buckets
            .entry((key.clone(), bucket_start))
            .or_insert_with(|| HistoryBucketDelta {
                template_key: key,
                bucket_start,
                ..HistoryBucketDelta::default()
            });
        bucket.executions += 1;
        bucket.errors += u64::from(row.error);
        let mut query_time = None;
        for (metric_id, input) in &row.metrics {
            let target = match metric_id {
                AuditMetricId::QueryTime => &mut bucket.query_time,
                AuditMetricId::ScanBytes => &mut bucket.scan_bytes,
                AuditMetricId::CpuCost => &mut bucket.cpu_cost,
                AuditMetricId::MemoryCost => &mut bucket.memory_cost,
                _ => continue,
            };
            if let Some(value) =
                nonnegative(&normalize_audit_metric(metric_id.clone(), input.clone()))
            {
                target.add(value);
                if *metric_id == AuditMetricId::QueryTime {
                    query_time = Some(value);
                }
            }
        }
        // Query IDs are identifiers, never SQL; a malformed one is not kept.
        let id = row.query_id.as_deref().filter(|id| {
            (1..=64).contains(&id.len())
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
        if let (Some(id), Some(time)) = (id, query_time) {
            bucket.exemplars.push(HistoryExemplar {
                query_id: id.to_owned(),
                query_time: u64::try_from(time).unwrap_or(u64::MAX),
                event_seconds: epoch,
            });
            keep_slowest(&mut bucket.exemplars);
        }
    }
    (
        templates.into_values().collect(),
        buckets.into_values().collect(),
        tally,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        digest: Option<&str>,
        sql: &str,
        epoch: i64,
        query_time: Option<&str>,
    ) -> CollectedExecution {
        CollectedExecution {
            query_id: None,
            catalog: Some("default_catalog".into()),
            database: Some("demo".into()),
            native_digest: digest.map(str::to_owned),
            statement: AuditFingerprintStatement::Text {
                value: sql.into(),
                truncated: false,
            },
            error: false,
            event_epoch_seconds: Some(epoch),
            metrics: vec![(
                AuditMetricId::QueryTime,
                AuditMetricInput::from_sql_cell(query_time.map(str::to_owned)),
            )],
        }
    }

    #[test]
    fn rows_become_literal_free_templates_and_hourly_buckets() {
        let rows = vec![
            row(None, "SELECT id FROM t WHERE id = 1", 7_200, Some("10")),
            row(None, "SELECT id FROM t WHERE id = 2", 7_260, Some("0")),
            row(None, "SELECT id FROM t WHERE id = 3", 10_800, None),
            row(
                Some("AB12"),
                "SELECT name FROM t WHERE name = 'secret'",
                7_300,
                Some("5"),
            ),
            row(None, "SELECT 'open", 7_300, Some("5")),
            CollectedExecution {
                database: None,
                ..row(None, "SELECT 1", 7_300, None)
            },
        ];
        let (templates, buckets, tally) = aggregate_collection("127.0.0.1:9030", &rows);
        assert_eq!(
            (
                tally.read,
                tally.attributed,
                tally.unavailable,
                tally.missing_context
            ),
            (6, 4, 1, 1)
        );
        assert_eq!(templates.len(), 2);
        assert!(
            templates
                .iter()
                .all(|t| !t.template.as_deref().unwrap_or("").contains("secret"))
        );
        let official = templates
            .iter()
            .find(|t| t.kind == HistoryTemplateKind::OfficialDigest)
            .unwrap();
        assert_eq!(official.identity, "ab12");
        assert_eq!(buckets.len(), 3);
        let first = buckets
            .iter()
            .find(|b| b.bucket_start == 7_200 && b.executions == 2)
            .unwrap();
        assert_eq!(
            (
                first.query_time.valid_count,
                first.query_time.sum,
                first.query_time.max
            ),
            (2, 10, Some(10))
        );
        // A SQL NULL is not a 0: the 10_800 bucket has an execution but no valid time.
        assert!(buckets.iter().any(|b| b.bucket_start == 10_800
            && b.executions == 1
            && b.query_time.valid_count == 0));
        assert_ne!(
            history_template_key("a", HistoryTemplateKind::OfficialDigest, "x", "c", "d"),
            history_template_key("b", HistoryTemplateKind::OfficialDigest, "x", "c", "d")
        );
    }

    #[test]
    fn audit_v1_07_malformed_audit_row_does_not_abort_the_slice() {
        let (templates, buckets, tally) = aggregate_collection(
            "t",
            &[
                row(None, "SELECT [1)", 7_200, Some("10")),
                row(None, "SELECT id FROM t WHERE id=1", 7_200, Some("20")),
            ],
        );
        assert_eq!((tally.read, tally.unavailable, tally.attributed), (2, 1, 1));
        assert_eq!(templates.len(), 1);
        assert_eq!(buckets[0].executions, 1);
    }

    #[test]
    fn buckets_keep_the_three_slowest_well_formed_query_ids() {
        let rows: Vec<CollectedExecution> = [
            ("q1", "5"),
            ("q2", "50"),
            ("q3", "30"),
            ("bad id", "99"),
            ("q4", "40"),
            ("q5", "x"),
        ]
        .iter()
        .map(|(id, time)| CollectedExecution {
            query_id: Some((*id).into()),
            ..row(None, "SELECT id FROM t WHERE id = 1", 7_200, Some(time))
        })
        .collect();
        let (_, buckets, _) = aggregate_collection("t", &rows);
        let kept: Vec<(&str, u64)> = buckets[0]
            .exemplars
            .iter()
            .map(|e| (e.query_id.as_str(), e.query_time))
            .collect();
        assert_eq!(kept, vec![("q2", 50), ("q4", 40), ("q3", 30)]);
        let mut merged = buckets[0].clone();
        merged.merge(&HistoryBucketDelta {
            exemplars: vec![HistoryExemplar {
                query_id: "q9".into(),
                query_time: 45,
                event_seconds: 7_300,
            }],
            ..HistoryBucketDelta::default()
        });
        assert_eq!(
            merged
                .exemplars
                .iter()
                .map(|e| e.query_time)
                .collect::<Vec<_>>(),
            vec![50, 45, 40]
        );
    }
}
