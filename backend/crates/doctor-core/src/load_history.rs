//! Task 58 (L1): one collected slice of finished loads becomes hourly bucket
//! deltas keyed by database, table and load kind, bucketed by finish time.
//! Counts StarRocks does not know are counted apart, never as zero; phase
//! timings exist only where the record measured them (Stream Load).
use std::collections::BTreeMap;

use crate::history_collection::{HISTORY_BUCKET_SECONDS, HistoryMetric};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LoadHistoryKind {
    StreamLoad,
    Insert,
    RoutineLoad,
    Broker,
    Other,
}

impl LoadHistoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StreamLoad => "stream_load",
            Self::Insert => "insert",
            Self::RoutineLoad => "routine_load",
            Self::Broker => "broker",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "stream_load" => Self::StreamLoad,
            "insert" => Self::Insert,
            "routine_load" => Self::RoutineLoad,
            "broker" => Self::Broker,
            "other" => Self::Other,
            _ => return None,
        })
    }

    /// The FE manager that keeps (and batch-clears) this kind's records:
    /// Stream Load and Routine Load tasks share one, the others another.
    pub fn stream_manager(self) -> bool {
        matches!(self, Self::StreamLoad | Self::RoutineLoad)
    }
}

/// Phase timings in `RUNTIME_DETAILS`, in this order.
pub const LOAD_PHASES: [&str; 3] = ["begin_txn", "plan", "receive_data"];

/// One finished (FINISHED or CANCELLED) load as the collector read it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectedLoad {
    pub job_id: u64,
    pub label: String,
    pub database: String,
    pub table: String,
    pub kind: LoadHistoryKind,
    pub cancelled: bool,
    /// Statistics jobs or the AuditLoader database.
    pub system: bool,
    pub finish_epoch_seconds: i64,
    pub loaded_rows: Option<u128>,
    pub filtered_rows: Option<u128>,
    pub unselected_rows: Option<u128>,
    pub scan_bytes: Option<u128>,
    pub total_seconds: Option<u64>,
    pub phases_ms: [Option<u64>; 3],
    /// `TRACKING_SQL` was set: StarRocks kept an error log for this load.
    pub has_error_log: bool,
}

impl CollectedLoad {
    /// Worth reading the error log for: rows were filtered, or StarRocks
    /// says it kept a log.
    pub fn wants_error_log(&self) -> bool {
        self.has_error_log || self.filtered_rows.is_some_and(|rows| rows > 0)
    }
}

/// An additive count with the number of loads that did not report it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadCount {
    pub sum: u128,
    pub unknown: u64,
}

impl LoadCount {
    fn add(&mut self, value: Option<u128>) {
        match value {
            Some(value) => self.sum += value,
            None => self.unknown += 1,
        }
    }

