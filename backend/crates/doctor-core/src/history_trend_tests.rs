//! RC-01..03 golden cases for the stage C trend rule.
use super::*;
use crate::HistoryMetric;

const NOW: i64 = 1_000 * HISTORY_BUCKET_SECONDS + 1_800;

#[test]
fn audit_v1_09_thin_current_hours_never_form_a_regression() {
    let windows = TrendWindows::ending_at(NOW, 12, 48);
    let mut buckets: Vec<_> = (12..36)
        .map(|ago| bucket(ago, "3.5.14", "v1", &[100; 5], &[1_000; 5], 5))
        .collect();
    buckets.extend((0..12).map(|ago| bucket(ago, "3.5.14", "v1", &[500], &[10_000], 1)));
    let trend = evaluate_template_trend(&buckets, &complete(), windows).unwrap();
    assert_eq!(
        trend.verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::InsufficientCurrent
        }
    );
    assert_eq!(
        (
            trend.current.valid,
            trend.current.median,
            trend.current.thin_hours,
            trend.current.hours
        ),
        (0, None, 12, 0)
    );
    assert_eq!(trend.current.executions, 12);
    // An eligible hour remains stable despite eleven extreme thin hours.
    buckets.retain(|b| b.bucket_start != 1_000 * HISTORY_BUCKET_SECONDS);
    buckets.push(bucket(0, "3.5.14", "v1", &[100; 10], &[1_000; 10], 10));
    let trend = evaluate_template_trend(&buckets, &complete(), windows).unwrap();
    assert_eq!(trend.verdict, TrendVerdict::Stable);
    assert_eq!(trend.current.valid, 10);
}

#[test]
fn audit_v1_09_mixed_rule_hours_never_publish_a_comparable_trend() {
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend(current(300, 3_000, 20));
    buckets.push(bucket(0, "3.5.14", "v2", &[100; 20], &[1_000; 20], 20));
    let trend = evaluate(buckets);
    assert_eq!(
        trend.verdict,
        TrendVerdict::NotComparable {
            reason: TrendNotComparableReason::RuleVersionChanged
        }
    );
    assert_eq!(trend.change, None);
}

fn metric(values: &[u64]) -> HistoryMetric {
    let mut metric = HistoryMetric::default();
    for value in values {
        metric.add(u128::from(*value));
    }
    metric
}

/// `hours_ago` 0 is the current hour.
fn bucket(
    hours_ago: i64,
    engine: &str,
    rule: &str,
    times: &[u64],
    scan: &[u64],
    executions: u64,
) -> TrendBucket {
    let start = (1_000 - hours_ago) * HISTORY_BUCKET_SECONDS;
    TrendBucket {
        bucket_start: start,
        engine_version: engine.into(),
        rule_version: rule.into(),
        delta: HistoryBucketDelta {
            template_key: "k".into(),
            bucket_start: start,
            executions,
            query_time: metric(times),
            scan_bytes: metric(scan),
            ..HistoryBucketDelta::default()
        },
    }
}

fn windows() -> TrendWindows {
    TrendWindows::ending_at(NOW, 2, 48)
}

fn complete() -> BTreeMap<i64, CoverageHourState> {
    (0..60)
        .map(|back| {
            (
                (1_000 - back) * HISTORY_BUCKET_SECONDS,
                CoverageHourState::Complete,
            )
        })
        .collect()
}

/// 24 baseline hours of five executions around 100 ms with 1000 scan bytes.
fn baseline(engine: &str, rule: &str) -> Vec<TrendBucket> {
    (2..26)
        .map(|ago| bucket(ago, engine, rule, &[96, 98, 100, 102, 104], &[1_000; 5], 5))
        .collect()
}

fn current(times: u64, scan: u64, samples: usize) -> Vec<TrendBucket> {
    (0..2)
        .map(|ago| {
            bucket(
                ago,
                "3.5.14",
                "v1",
                &vec![times; samples],
                &vec![scan; samples],
                samples as u64,
            )
        })
        .collect()
}

fn evaluate(buckets: Vec<TrendBucket>) -> TemplateTrend {
    evaluate_template_trend(&buckets, &complete(), windows()).unwrap()
}

