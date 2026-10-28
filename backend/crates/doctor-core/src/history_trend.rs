//! Stage C (ADR-5): one template's verdict from its hourly buckets. Product
//! rule, not an official one: the current window's median against the median
//! of baseline hourly medians ± k·MAD, with a relative floor and sample
//! minimums; a latency change needs a scan or CPU signal beside it. Values
//! stay in the audit source units; the API turns them into typed facts.
use std::collections::BTreeMap;

use doctor_contracts::{
    CoverageHourState, EvidenceCompleteness, TrendConfidence, TrendDirection,
    TrendNotComparableReason, TrendSeverity, TrendUnscoredReason, TrendVerdict, WorkloadSignal,
};

use crate::{HISTORY_BUCKET_SECONDS, HistoryBucketDelta, LogHistogram};

/// k for the MAD band; "待 G2 校准" until production samples exist.
pub const TREND_K_MAD: f64 = 3.0;
/// MAD scaled to a normal standard deviation.
const MAD_SCALE: f64 = 1.4826;
pub const TREND_MIN_RELATIVE_PERCENT: u16 = 20;
pub const TREND_MIN_BASELINE_HOURS: u16 = 6;
pub const TREND_MIN_HOUR_SAMPLES: u16 = 3;
pub const TREND_MIN_CURRENT_SAMPLES: u16 = 10;
pub const HISTORY_TREND_RULE_VERSION: &str = "trend.v2";