    pub fn merge(&mut self, other: &LoadCount) {
        self.sum += other.sum;
        self.unknown += other.unknown;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadBucketDelta {
    pub bucket_start: i64,
    pub database: String,
    pub table: String,
    pub kind: Option<LoadHistoryKind>,
    pub system: bool,
    pub loads: u64,
    pub failed: u64,
    pub loaded_rows: LoadCount,
    pub filtered_rows: LoadCount,
    pub unselected_rows: LoadCount,
    pub scan_bytes: LoadCount,
    /// Seconds from creation to finish (both second precision).
    pub total_seconds: HistoryMetric,
    pub phases_ms: [HistoryMetric; 3],
}

impl LoadBucketDelta {
    pub fn merge(&mut self, other: &LoadBucketDelta) {
        self.loads += other.loads;
        self.failed += other.failed;
        self.loaded_rows.merge(&other.loaded_rows);
        self.filtered_rows.merge(&other.filtered_rows);
        self.unselected_rows.merge(&other.unselected_rows);
        self.scan_bytes.merge(&other.scan_bytes);
        self.total_seconds.merge(&other.total_seconds);
        for (phase, other) in self.phases_ms.iter_mut().zip(&other.phases_ms) {
            phase.merge(other);
        }
    }
}

pub fn aggregate_loads(loads: &[CollectedLoad]) -> Vec<LoadBucketDelta> {
    let mut buckets: BTreeMap<(i64, String, String, LoadHistoryKind), LoadBucketDelta> =
        BTreeMap::new();
    for load in loads {
        let bucket_start =
            load.finish_epoch_seconds.div_euclid(HISTORY_BUCKET_SECONDS) * HISTORY_BUCKET_SECONDS;
        let bucket = buckets
            .entry((
                bucket_start,
                load.database.clone(),
                load.table.clone(),
                load.kind,
            ))
            .or_insert_with(|| LoadBucketDelta {
                bucket_start,
                database: load.database.clone(),
                table: load.table.clone(),
                kind: Some(load.kind),
                system: load.system,
                ..LoadBucketDelta::default()
            });
        bucket.loads += 1;
        bucket.failed += u64::from(load.cancelled);
        bucket.loaded_rows.add(load.loaded_rows);
        bucket.filtered_rows.add(load.filtered_rows);
        bucket.unselected_rows.add(load.unselected_rows);
        bucket.scan_bytes.add(load.scan_bytes);
        if let Some(seconds) = load.total_seconds {
            bucket.total_seconds.add(u128::from(seconds));
        }
        for (metric, value) in bucket.phases_ms.iter_mut().zip(load.phases_ms) {
            if let Some(value) = value {
                metric.add(u128::from(value));
            }
        }
    }
    buckets.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(kind: LoadHistoryKind, finish: i64) -> CollectedLoad {
        CollectedLoad {
            job_id: 1,
            label: "l".into(),
            database: "db".into(),
            table: "t".into(),
            kind,
            cancelled: false,
            system: false,
            finish_epoch_seconds: finish,
            loaded_rows: Some(10),
            filtered_rows: Some(0),
            unselected_rows: Some(0),
            scan_bytes: None,
            total_seconds: Some(2),
            phases_ms: [Some(5), Some(7), None],
            has_error_log: false,
        }
    }

    #[test]
    fn loads_bucket_by_finish_hour_and_kind_with_unknowns_apart() {
        let mut failed = load(LoadHistoryKind::StreamLoad, 3_700);
        failed.cancelled = true;
        failed.loaded_rows = None;
        failed.filtered_rows = Some(3);
        let buckets = aggregate_loads(&[
            load(LoadHistoryKind::StreamLoad, 3_600),
            failed,
            load(LoadHistoryKind::Insert, 3_601),
            load(LoadHistoryKind::StreamLoad, 7_200),
        ]);
        assert_eq!(buckets.len(), 3);
        let stream = &buckets[0];
        assert_eq!(
            (stream.bucket_start, stream.kind),
            (3_600, Some(LoadHistoryKind::StreamLoad))
        );
        assert_eq!((stream.loads, stream.failed), (2, 1));
        assert_eq!(
            stream.loaded_rows,
            LoadCount {
                sum: 10,
                unknown: 1
            }
        );
        assert_eq!(stream.filtered_rows.sum, 3);
        assert_eq!(stream.scan_bytes, LoadCount { sum: 0, unknown: 2 });
        assert_eq!(stream.phases_ms[0].valid_count, 2);
        assert_eq!(stream.phases_ms[2].valid_count, 0);
        assert_eq!(buckets[1].kind, Some(LoadHistoryKind::Insert));
        assert_eq!(buckets[2].bucket_start, 7_200);
    }

    #[test]
    fn error_logs_are_wanted_for_filtered_rows_or_a_tracking_sql() {
        let mut plain = load(LoadHistoryKind::StreamLoad, 0);
        assert!(!plain.wants_error_log());
        plain.filtered_rows = Some(1);
        assert!(plain.wants_error_log());
        plain.filtered_rows = None;
        plain.has_error_log = true;
        assert!(plain.wants_error_log());
    }
}