#[test]
fn rc01_regression_needs_band_floor_samples_and_a_workload_signal() {
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend(current(250, 3_000, 20));
    let trend = evaluate(buckets);
    assert_eq!(
        trend.verdict,
        TrendVerdict::Regression {
            severity: TrendSeverity::High,
            workload: WorkloadSignal::Increased
        }
    );
    assert_eq!(trend.confidence, Some(TrendConfidence::High));
    assert_eq!(trend.evidence, EvidenceCompleteness::Complete);
    assert_eq!((trend.baseline.hours, trend.current.valid), (24, 40));
    let base = trend.baseline.median.unwrap();
    assert!((94..=106).contains(&base), "baseline median {base}");
    let change = trend.change.unwrap();
    assert_eq!(change.direction, TrendDirection::Up);
    assert!(change.relative_permille.unwrap() > 1_000);

    // Latency up with the same scan bytes: still a regression, flagged.
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend(current(160, 1_000, 20));
    let trend = evaluate(buckets);
    assert_eq!(
        trend.verdict,
        TrendVerdict::Regression {
            severity: TrendSeverity::Medium,
            workload: WorkloadSignal::Stable
        }
    );
    assert!(
        trend
            .limitations
            .contains(&"workload_stable_latency_rose_possible_contention")
    );

    // Latency alone, no scan bytes or CPU anywhere: not scored.
    let mut buckets: Vec<TrendBucket> = (2..26)
        .map(|ago| bucket(ago, "3.5.14", "v1", &[100; 5], &[], 5))
        .collect();
    buckets.extend((0..2).map(|ago| bucket(ago, "3.5.14", "v1", &[300; 20], &[], 20)));
    assert_eq!(
        evaluate(buckets).verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::WorkloadSignalMissing
        }
    );
}

#[test]
fn rc01_band_and_relative_floor_keep_small_moves_stable_and_report_improvements() {
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend(current(115, 1_000, 20));
    assert_eq!(evaluate(buckets).verdict, TrendVerdict::Stable);
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend(current(40, 400, 20));
    let trend = evaluate(buckets);
    assert_eq!(trend.verdict, TrendVerdict::Improvement);
    assert_eq!(trend.change.unwrap().direction, TrendDirection::Down);
}

#[test]
fn rc01_zero_missing_overflow_and_thin_samples_are_distinct() {
    // A real 0 baseline: absolute change only, no percentage, no score.
    let mut buckets: Vec<TrendBucket> = (2..26)
        .map(|ago| bucket(ago, "3.5.14", "v1", &[0; 5], &[0; 5], 5))
        .collect();
    buckets.extend(current(50, 0, 20));
    let trend = evaluate(buckets);
    assert_eq!(
        trend.verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::ZeroBaseline
        }
    );
    assert_eq!(trend.baseline.median, Some(0));
    let change = trend.change.unwrap();
    assert_eq!(change.relative_permille, None);
    assert!(change.magnitude > 0);

    // Missing query times are not 0: 20 executions, no valid sample.
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend((0..2).map(|ago| bucket(ago, "3.5.14", "v1", &[], &[], 10)));
    let trend = evaluate(buckets);
    assert_eq!(
        trend.verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::InsufficientCurrent
        }
    );
    assert_eq!(
        (
            trend.current.executions,
            trend.current.valid,
            trend.current.median
        ),
        (20, 0, None)
    );
    assert_eq!(trend.evidence, EvidenceCompleteness::Partial);
    assert!(
        trend
            .limitations
            .contains(&"some_executions_lack_a_valid_query_time")
    );

    // A saturated value does not move the median or overflow the change.
    let mut buckets: Vec<TrendBucket> = (2..26)
        .map(|ago| {
            bucket(
                ago,
                "3.5.14",
                "v1",
                &[96, 98, 100, 102, u64::MAX],
                &[1_000; 5],
                5,
            )
        })
        .collect();
    buckets.extend(current(100, 1_000, 20));
    let trend = evaluate(buckets);
    assert_eq!(trend.verdict, TrendVerdict::Stable);
    assert!(trend.baseline.median.unwrap() < 200);

    // Hours under the per-hour minimum are counted as thin, not used.
    let mut buckets: Vec<TrendBucket> = (2..26)
        .map(|ago| bucket(ago, "3.5.14", "v1", &[100, 100], &[1_000; 2], 2))
        .collect();
    buckets.extend(current(300, 3_000, 20));
    let trend = evaluate(buckets);
    assert_eq!((trend.baseline.hours, trend.baseline.thin_hours), (0, 24));
    assert_eq!(
        trend.verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::InsufficientBaseline
        }
    );

    // Partial collection in the current window lowers confidence.
    let mut coverage = complete();
    coverage.insert(1_000 * HISTORY_BUCKET_SECONDS, CoverageHourState::Partial);
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend(current(250, 3_000, 20));
    let trend = evaluate_template_trend(&buckets, &coverage, windows()).unwrap();
    assert_eq!(
        (trend.evidence, trend.confidence),
        (EvidenceCompleteness::Partial, Some(TrendConfidence::Low))
    );
}