/// Persisted semantics: identity normalization and the trend evaluator both
/// participate. Old buckets/snapshots are never silently relabeled.
pub fn history_rule_version() -> String {
    format!(
        "{}/{}",
        doctor_contracts::PRODUCT_SQL_FINGERPRINT_VERSION,
        HISTORY_TREND_RULE_VERSION
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrendBucket {
    pub bucket_start: i64,
    pub engine_version: String,
    pub rule_version: String,
    pub delta: HistoryBucketDelta,
}

impl TrendBucket {
    fn key(&self) -> (&str, &str) {
        (&self.engine_version, &self.rule_version)
    }
}

/// `[baseline_start, current_start)` and `[current_start, end)`, hour aligned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrendWindows {
    pub baseline_start: i64,
    pub current_start: i64,
    pub end: i64,
}

impl TrendWindows {
    pub fn ending_at(now: i64, current_hours: u16, baseline_hours: u16) -> Self {
        let end = (now.div_euclid(HISTORY_BUCKET_SECONDS) + 1) * HISTORY_BUCKET_SECONDS;
        let current_start = end - i64::from(current_hours) * HISTORY_BUCKET_SECONDS;
        Self {
            baseline_start: current_start - i64::from(baseline_hours) * HISTORY_BUCKET_SECONDS,
            current_start,
            end,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SideStats {
    pub median: Option<u64>,
    pub mad: Option<u64>,
    pub executions: u64,
    pub valid: u64,
    pub hours: u32,
    pub thin_hours: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeStats {
    pub direction: TrendDirection,
    pub magnitude: u64,
    /// Tenths of a percent; `None` when the baseline is 0.
    pub relative_permille: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateTrend {
    pub verdict: TrendVerdict,
    pub confidence: Option<TrendConfidence>,
    pub evidence: EvidenceCompleteness,
    pub baseline: SideStats,
    pub current: SideStats,
    pub change: Option<ChangeStats>,
    /// Engine and rule version of the current window.
    pub engine_version: String,
    pub rule_version: String,
    pub limitations: Vec<&'static str>,
}

fn median_of(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        values[mid - 1] / 2 + values[mid] / 2 + (values[mid - 1] % 2 + values[mid] % 2) / 2
    })
}

fn histogram_median(histogram: &LogHistogram) -> Option<u64> {
    histogram.quantile(0.5).map(|(_, _, mid)| mid)
}

fn merged(
    buckets: &[&TrendBucket],
    pick: impl Fn(&HistoryBucketDelta) -> &crate::HistoryMetric,
) -> LogHistogram {
    let mut histogram = LogHistogram::default();
    for bucket in buckets {
        histogram.merge(&pick(&bucket.delta).histogram);
    }
    histogram
}

/// The verdict for one template. `buckets` are all of its buckets in the two
/// windows; `coverage` is the collection state of each current-window hour.
/// Returns `None` when the template did not run in the current window.
pub fn evaluate_template_trend(
    buckets: &[TrendBucket],
    coverage: &BTreeMap<i64, CoverageHourState>,
    windows: TrendWindows,
) -> Option<TemplateTrend> {
    let latest = buckets
        .iter()
        .filter(|bucket| (windows.current_start..windows.end).contains(&bucket.bucket_start))
        .max_by_key(|bucket| bucket.bucket_start)?;
    let key = latest.key();
    let mut limitations = vec!["quantiles_are_log_histogram_approximations"];
    let current: Vec<&TrendBucket> = buckets
        .iter()
        .filter(|b| {
            (windows.current_start..windows.end).contains(&b.bucket_start) && b.key() == key
        })
        .collect();
    let mixed_current = buckets
        .iter()
        .any(|b| (windows.current_start..windows.end).contains(&b.bucket_start) && b.key() != key);
    if mixed_current {
        limitations.push("current_window_spans_a_version_change");
    }
    let in_baseline = |b: &&TrendBucket| {
        (windows.baseline_start..windows.current_start).contains(&b.bucket_start)
    };
    let baseline_all: Vec<&TrendBucket> = buckets.iter().filter(in_baseline).collect();
    let baseline: Vec<&TrendBucket> = baseline_all
        .iter()
        .copied()
        .filter(|b| b.key() == key)
        .collect();
    if baseline.len() < baseline_all.len() && !baseline.is_empty() {
        limitations.push("older_version_buckets_excluded_from_baseline");
    }

    let eligible =
        |b: &&TrendBucket| b.delta.query_time.valid_count >= u64::from(TREND_MIN_HOUR_SAMPLES);
    let current_eligible: Vec<_> = current.iter().copied().filter(eligible).collect();
    let baseline_eligible: Vec<_> = baseline.iter().copied().filter(eligible).collect();
    let current_hist = merged(&current_eligible, |d| &d.query_time);
    let current_stats = SideStats {
        median: histogram_median(&current_hist),
        mad: None,
        executions: current.iter().map(|b| b.delta.executions).sum(),
        valid: current_hist.count(),
        hours: current_eligible.len() as u32,
        thin_hours: (current.len() - current_eligible.len()) as u32,
    };
    let mut hourly: Vec<u64> = Vec::new();
    let mut thin = 0u32;
    for bucket in &baseline {
        if bucket.delta.query_time.valid_count < u64::from(TREND_MIN_HOUR_SAMPLES) {
            thin += 1;
        } else if let Some(median) = histogram_median(&bucket.delta.query_time.histogram) {
            hourly.push(median);
        }
    }
    let baseline_median = median_of(&mut hourly.clone());
    let mad = baseline_median.and_then(|center| {
        median_of(
            &mut hourly
                .iter()
                .map(|value| value.abs_diff(center))
                .collect::<Vec<_>>(),
        )
    });
    let baseline_stats = SideStats {
        median: baseline_median,
        mad,
        executions: baseline.iter().map(|b| b.delta.executions).sum(),
        valid: baseline_eligible
            .iter()
            .map(|b| b.delta.query_time.valid_count)
            .sum(),
        hours: hourly.len() as u32,
        thin_hours: thin,
    };
    if baseline_stats.thin_hours > 0 || current_stats.thin_hours > 0 {
        limitations.push("thin_hours_excluded_from_statistics");
    }

    let current_hours =
        (windows.current_start..windows.end).step_by(HISTORY_BUCKET_SECONDS as usize);
    let gaps = current_hours.clone().any(|hour| {
        coverage
            .get(&hour)
            .is_none_or(|state| *state != CoverageHourState::Complete)
    });
    if gaps {
        limitations.push("current_window_not_fully_collected");
    }
    let observed_valid: u64 = current.iter().map(|b| b.delta.query_time.valid_count).sum();
    if observed_valid < current_stats.executions {
        limitations.push("some_executions_lack_a_valid_query_time");
    }
    let mut evidence =
        if gaps || observed_valid < current_stats.executions || current_stats.thin_hours > 0 {
            EvidenceCompleteness::Partial
        } else {
            EvidenceCompleteness::Complete
        };

    let change = match (baseline_stats.median, current_stats.median) {
        (Some(base), Some(now)) => Some(ChangeStats {
            direction: match now.cmp(&base) {
                std::cmp::Ordering::Greater => TrendDirection::Up,
                std::cmp::Ordering::Less => TrendDirection::Down,
                std::cmp::Ordering::Equal => TrendDirection::None,
            },
            magnitude: now.abs_diff(base),
            relative_permille: (base > 0)
                .then(|| ((i128::from(now) - i128::from(base)) * 1000 / i128::from(base)) as i64),
        }),
        _ => None,
    };
    let result = |verdict, confidence, evidence, change, limitations| TemplateTrend {
        verdict,
        confidence,
        evidence,
        baseline: baseline_stats.clone(),
        current: current_stats.clone(),
        change,
        engine_version: key.0.to_owned(),
        rule_version: key.1.to_owned(),
        limitations,
    };

    if mixed_current {
        let reason = if buckets.iter().any(|b| {
            (windows.current_start..windows.end).contains(&b.bucket_start)
                && b.engine_version != key.0
        }) {
            TrendNotComparableReason::EngineVersionChanged
        } else {
            TrendNotComparableReason::RuleVersionChanged
        };
        return Some(result(
            TrendVerdict::NotComparable { reason },
            None,
            EvidenceCompleteness::Unavailable,
            None,
            limitations,
        ));
    }

    if baseline.is_empty() && !baseline_all.is_empty() {
        let reason = if baseline_all.iter().any(|b| b.engine_version != key.0) {
            TrendNotComparableReason::EngineVersionChanged
        } else {
            TrendNotComparableReason::RuleVersionChanged
        };
        limitations.push("baseline_waits_for_samples_under_the_current_version");
        return Some(result(
            TrendVerdict::NotComparable { reason },
            None,
            EvidenceCompleteness::Unavailable,
            None,
            limitations,
        ));
    }
    if baseline_stats.hours < u32::from(TREND_MIN_BASELINE_HOURS) {
        return Some(result(
            TrendVerdict::Unscored {
                reason: TrendUnscoredReason::InsufficientBaseline,
            },
            None,
            evidence,
            change,
            limitations,
        ));
    }
    if current_stats.valid < u64::from(TREND_MIN_CURRENT_SAMPLES) {
        return Some(result(
            TrendVerdict::Unscored {
                reason: TrendUnscoredReason::InsufficientCurrent,
            },
            None,
            evidence,
            change,
            limitations,
        ));
    }
    let (Some(base), Some(now), Some(mad)) = (
        baseline_stats.median,
        current_stats.median,
        baseline_stats.mad,
    ) else {
        return Some(result(
            TrendVerdict::Unscored {
                reason: TrendUnscoredReason::InsufficientBaseline,
            },
            None,
            evidence,
            change,
            limitations,
        ));
    };
    if base == 0 {
        return Some(result(
            TrendVerdict::Unscored {
                reason: TrendUnscoredReason::ZeroBaseline,
            },
            None,
            evidence,
            change,
            limitations,
        ));
    }
    let threshold = (TREND_K_MAD * MAD_SCALE * mad as f64)
        .max(base as f64 * f64::from(TREND_MIN_RELATIVE_PERCENT) / 100.0);
    let deviation = now as f64 - base as f64;
    if deviation.abs() <= threshold {
        return Some(result(
            TrendVerdict::Stable,
            None,
            evidence,
            change,
            limitations,
        ));
    }
    if deviation < 0.0 {
        let confidence = if evidence == EvidenceCompleteness::Partial {
            TrendConfidence::Low
        } else {
            TrendConfidence::High
        };
        return Some(result(
            TrendVerdict::Improvement,
            Some(confidence),
            evidence,
            change,
            limitations,
        ));
    }
    // Latency rose: it counts only beside a workload signal.
    let ratio_of = |pick: fn(&HistoryBucketDelta) -> &crate::HistoryMetric| -> Option<f64> {
        let before = histogram_median(&merged(&baseline_eligible, pick))?;
        let after = histogram_median(&merged(&current_eligible, pick))?;
        Some(if before == 0 {
            if after == 0 { 1.0 } else { f64::INFINITY }
        } else {
            after as f64 / before as f64
        })
    };
    let signals: Vec<f64> = [ratio_of(|d| &d.scan_bytes), ratio_of(|d| &d.cpu_cost)]
        .into_iter()
        .flatten()
        .collect();
    if signals.is_empty() {
        limitations.push("scan_bytes_and_cpu_unavailable");
        evidence = EvidenceCompleteness::Partial;
        return Some(result(
            TrendVerdict::Unscored {
                reason: TrendUnscoredReason::WorkloadSignalMissing,
            },
            None,
            evidence,
            change,
            limitations,
        ));
    }
    let workload = if signals
        .iter()
        .any(|ratio| *ratio >= 1.0 + f64::from(TREND_MIN_RELATIVE_PERCENT) / 100.0)
    {
        WorkloadSignal::Increased
    } else {
        limitations.push("workload_stable_latency_rose_possible_contention");
        WorkloadSignal::Stable
    };
    let ratio = now as f64 / base as f64;
    let severity = if ratio >= 2.0 {
        TrendSeverity::High
    } else if ratio >= 1.5 {
        TrendSeverity::Medium
    } else {
        TrendSeverity::Low
    };
    let confidence = if evidence == EvidenceCompleteness::Partial {
        TrendConfidence::Low
    } else if workload == WorkloadSignal::Increased
        && baseline_stats.hours >= 24
        && current_stats.valid >= 30
    {
        TrendConfidence::High
    } else {
        TrendConfidence::Medium
    };
    Some(result(
        TrendVerdict::Regression { severity, workload },
        Some(confidence),
        evidence,
        change,
        limitations,
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeriesPoint {
    pub hour_start: i64,
    pub coverage: CoverageHourState,
    /// `None` when the hour was not collected.
    pub executions: Option<u64>,
    pub median: Option<u64>,
    pub valid: u64,
    pub thin: bool,
    pub comparable: bool,
}

/// Hourly points `[start, end)`. A collected hour without a bucket is a true
/// 0; a hour that was not collected stays `None`, never interpolated.
pub fn trend_series(
    buckets: &[TrendBucket],
    coverage: &BTreeMap<i64, CoverageHourState>,
    start: i64,
    end: i64,
    engine_version: &str,
    rule_version: &str,
) -> Vec<SeriesPoint> {
    (start..end)
        .step_by(HISTORY_BUCKET_SECONDS as usize)
        .map(|hour| {
            let state = coverage
                .get(&hour)
                .copied()
                .unwrap_or(CoverageHourState::NotCollected);
            let here: Vec<&TrendBucket> =
                buckets.iter().filter(|b| b.bucket_start == hour).collect();
            let comparable_here: Vec<&TrendBucket> = here
                .iter()
                .copied()
                .filter(|b| b.engine_version == engine_version && b.rule_version == rule_version)
                .collect();
            let histogram = merged(&comparable_here, |d| &d.query_time);
            let executions: u64 = here.iter().map(|b| b.delta.executions).sum();
            SeriesPoint {
                hour_start: hour,
                coverage: state,
                executions: if here.is_empty() && state == CoverageHourState::NotCollected {
                    None
                } else {
                    Some(executions)
                },
                median: histogram_median(&histogram),
                valid: histogram.count(),
                thin: !here.is_empty() && histogram.count() < u64::from(TREND_MIN_HOUR_SAMPLES),
                comparable: here.is_empty() || !comparable_here.is_empty(),
            }
        })
        .collect()
}

/// One template's window, merged: the "before" a governance item pins and
/// the "after" it is verified against. Values in audit source units.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowStats {
    pub executions: u64,
    pub valid: u64,
    pub query_time_median: Option<u64>,
    pub scan_bytes_median: Option<u64>,
    pub cpu_cost_median: Option<u64>,
    pub engine_version: String,
    pub rule_version: String,
}

/// Buckets in `[start, end)` under one version key; `None` picks the newest
/// bucket's key. Returns `None` when no bucket matches.
pub fn window_stats(
    buckets: &[TrendBucket],
    start: i64,
    end: i64,
    key: Option<(&str, &str)>,
) -> Option<WindowStats> {
    let inside: Vec<&TrendBucket> = buckets
        .iter()
        .filter(|b| (start..end).contains(&b.bucket_start))
        .collect();
    let key = match key {
        Some(key) => key,
        None => inside.iter().max_by_key(|b| b.bucket_start)?.key(),
    };
    let chosen: Vec<&TrendBucket> = inside.into_iter().filter(|b| b.key() == key).collect();
    if chosen.is_empty() {
        return None;
    }
    let query_time = merged(&chosen, |d| &d.query_time);
    Some(WindowStats {
        executions: chosen.iter().map(|b| b.delta.executions).sum(),
        valid: query_time.count(),
        query_time_median: histogram_median(&query_time),
        scan_bytes_median: histogram_median(&merged(&chosen, |d| &d.scan_bytes)),
        cpu_cost_median: histogram_median(&merged(&chosen, |d| &d.cpu_cost)),
        engine_version: key.0.to_owned(),
        rule_version: key.1.to_owned(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeVerdict {
    Improved,
    NotImproved,
    InsufficientSamples,
    WorkloadSignalMissing,
    NotComparable(TrendNotComparableReason),
}

/// Before/after verification (ADR-7, product rule; deviates from the Profile
/// comparator because the history keeps aggregates, not Profiles). Improved
/// only when the median query time fell by the relative floor, both sides
/// have enough samples, and no available workload signal (scan bytes, CPU)
/// grew by the floor; with neither signal there is no verdict.
pub fn verify_change(
    before: &WindowStats,
    after: Option<&WindowStats>,
    after_versions: &[(String, String)],
) -> ChangeVerdict {
    // Inspect the whole after window before samples or improvement. A
    // same-version subset cannot authorize closing a mixed-version change.
    if after_versions
        .iter()
        .any(|(engine, _)| *engine != before.engine_version)
        || after.is_some_and(|a| a.engine_version != before.engine_version)
    {
        return ChangeVerdict::NotComparable(TrendNotComparableReason::EngineVersionChanged);
    }
    if after_versions
        .iter()
        .any(|(_, rule)| *rule != before.rule_version)
        || after.is_some_and(|a| a.rule_version != before.rule_version)
    {
        return ChangeVerdict::NotComparable(TrendNotComparableReason::RuleVersionChanged);
    }
    let Some(after) = after else {
        return ChangeVerdict::InsufficientSamples;
    };
    if after.valid < u64::from(TREND_MIN_CURRENT_SAMPLES)
        || before.valid < u64::from(TREND_MIN_CURRENT_SAMPLES)
    {
        return ChangeVerdict::InsufficientSamples;
    }
    let (Some(was), Some(now)) = (before.query_time_median, after.query_time_median) else {
        return ChangeVerdict::InsufficientSamples;
    };
    let floor = 1.0 + f64::from(TREND_MIN_RELATIVE_PERCENT) / 100.0;
    let workload: Vec<bool> = [
        (before.scan_bytes_median, after.scan_bytes_median),
        (before.cpu_cost_median, after.cpu_cost_median),
    ]
    .into_iter()
    .filter_map(|pair| match pair {
        (Some(b), Some(a)) => Some(b == 0 && a == 0 || b > 0 && (a as f64) < b as f64 * floor),
        _ => None,
    })
    .collect();
    if workload.is_empty() {
        return ChangeVerdict::WorkloadSignalMissing;
    }
    let faster = was > 0
        && (now as f64) <= was as f64 * (1.0 - f64::from(TREND_MIN_RELATIVE_PERCENT) / 100.0);
    if faster && workload.iter().all(|stable| *stable) {
        ChangeVerdict::Improved
    } else {
        ChangeVerdict::NotImproved
    }
}

#[cfg(test)]
#[path = "history_trend_tests.rs"]
mod tests;
