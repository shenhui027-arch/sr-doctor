//! Stage C trends (ADR-5) over the local history. The statistics live in
//! doctor-core; this module reads the store and projects typed facts.
use std::collections::BTreeMap;

use doctor_contracts::{
    AuditMetricFact, AuditMetricId, AuditMetricInput, CoverageHourState, HistorySeriesRequest,
    HistorySeriesResponse, HistoryTrendsRequest, HistoryTrendsResponse, TrendChange,
    TrendComparability, TrendCounts, TrendExemplar, TrendPoint, TrendRow, TrendRule, TrendSide,
    TrendVerdict, TrendWindow,
};
use doctor_core::{
    HISTORY_BUCKET_SECONDS, SideStats, TREND_K_MAD, TREND_MIN_BASELINE_HOURS,
    TREND_MIN_CURRENT_SAMPLES, TREND_MIN_HOUR_SAMPLES, TREND_MIN_RELATIVE_PERCENT, TemplateTrend,
    TrendBucket, TrendWindows, evaluate_template_trend, normalize_audit_metric, trend_series,
};
use doctor_history::{HistoryStore, RunRecord, StoredTemplate};

use super::*;
use crate::audit::no_store;
use crate::history::unix_now;

fn fact(value: u64) -> AuditMetricFact {
    metric_fact(AuditMetricId::QueryTime, value)
}

/// A stored source-unit value through the one typed parser.
pub(crate) fn metric_fact(metric: AuditMetricId, value: u64) -> AuditMetricFact {
    normalize_audit_metric(
        metric,
        AuditMetricInput::Text {
            value: value.to_string(),
        },
    )
}

pub(crate) fn percent_text(permille: i64) -> String {
    let sign = if permille < 0 { "-" } else { "" };
    format!(
        "{sign}{}.{}",
        permille.unsigned_abs() / 10,
        permille.unsigned_abs() % 10
    )
}