#[test]
fn rc02_version_or_rule_change_makes_old_buckets_not_comparable() {
    let mut buckets = baseline("3.5.13", "v1");
    buckets.extend(current(250, 3_000, 20));
    let trend = evaluate(buckets);
    assert_eq!(
        trend.verdict,
        TrendVerdict::NotComparable {
            reason: TrendNotComparableReason::EngineVersionChanged
        }
    );
    assert_eq!(
        (trend.evidence, trend.change),
        (EvidenceCompleteness::Unavailable, None)
    );
    let mut buckets = baseline("3.5.14", "v0");
    buckets.extend(current(250, 3_000, 20));
    assert_eq!(
        evaluate(buckets).verdict,
        TrendVerdict::NotComparable {
            reason: TrendNotComparableReason::RuleVersionChanged
        }
    );
    // Mixed: only same-version hours form the baseline.
    let mut buckets = baseline("3.5.14", "v1");
    buckets.extend((26..40).map(|ago| bucket(ago, "3.5.13", "v1", &[900; 5], &[9_000; 5], 5)));
    buckets.extend(current(100, 1_000, 20));
    let trend = evaluate(buckets);
    assert_eq!(trend.verdict, TrendVerdict::Stable);
    assert!(
        trend
            .limitations
            .contains(&"older_version_buckets_excluded_from_baseline")
    );
}

#[test]
fn rc03_no_baseline_is_never_scored() {
    let trend = evaluate(current(250, 3_000, 20));
    assert_eq!(
        trend.verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::InsufficientBaseline
        }
    );
    assert_eq!((trend.confidence, trend.change), (None, None));
    let short: Vec<TrendBucket> = (2..6)
        .map(|ago| bucket(ago, "3.5.14", "v1", &[100; 5], &[1_000; 5], 5))
        .chain(current(250, 3_000, 20))
        .collect();
    assert_eq!(
        evaluate(short).verdict,
        TrendVerdict::Unscored {
            reason: TrendUnscoredReason::InsufficientBaseline
        }
    );
    // Not run in the current window: no row at all.
    assert!(evaluate_template_trend(&baseline("3.5.14", "v1"), &complete(), windows()).is_none());
}

#[test]
fn series_keeps_not_collected_hours_apart_from_true_zero() {
    let buckets = vec![
        bucket(3, "3.5.14", "v1", &[100; 5], &[], 5),
        bucket(1, "3.5.14", "v1", &[100], &[], 1),
        bucket(0, "3.5.13", "v1", &[100; 5], &[], 5),
    ];
    let mut coverage = BTreeMap::new();
    for ago in [3, 2, 1, 0] {
        coverage.insert(
            (1_000 - ago) * HISTORY_BUCKET_SECONDS,
            CoverageHourState::Complete,
        );
    }
    let points = trend_series(
        &buckets,
        &coverage,
        996 * HISTORY_BUCKET_SECONDS,
        1_001 * HISTORY_BUCKET_SECONDS,
        "3.5.14",
        "v1",
    );
    let shape: Vec<(Option<u64>, bool, bool)> = points
        .iter()
        .map(|p| (p.executions, p.thin, p.comparable))
        .collect();
    assert_eq!(
        shape,
        vec![
            (None, false, true),
            (Some(5), false, true),
            (Some(0), false, true),
            (Some(1), true, true),
            (Some(5), true, false)
        ]
    );
    assert_eq!(points[0].coverage, CoverageHourState::NotCollected);
}

