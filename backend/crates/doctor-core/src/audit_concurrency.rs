//! Exact arithmetic for historical, terminal audit intervals, not live or
//! blocked-query counts. The caller owns canonical identity and source coverage.

use std::collections::BTreeMap;

use doctor_contracts::{AuditMetricId, AuditMetricInput, AuditMetricState};

use crate::normalize_audit_metric;

/// Fixed QueryTime(ms) eligibility for source-side integer arithmetic. The
/// shared metric parser represents canonical ns up to i64::MAX. Boundary tests
/// must keep this SQL-builder guard aligned with that parser, not replace it.
pub const MAX_AUDIT_QUERY_TIME_MS: i64 = i64::MAX / 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditInterval {
    pub start_ms: i128,
    pub end_ms: i128,
}

impl AuditInterval {
    pub fn overlaps(&self, start_ms: i128, end_ms: i128) -> bool {
        self.start_ms < self.end_ms
            && start_ms < end_ms
            && self.start_ms < end_ms
            && self.end_ms > start_ms
    }

    pub fn active_at(&self, at_ms: i128) -> bool {
        self.start_ms <= at_ms && at_ms < self.end_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditIntervalExclusion {
    TimestampMissing,
    TimestampInvalid,
    NotTerminal,
    QueryTime(AuditMetricState),
    ZeroDuration,
    EndpointOverflow,
}

/// The connector supplies an already decoded timestamp. `queryTime` remains
/// a source cell until the one official metric adapter has classified it.
pub fn audit_interval(
    recorded_start_ms: Option<i128>,
    terminal: bool,
    query_time: AuditMetricInput,
) -> Result<AuditInterval, AuditIntervalExclusion> {
    if !terminal {
        return Err(AuditIntervalExclusion::NotTerminal);
    }
    let start_ms = recorded_start_ms.ok_or(AuditIntervalExclusion::TimestampMissing)?;
    if start_ms < 0 {
        return Err(AuditIntervalExclusion::TimestampInvalid);
    }
    let duration = normalize_audit_metric(AuditMetricId::QueryTime, query_time);
    if !matches!(
        duration.state,
        AuditMetricState::Exact | AuditMetricState::PrecisionUnsafe
    ) {
        return Err(AuditIntervalExclusion::QueryTime(duration.state));
    }
    // This is exact arithmetic on the parser's canonical ns result, not a
    // second unit parser. Integer source milliseconds must divide exactly.
    let canonical_ns = duration
        .canonical_number
        .filter(|_| duration.canonical_unit.as_deref() == Some("ns"))
        .and_then(|number| number.value.parse::<i128>().ok())
        .filter(|value| *value >= 0 && value % 1_000_000 == 0)
        .ok_or(AuditIntervalExclusion::QueryTime(AuditMetricState::Invalid))?;
    let duration_ms = canonical_ns / 1_000_000;
    if duration_ms == 0 {
        return Err(AuditIntervalExclusion::ZeroDuration);
    }
    let end_ms = start_ms
        .checked_add(duration_ms)
        .ok_or(AuditIntervalExclusion::EndpointOverflow)?;
    Ok(AuditInterval { start_ms, end_ms })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditConcurrencyStep {
    /// Inclusive start of this constant-count segment. No step is emitted at
    /// the window's exclusive right end: rendering its cap is not a new sample.
    pub at_ms: i128,
    pub active_count: i128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditConcurrencyDelta {
    pub at_ms: i128,
    pub delta: i128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditConcurrencyError {
    InvalidWindow,
    InvalidInterval,
    PointBudgetExceeded,
    InvalidSourceTransitions,
    CountOverflow,
}

/// Project a complete source-side aggregate without transferring or capping
/// the underlying query rows. `initial_count` is active at the inclusive left
/// edge; deltas are grouped by timestamp strictly inside the window. The caller
/// separately verifies the source's exact transition count against returned rows.
pub fn audit_concurrency_from_deltas(
    window_start_ms: i128,
    window_end_ms: i128,
    initial_count: i128,
    deltas: &[AuditConcurrencyDelta],
    max_points: usize,
) -> Result<Vec<AuditConcurrencyStep>, AuditConcurrencyError> {
    if window_start_ms < 0 || window_start_ms >= window_end_ms {
        return Err(AuditConcurrencyError::InvalidWindow);
    }
    if deltas.len() >= max_points {
        return Err(AuditConcurrencyError::PointBudgetExceeded);
    }
    if initial_count < 0 {
        return Err(AuditConcurrencyError::InvalidSourceTransitions);
    }
    let mut steps = vec![AuditConcurrencyStep {
        at_ms: window_start_ms,
        active_count: initial_count,
    }];
    let mut previous = window_start_ms;
    let mut count = initial_count;
    for event in deltas {
        if event.at_ms <= previous || event.at_ms >= window_end_ms {
            return Err(AuditConcurrencyError::InvalidSourceTransitions);
        }
        count = count
            .checked_add(event.delta)
            .ok_or(AuditConcurrencyError::CountOverflow)?;
        if count < 0 {
            return Err(AuditConcurrencyError::InvalidSourceTransitions);
        }
        steps.push(AuditConcurrencyStep {
            at_ms: event.at_ms,
            active_count: count,
        });
        previous = event.at_ms;
    }
    Ok(steps)
}

/// Scan every supplied canonical interval, never a Top-K sample. Callers must
/// suppress this projection if their source read was incomplete or over budget.
/// Same-timestamp exits and entries are applied together under [start, end).
pub fn audit_concurrency_steps(
    window_start_ms: i128,
    window_end_ms: i128,
    intervals: &[AuditInterval],
    max_points: usize,
) -> Result<Vec<AuditConcurrencyStep>, AuditConcurrencyError> {
    if window_start_ms < 0 || window_start_ms >= window_end_ms {
        return Err(AuditConcurrencyError::InvalidWindow);
    }
    if max_points == 0 {
        return Err(AuditConcurrencyError::PointBudgetExceeded);
    }
    // Scope: bounded input and event points, O(n log n); no sampling or
    // smoothing. Larger sources need a separately verified aggregate path.
    let mut deltas = BTreeMap::<i128, i128>::from([(window_start_ms, 0)]);
    for interval in intervals {
        if interval.start_ms < 0 || interval.start_ms >= interval.end_ms {
            return Err(AuditConcurrencyError::InvalidInterval);
        }
        if !interval.overlaps(window_start_ms, window_end_ms) {
            continue;
        }
        *deltas
            .entry(interval.start_ms.max(window_start_ms))
            .or_default() += 1;
        if interval.end_ms < window_end_ms {
            *deltas.entry(interval.end_ms).or_default() -= 1;
        }
        if deltas.len() > max_points {
            return Err(AuditConcurrencyError::PointBudgetExceeded);
        }
    }
    let mut count = 0_i128;
    deltas
        .into_iter()
        .map(|(at_ms, delta)| {
            count += delta;
            if count < 0 {
                return Err(AuditConcurrencyError::InvalidInterval);
            }
            Ok(AuditConcurrencyStep {
                at_ms,
                active_count: count,
            })
        })
        .collect()
}