pub(crate) fn seconds(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

pub(crate) fn rule() -> TrendRule {
    TrendRule {
        k_mad: format!("{TREND_K_MAD}"),
        min_relative_percent: TREND_MIN_RELATIVE_PERCENT,
        min_baseline_hours: TREND_MIN_BASELINE_HOURS,
        min_hour_samples: TREND_MIN_HOUR_SAMPLES,
        min_current_samples: TREND_MIN_CURRENT_SAMPLES,
        calibrated: false,
    }
}

/// Collection state of each hour in `[start, end)` from the runs over it.
pub(crate) fn coverage_map(
    runs: &[RunRecord],
    start: i64,
    end: i64,
) -> BTreeMap<i64, CoverageHourState> {
    (start..end)
        .step_by(HISTORY_BUCKET_SECONDS as usize)
        .map(|hour| {
            let state =
                crate::history::coverage_interval(runs, hour, hour + HISTORY_BUCKET_SECONDS);
            (hour, state)
        })
        .collect()
}

fn side(stats: &SideStats) -> TrendSide {
    TrendSide {
        median: stats.median.map(fact),
        mad: stats.mad.map(fact),
        executions: stats.executions.to_string(),
        valid_samples: stats.valid.to_string(),
        hours: stats.hours,
        thin_hours: stats.thin_hours,
    }
}

fn row(template: &StoredTemplate, trend: TemplateTrend) -> TrendRow {
    TrendRow {
        template_key: template.template.template_key.clone(),
        kind: template.template.kind.as_str().to_owned(),
        template: template.template.template.clone(),
        catalog: template.template.catalog.clone(),
        database: template.template.database.clone(),
        verdict: trend.verdict,
        confidence: trend.confidence,
        evidence: trend.evidence,
        baseline: side(&trend.baseline),
        current: side(&trend.current),
        change: trend.change.map(|change| TrendChange {
            direction: change.direction,
            magnitude: fact(change.magnitude),
            relative_percent: change.relative_permille.map(percent_text),
        }),
        comparability: TrendComparability {
            engine_version: trend.engine_version,
            rule_version: trend.rule_version,
        },
        limitations: trend.limitations.into_iter().map(str::to_owned).collect(),
    }
}

fn rank(row: &TrendRow) -> (u8, u8, i64) {
    let relative = row
        .change
        .as_ref()
        .and_then(|c| c.relative_percent.as_deref())
        .and_then(|p| p.replace('.', "").parse::<i64>().ok())
        .unwrap_or(0);
    match &row.verdict {
        TrendVerdict::Regression { severity, .. } => (0, *severity as u8, -relative),
        TrendVerdict::Improvement => (1, 0, relative),
        TrendVerdict::NotComparable { .. } => (2, 0, 0),
        TrendVerdict::Unscored { .. } => (3, 0, -relative),
        TrendVerdict::Stable => (4, 0, 0),
    }
}

fn window(windows: TrendWindows) -> TrendWindow {
    TrendWindow {
        current_start_seconds: seconds(windows.current_start),
        baseline_start_seconds: seconds(windows.baseline_start),
        end_seconds: seconds(windows.end),
    }
}

pub(crate) struct Evaluated {
    pub(crate) templates: Vec<StoredTemplate>,
    pub(crate) trends: Vec<(StoredTemplate, TemplateTrend)>,
    pub(crate) buckets: BTreeMap<String, Vec<TrendBucket>>,
    pub(crate) coverage: BTreeMap<i64, CoverageHourState>,
}

pub(crate) fn evaluate(
    store: &HistoryStore,
    target: &str,
    windows: TrendWindows,
    series_start: i64,
) -> doctor_history::Result<Evaluated> {
    let from = windows.baseline_start.min(series_start);
    let templates = store.templates(target)?;
    let runs = store.runs(target, from, windows.end, u32::MAX)?;
    let coverage = coverage_map(&runs, from, windows.end);
    let mut buckets: BTreeMap<String, Vec<TrendBucket>> = BTreeMap::new();
    for bucket in store.buckets(target, from, windows.end, None)? {
        buckets
            .entry(bucket.template_key.clone())
            .or_default()
            .push(TrendBucket {
                bucket_start: bucket.delta.bucket_start,
                engine_version: bucket.engine_version,
                rule_version: bucket.rule_version,
                delta: bucket.delta,
            });
    }
    let trends = templates
        .iter()
        .filter_map(|template| {
            let own = buckets.get(&template.template.template_key)?;
            evaluate_template_trend(own, &coverage, windows).map(|mut trend| {
                if trend.rule_version != doctor_core::history_rule_version() {
                    trend.verdict = TrendVerdict::NotComparable {
                        reason: doctor_contracts::TrendNotComparableReason::RuleVersionChanged,
                    };
                    trend.change = None;
                    trend.confidence = None;
                    trend.evidence = doctor_contracts::EvidenceCompleteness::Unavailable;
                    trend
                        .limitations
                        .push("historical_rule_requires_new_samples");
                }
                (template.clone(), trend)
            })
        })
        .collect();
    Ok(Evaluated {
        templates,
        trends,
        buckets,
        coverage,
    })
}

pub(super) async fn trends(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: HistoryTrendsRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_history_trends_request",
            "history",
            "invalid_history_trends_request",
            "change_input",
        ));
    }
    let Some(store) = state.history.store().cloned() else {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::CONFLICT,
            "history_disabled",
            "history",
            "history_disabled",
            "change_input",
        ));
    };
    let windows =
        TrendWindows::ending_at(unix_now(), request.current_hours, request.baseline_hours);
    let worker =
        tokio::task::spawn_blocking(move || -> doctor_history::Result<HistoryTrendsResponse> {
            let targets = store.targets()?;
            let target = request.target.clone().or_else(|| targets.first().cloned());
            let mut rows = Vec::new();
            let mut counts = [0u64; 5];
            if let Some(target) = target
                .as_deref()
                .filter(|target| targets.iter().any(|known| known == *target))
            {
                for (template, trend) in
                    evaluate(&store, target, windows, windows.current_start)?.trends
                {
                    let slot = match trend.verdict {
                        TrendVerdict::Regression { .. } => 0,
                        TrendVerdict::Improvement => 1,
                        TrendVerdict::Stable => 2,
                        TrendVerdict::Unscored { .. } => 3,
                        TrendVerdict::NotComparable { .. } => 4,
                    };
                    counts[slot] += 1;
                    if slot != 2 {
                        rows.push(row(&template, trend));
                    }
                }
            }
            rows.sort_by_key(rank);
            let truncated = rows.len() > usize::from(request.limit);
            rows.truncate(usize::from(request.limit));
            Ok(HistoryTrendsResponse {
                target,
                targets,
                window: window(windows),
                rule: rule(),
                rows,
                truncated,
                counts: TrendCounts {
                    regression: counts[0].to_string(),
                    improvement: counts[1].to_string(),
                    stable: counts[2].to_string(),
                    unscored: counts[3].to_string(),
                    not_comparable: counts[4].to_string(),
                },
                limitations: [
                    "rule_thresholds_pending_g2_calibration",
                    "quantiles_are_log_histogram_approximations",
                    "hours_not_collected_are_never_interpolated",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            })
        });
    let response = worker
        .await
        .map_err(|_| internal_failure(&state))?
        .map_err(|_| internal_failure(&state))?;
    Ok(no_store(response))
}