fn stats(median: u64, scan: Option<u64>, valid: u64, engine: &str) -> WindowStats {
    WindowStats {
        executions: valid,
        valid,
        query_time_median: Some(median),
        scan_bytes_median: scan,
        cpu_cost_median: None,
        engine_version: engine.into(),
        rule_version: "v1".into(),
    }
}

#[test]
fn audit_v1_04_mixed_after_versions_block_improvement_before_sample_checks() {
    let before = stats(250, Some(3_000), 40, "3.5.14");
    let after = stats(150, Some(3_000), 40, "3.5.14");
    assert_eq!(
        verify_change(
            &before,
            Some(&after),
            &[
                ("3.5.14".into(), "v1".into()),
                ("3.5.15".into(), "v1".into())
            ]
        ),
        ChangeVerdict::NotComparable(TrendNotComparableReason::EngineVersionChanged)
    );
    assert_eq!(
        verify_change(&before, Some(&after), &[("3.5.14".into(), "v2".into())]),
        ChangeVerdict::NotComparable(TrendNotComparableReason::RuleVersionChanged)
    );
    assert_eq!(
        verify_change(&before, Some(&stats(150, Some(3_000), 1, "3.5.15")), &[]),
        ChangeVerdict::NotComparable(TrendNotComparableReason::EngineVersionChanged)
    );
}

#[test]
fn rd03_verification_needs_a_real_drop_samples_a_workload_signal_and_one_version() {
    let before = stats(250, Some(3_000), 40, "3.5.14");
    assert_eq!(
        verify_change(&before, Some(&stats(150, Some(3_000), 40, "3.5.14")), &[]),
        ChangeVerdict::Improved
    );
    // Faster but scanning far more: not an improvement.
    assert_eq!(
        verify_change(&before, Some(&stats(150, Some(9_000), 40, "3.5.14")), &[]),
        ChangeVerdict::NotImproved
    );
    assert_eq!(
        verify_change(&before, Some(&stats(230, Some(3_000), 40, "3.5.14")), &[]),
        ChangeVerdict::NotImproved
    );
    assert_eq!(
        verify_change(&before, Some(&stats(150, Some(3_000), 5, "3.5.14")), &[]),
        ChangeVerdict::InsufficientSamples
    );
    assert_eq!(
        verify_change(&before, Some(&stats(150, None, 40, "3.5.14")), &[]),
        ChangeVerdict::WorkloadSignalMissing
    );
    assert_eq!(
        verify_change(&before, None, &[]),
        ChangeVerdict::InsufficientSamples
    );
    assert_eq!(
        verify_change(&before, None, &[("3.5.15".into(), "v1".into())]),
        ChangeVerdict::NotComparable(TrendNotComparableReason::EngineVersionChanged)
    );
    assert_eq!(
        verify_change(&before, None, &[("3.5.14".into(), "v2".into())]),
        ChangeVerdict::NotComparable(TrendNotComparableReason::RuleVersionChanged)
    );
}

#[test]
fn window_stats_merge_one_version_and_pick_the_newest_by_default() {
    let mut buckets = baseline("3.5.14", "v1");
    buckets.push(bucket(1, "3.5.15", "v1", &[500; 5], &[50; 5], 5));
    let all = window_stats(&buckets, 0, 2_000 * HISTORY_BUCKET_SECONDS, None).unwrap();
    assert_eq!((all.engine_version.as_str(), all.executions), ("3.5.15", 5));
    let old = window_stats(
        &buckets,
        0,
        2_000 * HISTORY_BUCKET_SECONDS,
        Some(("3.5.14", "v1")),
    )
    .unwrap();
    assert_eq!((old.executions, old.valid), (120, 120));
    assert!((94..=106).contains(&old.query_time_median.unwrap()));
    assert!(window_stats(&buckets, 0, 10, None).is_none());
}
