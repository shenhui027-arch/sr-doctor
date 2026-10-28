//! In-process grouping of digest-missing audit executions by product
//! fingerprint. Aggregates keep the exact-family SQL semantics: a metric value
//! counts only when the core parser gives it a nonnegative integer, and a
//! group with no such value has no aggregate, never 0.
use std::cmp::Ordering;
use std::collections::BTreeMap;

use doctor_contracts::{
    AuditFamilySort, AuditMetricFact, AuditMetricId, AuditMetricInput, AuditMetricState,
    ProductSqlFingerprint, ProductSqlFingerprintUnavailableReason,
};

use crate::{SqlFingerprintInput, normalize_audit_metric, product_sql_fingerprint};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditFingerprintStatement {
    Text {
        value: String,
        truncated: bool,
    },
    SqlNull,
    /// Duplicate audit rows of one execution disagree on `stmt`.
    Conflicting,
    /// The statement read returned no row for this execution.
    NotRead,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFingerprintEvent {
    pub catalog: String,
    pub database: String,
    pub statement: AuditFingerprintStatement,
    pub error: bool,
    pub metrics: Vec<(AuditMetricId, AuditMetricInput)>,
}

/// One group's aggregate cells as exact decimal text, in the same shape the
/// exact-family SQL returns, so both are projected by the same code.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditFingerprintGroup {
    pub fingerprint: String,
    pub template: String,
    pub catalog: String,
    pub database: String,
    pub execution_count: u64,
    pub error_count: u64,
    pub query_time: MetricCells,
    pub scan_bytes: MetricCells,
    pub cpu_cost: MetricCells,
    pub memory_cost: MetricCells,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetricCells {
    pub valid_count: u64,
    pub sum: Option<u128>,
    pub max: Option<u128>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditFingerprintTally {
    pub fingerprinted: u64,
    pub partial: u64,
    pub unavailable: BTreeMap<ProductSqlFingerprintUnavailableReason, u64>,
}

/// The parser decides validity; the value stays in the source column's unit,
/// as the exact-family SQL sums it, and is projected by the parser again.
pub(crate) fn nonnegative(fact: &AuditMetricFact) -> Option<u128> {
    if !matches!(
        fact.state,
        AuditMetricState::Exact | AuditMetricState::PrecisionUnsafe
    ) {
        return None;
    }
    match &fact.input {
        AuditMetricInput::Text { value } => value.parse().ok(),
        _ => None,
    }
}

impl MetricCells {
    fn add(&mut self, metric_id: AuditMetricId, input: &AuditMetricInput) {
        let Some(value) = nonnegative(&normalize_audit_metric(metric_id, input.clone())) else {
            return;
        };
        self.valid_count += 1;
        self.sum = Some(self.sum.unwrap_or(0) + value);
        self.max = Some(self.max.map_or(value, |max| max.max(value)));
    }
}

pub fn group_audit_fingerprints(
    events: &[AuditFingerprintEvent],
) -> (Vec<AuditFingerprintGroup>, AuditFingerprintTally) {
    let mut tally = AuditFingerprintTally::default();
    let mut groups: BTreeMap<(String, String, String), AuditFingerprintGroup> = BTreeMap::new();
    for event in events {
        let fingerprint = match &event.statement {
            AuditFingerprintStatement::Conflicting => ProductSqlFingerprint::Unavailable {
                reason: ProductSqlFingerprintUnavailableReason::ConflictingSourceRows,
            },
            AuditFingerprintStatement::NotRead => ProductSqlFingerprint::Unavailable {
                reason: ProductSqlFingerprintUnavailableReason::StatementNotRead,
            },
            statement => product_sql_fingerprint(SqlFingerprintInput {
                statement: match statement {
                    AuditFingerprintStatement::Text { value, .. } => Some(value),
                    _ => None,
                },
                truncated: matches!(
                    statement,
                    AuditFingerprintStatement::Text {
                        truncated: true,
                        ..
                    }
                ),
                catalog: Some(&event.catalog),
                database: Some(&event.database),
            }),
        };
        let (fingerprint, template) = match fingerprint {
            ProductSqlFingerprint::Exact {
                fingerprint,
                template,
            } => (fingerprint, template),
            ProductSqlFingerprint::Partial { .. } => {
                tally.partial += 1;
                continue;
            }
            ProductSqlFingerprint::Unavailable { reason } => {
                *tally.unavailable.entry(reason).or_default() += 1;
                continue;
            }
        };
        tally.fingerprinted += 1;
        let group = groups
            .entry((
                event.catalog.clone(),
                event.database.clone(),
                fingerprint.clone(),
            ))
            .or_insert_with(|| AuditFingerprintGroup {
                fingerprint,
                template,
                catalog: event.catalog.clone(),
                database: event.database.clone(),
                ..AuditFingerprintGroup::default()
            });
        group.execution_count += 1;
        group.error_count += u64::from(event.error);
        for (metric_id, input) in &event.metrics {
            let cells = match metric_id {
                AuditMetricId::QueryTime => &mut group.query_time,
                AuditMetricId::ScanBytes => &mut group.scan_bytes,
                AuditMetricId::CpuCost => &mut group.cpu_cost,
                AuditMetricId::MemoryCost => &mut group.memory_cost,
                _ => continue,
            };
            cells.add(metric_id.clone(), input);
        }
    }
    (groups.into_values().collect(), tally)
}

/// The exact-family SQL order: present values first, descending, then the
/// identity ascending. Error proportion compares exact fractions.
pub fn sort_audit_fingerprint_groups(groups: &mut [AuditFingerprintGroup], sort: &AuditFamilySort) {
    groups.sort_by(|left, right| {
        compare_family_cells(sort, &left.cells(), &right.cells())
            .then_with(|| left.catalog.cmp(&right.catalog))
            .then_with(|| left.database.cmp(&right.database))
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
    });
}

/// The counters and metric cells a family-like row is ranked by.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FamilyCells {
    pub execution_count: u64,
    pub error_count: u64,
    pub query_time: MetricCells,
    pub scan_bytes: MetricCells,
    pub cpu_cost: MetricCells,
    pub memory_cost: MetricCells,
}

impl FamilyCells {
    pub fn add(&mut self, other: &FamilyCells) {
        self.execution_count += other.execution_count;
        self.error_count += other.error_count;
        for (mine, theirs) in [
            (&mut self.query_time, &other.query_time),
            (&mut self.scan_bytes, &other.scan_bytes),
            (&mut self.cpu_cost, &other.cpu_cost),
            (&mut self.memory_cost, &other.memory_cost),
        ] {
            mine.valid_count += theirs.valid_count;
            mine.sum = match (mine.sum, theirs.sum) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
            mine.max = mine.max.max(theirs.max);
        }
    }
}

impl AuditFingerprintGroup {
    pub fn cells(&self) -> FamilyCells {
        FamilyCells {
            execution_count: self.execution_count,
            error_count: self.error_count,
            query_time: self.query_time,
            scan_bytes: self.scan_bytes,
            cpu_cost: self.cpu_cost,
            memory_cost: self.memory_cost,
        }
    }
}

/// The exact-family SQL order on one dimension: present values first,
/// descending; error proportion compares exact fractions. Ties are the
/// caller's identity order.
pub fn compare_family_cells(
    sort: &AuditFamilySort,
    left: &FamilyCells,
    right: &FamilyCells,
) -> Ordering {
    let descending = |left: Option<u128>, right: Option<u128>| match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (left, right) => right.is_some().cmp(&left.is_some()),
    };
    {
        match sort {
            AuditFamilySort::CumulativeQueryTimeDesc => {
                descending(left.query_time.sum, right.query_time.sum)
            }
            AuditFamilySort::CumulativeScanBytesDesc => {
                descending(left.scan_bytes.sum, right.scan_bytes.sum)
            }
            AuditFamilySort::CumulativeCpuCostDesc => {
                descending(left.cpu_cost.sum, right.cpu_cost.sum)
            }
            AuditFamilySort::ExecutionCountDesc => right.execution_count.cmp(&left.execution_count),
            AuditFamilySort::ErrorCountDesc => right.error_count.cmp(&left.error_count),
            AuditFamilySort::ErrorProportionDesc => (u128::from(right.error_count)
                * u128::from(left.execution_count))
            .cmp(&(u128::from(left.error_count) * u128::from(right.execution_count))),
            AuditFamilySort::MaxQueryTimeDesc => {
                descending(left.query_time.max, right.query_time.max)
            }
            AuditFamilySort::MaxMemoryCostDesc => {
                descending(left.memory_cost.max, right.memory_cost.max)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sql: &str, error: bool, query_time: Option<&str>) -> AuditFingerprintEvent {
        AuditFingerprintEvent {
            catalog: "default_catalog".into(),
            database: "db".into(),
            statement: AuditFingerprintStatement::Text {
                value: sql.into(),
                truncated: false,
            },
            error,
            metrics: vec![(
                AuditMetricId::QueryTime,
                AuditMetricInput::from_sql_cell(query_time.map(str::to_owned)),
            )],
        }
    }

    #[test]
    fn groups_literal_variants_and_keeps_unreliable_rows_out_of_groups_and_ranking() {
        let mut events = vec![
            event("SELECT id FROM t WHERE id = 1", false, Some("10")),
            event("select id from db.t where id = 22", true, Some("0")),
            event("SELECT id FROM t WHERE id = 3", false, None),
            event("SELECT name FROM t", false, Some("-1")),
            event("SHOW TABLES", false, Some("5")),
        ];
        events.push(AuditFingerprintEvent {
            statement: AuditFingerprintStatement::Text {
                value: "SELECT id FROM t WHERE id = 4".into(),
                truncated: true,
            },
            ..events[0].clone()
        });
        events.push(AuditFingerprintEvent {
            statement: AuditFingerprintStatement::SqlNull,
            ..events[0].clone()
        });
        events.push(AuditFingerprintEvent {
            statement: AuditFingerprintStatement::Conflicting,
            ..events[0].clone()
        });
        let (mut groups, tally) = group_audit_fingerprints(&events);
        assert_eq!(tally.fingerprinted, 4);
        assert_eq!(tally.partial, 1);
        assert_eq!(
            tally.unavailable,
            BTreeMap::from([
                (ProductSqlFingerprintUnavailableReason::StatementSqlNull, 1),
                (
                    ProductSqlFingerprintUnavailableReason::ConflictingSourceRows,
                    1
                ),
                (
                    ProductSqlFingerprintUnavailableReason::UnsupportedStatement,
                    1
                ),
            ])
        );
        sort_audit_fingerprint_groups(&mut groups, &AuditFamilySort::CumulativeQueryTimeDesc);
        let by_id = &groups[0];
        assert_eq!(by_id.template, "SELECT id FROM t WHERE id = ?");
        assert_eq!((by_id.execution_count, by_id.error_count), (3, 1));
        // A SQL NULL is not 0 and a real 0 is counted.
        assert_eq!(
            by_id.query_time,
            MetricCells {
                valid_count: 2,
                sum: Some(10),
                max: Some(10)
            }
        );
        // A negative sentinel leaves the group with no aggregate, ranked last.
        assert_eq!(groups[1].template, "SELECT name FROM t");
        assert_eq!(groups[1].query_time, MetricCells::default());
    }

    #[test]
    fn error_proportion_ranks_by_exact_fraction() {
        let events = [
            event("SELECT a FROM t", true, Some("1")),
            event("SELECT a FROM t", false, Some("1")),
            event("SELECT a FROM t", false, Some("1")),
            event("SELECT b FROM t", true, Some("1")),
            event("SELECT b FROM t", false, Some("1")),
        ];
        let (mut groups, _) = group_audit_fingerprints(&events);
        sort_audit_fingerprint_groups(&mut groups, &AuditFamilySort::ErrorProportionDesc);
        assert_eq!(groups[0].template, "SELECT b FROM t");
        sort_audit_fingerprint_groups(&mut groups, &AuditFamilySort::ExecutionCountDesc);
        assert_eq!(groups[0].template, "SELECT a FROM t");
    }
}