pub(super) async fn series(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: HistorySeriesRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_history_series_request",
            "history",
            "invalid_history_series_request",
            "change_input",
        ));
    }
    let Some(store) = state.history.store().cloned() else {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::CONFLICT,
            "history_disabled",
            "history",
            "history_disabled",
            "change_input",
        ));
    };
    let windows =
        TrendWindows::ending_at(unix_now(), request.current_hours, request.baseline_hours);
    let series_start = windows.end - i64::from(request.points) * HISTORY_BUCKET_SECONDS;
    let worker = tokio::task::spawn_blocking(
        move || -> doctor_history::Result<Option<HistorySeriesResponse>> {
            let evaluated = evaluate(&store, &request.target, windows, series_start)?;
            let Some((template, trend)) = evaluated
                .trends
                .into_iter()
                .find(|(t, _)| t.template.template_key == request.template_key)
            else {
                return Ok(None);
            };
            let _ = evaluated.templates;
            let own = evaluated
                .buckets
                .get(&request.template_key)
                .cloned()
                .unwrap_or_default();
            let points = trend_series(
                &own,
                &evaluated.coverage,
                series_start,
                windows.end,
                &trend.engine_version,
                &doctor_core::history_rule_version(),
            )
            .into_iter()
            .map(|point| TrendPoint {
                hour_start_seconds: seconds(point.hour_start),
                coverage: point.coverage,
                executions: point.executions.map(|value| value.to_string()),
                median: point.median.map(fact),
                valid_samples: point.valid.to_string(),
                thin: point.thin,
                comparable: point.comparable,
            })
            .collect();
            let exemplars = store
                .exemplars(&request.template_key, windows.current_start, windows.end, 5)?
                .into_iter()
                .map(|exemplar| TrendExemplar {
                    query_id: exemplar.query_id,
                    query_time: fact(exemplar.query_time),
                    event_seconds: seconds(exemplar.event_time),
                })
                .collect();
            Ok(Some(HistorySeriesResponse {
                target: request.target.clone(),
                window: window(windows),
                rule: rule(),
                row: row(&template, trend),
                points,
                exemplars,
            }))
        },
    );
    match worker
        .await
        .map_err(|_| internal_failure(&state))?
        .map_err(|_| internal_failure(&state))?
    {
        Some(response) => Ok(no_store(response)),
        None => Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::NOT_FOUND,
            "history_template_not_found",
            "history",
            "history_template_not_found",
            "change_input",
        )),
    }
}
