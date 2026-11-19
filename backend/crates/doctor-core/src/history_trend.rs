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
use starrocks_version_pack_3_5_14::{
    PACK_ID, PRODUCT_PACK_VERSION, SEMANTIC_MANIFEST_HASH, UPSTREAM_RELEASE,
    deltas::{DELTAS_HASH, ReleaseResolution, resolve_release},
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

    pub fn storage_rule_version(engine_version: &str, rule_version: &str) -> String {
        // Official builds can report the release followed by a short commit.
        let engine_version = engine_version.trim();
        let release = engine_version
            .rsplit_once('-')
            .filter(|(_, suffix)| {
                suffix.len() == 7 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .map_or(engine_version, |(release, _)| release);
        if matches!(resolve_release(release), ReleaseResolution::Audited(_)) {
            format!(
                "{rule_version}|vp={PACK_ID}@{PRODUCT_PACK_VERSION}:{SEMANTIC_MANIFEST_HASH}:{DELTAS_HASH}"
            )
        } else {
            format!("{rule_version}|vp=unknown")
        }
    }

    pub fn active_storage_rule_version() -> String {
        Self::storage_rule_version(UPSTREAM_RELEASE, &history_rule_version())
    }

    pub fn version_pack_in(rule_version: &str) -> Option<&str> {
        let (label, pack) = rule_version.split_once("|vp=")?;
        let (id, rest) = pack.split_once('@')?;
        let (version, hashes) = rest.split_once(':')?;
        let (manifest_hash, deltas_hash) = hashes.split_once(':')?;
        let label_valid = !label.is_empty()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte));
        let id_valid = matches!(id.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9'))
            && id
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-'));
        let (major, rest) = version.split_once('.')?;
        let (minor, patch) = rest.split_once('.')?;
        let version_valid = [major, minor, patch]
            .into_iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
        let hash_valid = |part: &str| {
            part.len() == 64
                && part
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        };
        (label_valid
            && id_valid
            && version_valid
            && hash_valid(manifest_hash)
            && hash_valid(deltas_hash))
        .then_some(pack)
    }

    pub fn rule_version_label(rule_version: &str) -> &str {
        rule_version
            .split_once("|vp=")
            .map_or(rule_version, |(label, _)| label)
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

impl TemplateTrend {
    pub fn without_current_samples(
        buckets: &[TrendBucket],
        coverage: &BTreeMap<i64, CoverageHourState>,
        windows: TrendWindows,
        reference: Option<&TrendBucket>,
    ) -> Self {
        let before = windows.baseline_start..windows.current_start;
        let baseline: Vec<_> = buckets
            .iter()
            .filter(|bucket| {
                before.contains(&bucket.bucket_start)
                    && reference.is_some_and(|reference| {
                        TrendBucket::version_pack_in(&reference.rule_version).is_some()
                            && bucket.key() == reference.key()
                    })
            })
            .collect();
        let baseline_gap =
            window_has_coverage_gap(coverage, windows.baseline_start, windows.current_start);
        let current_gap = window_has_coverage_gap(coverage, windows.current_start, windows.end);
        let mut limitations = vec!["quantiles_are_log_histogram_approximations"];
        if baseline_gap {
            limitations.push("baseline_window_not_fully_collected");
        }
        if current_gap {
            limitations.push("current_window_not_fully_collected");
        }
        if reference
            .is_some_and(|bucket| TrendBucket::version_pack_in(&bucket.rule_version).is_none())
        {
            limitations.push("version_pack_semantics_unknown");
        }
        let evidence = if !baseline_gap && !current_gap {
            EvidenceCompleteness::Complete
        } else if buckets
            .iter()
            .any(|bucket| before.contains(&bucket.bucket_start))
            || coverage
                .range(windows.baseline_start..windows.end)
                .any(|(_, state)| *state != CoverageHourState::NotCollected)
        {
            EvidenceCompleteness::Partial
        } else {
            EvidenceCompleteness::Unavailable
        };
        let baseline = baseline_stats(&baseline);
        if baseline.thin_hours > 0 {
            limitations.push("thin_hours_excluded_from_statistics");
        }
        Self {
            verdict: TrendVerdict::Unscored {
                reason: TrendUnscoredReason::InsufficientCurrent,
            },
            confidence: None,
            evidence,
            baseline,
            current: SideStats::default(),
            change: None,
            engine_version: reference
                .map_or_else(String::new, |bucket| bucket.engine_version.clone()),
            rule_version: reference.map_or_else(String::new, |bucket| bucket.rule_version.clone()),
            limitations,
        }
    }
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

fn baseline_stats(buckets: &[&TrendBucket]) -> SideStats {
    let mut hourly = Vec::new();
    let mut thin = 0u32;
    let mut valid = 0;
    for bucket in buckets {
        let query_time = &bucket.delta.query_time;
        if query_time.valid_count < u64::from(TREND_MIN_HOUR_SAMPLES) {
            thin += 1;
        } else {
            valid += query_time.valid_count;
            if let Some(median) = histogram_median(&query_time.histogram) {
                hourly.push(median);
            }
        }
    }
    let median = median_of(&mut hourly);
    let mad = median.and_then(|center| {
        let mut deviations: Vec<_> = hourly.iter().map(|value| value.abs_diff(center)).collect();
        median_of(&mut deviations)
    });
    SideStats {
        median,
        mad,
        executions: buckets.iter().map(|b| b.delta.executions).sum(),
        valid,
        hours: hourly.len() as u32,
        thin_hours: thin,
    }
}

fn window_has_coverage_gap(
    coverage: &BTreeMap<i64, CoverageHourState>,
    start: i64,
    end: i64,
) -> bool {
    (start..end)
        .step_by(HISTORY_BUCKET_SECONDS as usize)
        .any(|hour| coverage.get(&hour) != Some(&CoverageHourState::Complete))
}

fn workload_ratio(
    baseline: &[&TrendBucket],
    current: &[&TrendBucket],
    metric: fn(&HistoryBucketDelta) -> &crate::HistoryMetric,
) -> Option<f64> {
    let before = histogram_median(&merged(baseline, metric))?;
    let after = histogram_median(&merged(current, metric))?;
    Some(if before == 0 {
        if after == 0 { 1.0 } else { f64::INFINITY }
    } else {
        after as f64 / before as f64
    })
}

/// The verdict for one template. `buckets` are all of its buckets in the two
/// windows; `coverage` is the collection state of each hour in both windows.
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
    let baseline_stats = baseline_stats(&baseline);
    if baseline_stats.thin_hours > 0 || current_stats.thin_hours > 0 {
        limitations.push("thin_hours_excluded_from_statistics");
    }

    let baseline_gap =
        window_has_coverage_gap(coverage, windows.baseline_start, windows.current_start);
    let current_gap = window_has_coverage_gap(coverage, windows.current_start, windows.end);
    if baseline_gap {
        limitations.push("baseline_window_not_fully_collected");
    }
    if current_gap {
        limitations.push("current_window_not_fully_collected");
    }
    let observed_valid: u64 = current.iter().map(|b| b.delta.query_time.valid_count).sum();
    if observed_valid < current_stats.executions {
        limitations.push("some_executions_lack_a_valid_query_time");
    }
    let evidence = if baseline_gap
        || current_gap
        || observed_valid < current_stats.executions
        || current_stats.thin_hours > 0
    {
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

    if mixed_current || baseline.len() < baseline_all.len() {
        let window = if mixed_current {
            windows.current_start..windows.end
        } else {
            windows.baseline_start..windows.current_start
        };
        let mut incompatible = buckets
            .iter()
            .filter(|bucket| window.contains(&bucket.bucket_start) && bucket.key() != key);
        let pack = TrendBucket::version_pack_in(key.1);
        if pack.is_none()
            || incompatible
                .clone()
                .any(|b| TrendBucket::version_pack_in(&b.rule_version).is_none())
        {
            limitations.push("version_pack_semantics_unknown");
        }
        if let Some(pack) = pack
            && incompatible
                .clone()
                .filter_map(|b| TrendBucket::version_pack_in(&b.rule_version))
                .any(|other| other != pack)
        {
            limitations.push("version_pack_changed");
        }
        if mixed_current || baseline.is_empty() {
            let reason = if incompatible.any(|b| b.engine_version != key.0) {
                TrendNotComparableReason::EngineVersionChanged
            } else {
                TrendNotComparableReason::RuleVersionChanged
            };
            if !mixed_current {
                limitations.push("baseline_waits_for_samples_under_the_current_version");
            }
            return Some(result(
                TrendVerdict::NotComparable { reason },
                None,
                EvidenceCompleteness::Unavailable,
                None,
                limitations,
            ));
        }
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
    let signals: Vec<f64> = [
        workload_ratio(&baseline_eligible, &current_eligible, |d| &d.scan_bytes),
        workload_ratio(&baseline_eligible, &current_eligible, |d| &d.cpu_cost),
    ]
    .into_iter()
    .flatten()
    .collect();
    if signals.is_empty() {
        limitations.push("scan_bytes_and_cpu_unavailable");
        return Some(result(
            TrendVerdict::Unscored {
                reason: TrendUnscoredReason::WorkloadSignalMissing,
            },
            None,
            EvidenceCompleteness::Partial,
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
    /// `None` when zero executions could not be established.
    pub executions: Option<u64>,
    pub median: Option<u64>,
    pub valid: u64,
    pub thin: bool,
    pub comparable: bool,
}

/// Hourly points `[start, end)`. Only complete collection can establish a
/// true zero. Positive execution counts reflect the observed samples.
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
            let comparable = here
                .iter()
                .all(|b| b.engine_version == engine_version && b.rule_version == rule_version);
            let histogram = if comparable {
                merged(&here, |d| &d.query_time)
            } else {
                LogHistogram::default()
            };
            let executions: u64 = here.iter().map(|b| b.delta.executions).sum();
            SeriesPoint {
                hour_start: hour,
                coverage: state,
                executions: (executions > 0 || state == CoverageHourState::Complete)
                    .then_some(executions),
                median: histogram_median(&histogram),
                valid: histogram.count(),
                thin: !here.is_empty() && histogram.count() < u64::from(TREND_MIN_HOUR_SAMPLES),
                comparable,
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

#[cfg(test)]
mod trend_correctness_tests {
    use super::*;

    #[test]
    fn storage_rule_key_has_all_version_pack_components() {
        let key = TrendBucket::storage_rule_version("3.5.14", "example-rule");
        let (label, identity) = key.split_once("|vp=").unwrap();
        let (id, version_and_hashes) = identity.split_once('@').unwrap();
        let components: Vec<_> = version_and_hashes.split(':').collect();
        assert_eq!(label, "example-rule");
        assert_eq!(id, PACK_ID);
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], PRODUCT_PACK_VERSION);
        assert_eq!(components[1], SEMANTIC_MANIFEST_HASH);
        assert_eq!(components[2], DELTAS_HASH);
        assert_eq!(TrendBucket::version_pack_in(&key), Some(identity));
    }

    #[test]
    fn storage_rule_key_rejects_missing_or_malformed_pack_components() {
        let hash = "a".repeat(64);
        let short = &hash[..63];
        let non_hex = "g".repeat(64);
        let uppercase = hash.to_ascii_uppercase();
        for key in [
            "rule".into(),
            "rule|vp=unknown".into(),
            "rule|vp=garbage".into(),
            format!("|vp=pack@1.2.3:{hash}:{hash}"),
            format!("   |vp=pack@1.2.3:{hash}:{hash}"),
            format!("rule label|vp=pack@1.2.3:{hash}:{hash}"),
            format!("rule|vp=pack@1.2.3:{hash}"),
            format!("rule|vp=pack@:{hash}:{hash}"),
            format!("rule|vp=starrocks-3.5.14@not-a-version:{hash}:{hash}"),
            format!("rule|vp=pack@1..3:{hash}:{hash}"),
            format!("rule|vp=pack@1.2.3.4:{hash}:{hash}"),
            format!("rule|vp=@1.2.3:{hash}:{hash}"),
            format!("rule|vp=-pack@1.2.3:{hash}:{hash}"),
            format!("rule|vp=Pack@1.2.3:{hash}:{hash}"),
            format!("rule|vp=the_pack@1.2.3:{hash}:{hash}"),
            format!("rule|vp=pack@1.2.3:{short}:{hash}"),
            format!("rule|vp=pack@1.2.3:{hash}:{non_hex}"),
            format!("rule|vp=pack@1.2.3:{uppercase}:{hash}"),
            format!("rule|vp=pack@1.2.3:{hash}:{uppercase}"),
            format!("rule|vp=pack@1.2.3:{hash}:{hash}:extra"),
        ] {
            assert_eq!(TrendBucket::version_pack_in(&key), None, "{key}");
        }
    }

    #[test]
    fn storage_rule_key_accepts_slash_label_and_historical_pack_version() {
        let pack = format!("starrocks-3.5.14@0.1.10:{:064x}:{:064x}", 10, 15);
        let key = format!("SQL_fingerprint-v1/trend.v2|vp={pack}");
        assert_eq!(TrendBucket::version_pack_in(&key), Some(pack.as_str()));
    }

    #[test]
    fn baseline_gap_downgrades_trend_evidence() {
        let hour = HISTORY_BUCKET_SECONDS;
        let windows = TrendWindows::ending_at(6 * hour, 1, 6);
        let rule = TrendBucket::active_storage_rule_version();
        let buckets: Vec<_> = (0..7)
            .map(|index| {
                let (executions, latency) = if index == 6 { (10, 300) } else { (3, 100) };
                let mut delta = HistoryBucketDelta {
                    executions,
                    ..HistoryBucketDelta::default()
                };
                for _ in 0..executions {
                    delta.query_time.add(latency);
                    delta.scan_bytes.add(1_000);
                }
                TrendBucket {
                    bucket_start: index * hour,
                    engine_version: "3.5.14".into(),
                    rule_version: rule.clone(),
                    delta,
                }
            })
            .collect();
        let mut coverage: BTreeMap<_, _> = (0..7)
            .map(|index| (index * hour, CoverageHourState::Complete))
            .collect();
        let complete = evaluate_template_trend(&buckets, &coverage, windows).unwrap();
        assert_eq!(complete.evidence, EvidenceCompleteness::Complete);
        coverage.insert(hour, CoverageHourState::Partial);
        let partial = evaluate_template_trend(&buckets, &coverage, windows).unwrap();
        coverage.remove(&hour);
        let missing = evaluate_template_trend(&buckets, &coverage, windows).unwrap();
        for trend in [partial, missing] {
            assert_eq!(trend.evidence, EvidenceCompleteness::Partial);
            assert_eq!(trend.confidence, Some(TrendConfidence::Low));
            assert!(
                trend
                    .limitations
                    .contains(&"baseline_window_not_fully_collected")
            );
        }
    }

    #[test]
    fn coverage_gap_survives_incomparable_verdict() {
        let hour = HISTORY_BUCKET_SECONDS;
        let windows = TrendWindows::ending_at(6 * hour, 1, 6);
        let rule = TrendBucket::active_storage_rule_version();
        let buckets: Vec<_> = (0..7)
            .map(|index| {
                let (engine, executions, latency) = if index == 6 {
                    ("3.5.14", 10, 300)
                } else {
                    ("3.5.13", 3, 100)
                };
                let mut delta = HistoryBucketDelta {
                    executions,
                    ..HistoryBucketDelta::default()
                };
                for _ in 0..executions {
                    delta.query_time.add(latency);
                    delta.scan_bytes.add(1_000);
                }
                TrendBucket {
                    bucket_start: index * hour,
                    engine_version: engine.into(),
                    rule_version: rule.clone(),
                    delta,
                }
            })
            .collect();
        let mut coverage: BTreeMap<_, _> = (0..7)
            .map(|index| (index * hour, CoverageHourState::Complete))
            .collect();
        coverage.insert(hour, CoverageHourState::Partial);
        let trend = evaluate_template_trend(&buckets, &coverage, windows).unwrap();
        assert_eq!(
            trend.verdict,
            TrendVerdict::NotComparable {
                reason: TrendNotComparableReason::EngineVersionChanged,
            }
        );
        assert_eq!(trend.change, None);
        assert_eq!(trend.evidence, EvidenceCompleteness::Unavailable);
        assert!(
            trend
                .limitations
                .contains(&"baseline_window_not_fully_collected")
        );
    }

    #[test]
    fn series_distinguishes_unknown_executions_from_proven_zero() {
        let hour = HISTORY_BUCKET_SECONDS;
        let rule = TrendBucket::active_storage_rule_version();
        let coverage = BTreeMap::from([
            (0, CoverageHourState::Partial),
            (hour, CoverageHourState::Complete),
            (3 * hour, CoverageHourState::Partial),
        ]);
        let buckets = [TrendBucket {
            bucket_start: 3 * hour,
            engine_version: "3.5.14".into(),
            rule_version: rule.clone(),
            delta: HistoryBucketDelta {
                executions: 2,
                ..HistoryBucketDelta::default()
            },
        }];
        let points = trend_series(&buckets, &coverage, 0, 4 * hour, "3.5.14", &rule);
        let observed: Vec<_> = points.iter().map(|p| (p.coverage, p.executions)).collect();
        assert_eq!(
            observed,
            [
                (CoverageHourState::Partial, None),
                (CoverageHourState::Complete, Some(0)),
                (CoverageHourState::NotCollected, None),
                (CoverageHourState::Partial, Some(2)),
            ]
        );
    }

    #[test]
    fn latency_drop_without_paired_workload_is_unscored() {
        let hour = HISTORY_BUCKET_SECONDS;
        let windows = TrendWindows::ending_at(6 * hour, 1, 6);
        let rule = TrendBucket::active_storage_rule_version();
        let coverage = (0..7)
            .map(|index| (index * hour, CoverageHourState::Complete))
            .collect();
        for baseline_has_scan in [false, true] {
            let buckets: Vec<_> = (0..7)
                .map(|index| {
                    let (executions, latency) = if index == 6 { (10, 100) } else { (3, 400) };
                    let mut delta = HistoryBucketDelta {
                        executions,
                        ..HistoryBucketDelta::default()
                    };
                    for _ in 0..executions {
                        delta.query_time.add(latency);
                        if baseline_has_scan && index < 6 {
                            delta.scan_bytes.add(1_000);
                        }
                    }
                    TrendBucket {
                        bucket_start: index * hour,
                        engine_version: "3.5.14".into(),
                        rule_version: rule.clone(),
                        delta,
                    }
                })
                .collect();
            let trend = evaluate_template_trend(&buckets, &coverage, windows).unwrap();
            assert_eq!(
                trend.verdict,
                TrendVerdict::Unscored {
                    reason: TrendUnscoredReason::WorkloadSignalMissing,
                }
            );
            assert_eq!(trend.confidence, None);
            assert_eq!(trend.evidence, EvidenceCompleteness::Partial);
            assert_eq!(trend.change.unwrap().direction, TrendDirection::Down);
        }
    }

    #[test]
    fn valid_zero_scan_on_both_sides_is_a_workload_signal() {
        let hour = HISTORY_BUCKET_SECONDS;
        let windows = TrendWindows::ending_at(6 * hour, 1, 6);
        let rule = TrendBucket::active_storage_rule_version();
        let buckets: Vec<_> = (0..7)
            .map(|index| {
                let (executions, latency) = if index == 6 { (10, 100) } else { (3, 400) };
                let mut delta = HistoryBucketDelta {
                    executions,
                    ..HistoryBucketDelta::default()
                };
                for _ in 0..executions {
                    delta.query_time.add(latency);
                    delta.scan_bytes.add(0);
                }
                TrendBucket {
                    bucket_start: index * hour,
                    engine_version: "3.5.14".into(),
                    rule_version: rule.clone(),
                    delta,
                }
            })
            .collect();
        let coverage = (0..7)
            .map(|index| (index * hour, CoverageHourState::Complete))
            .collect();
        let trend = evaluate_template_trend(&buckets, &coverage, windows).unwrap();
        assert_eq!(trend.verdict, TrendVerdict::Improvement);
    }
}
