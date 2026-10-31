//! Task 59 (L2): load alerts from the stored load history, evaluated after
//! every collection cycle. Five rules, on trial until the G2 calibration:
//! Routine Load paused, stopped or cancelled, lag growing three hours in a
//! row, and a table's failure or filter rate jumping against its 7-day
//! baseline. Too few samples, an incompletely collected window or a version
//! change decides nothing: no alert is raised and none is resolved.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    AlertAction, AlertActionRequest, AlertState, CoverageHourState, LOAD_ALERT_LIST_LIMIT,
    LoadAlertCounts, LoadAlertDetail, LoadAlertLagHour, LoadAlertRule, LoadAlertThresholds,
    LoadAlertView, LoadAlertWindow, LoadAlertsRequest, LoadAlertsResponse, LoadKindGroup,
    TrendSeverity,
};
use doctor_core::{HISTORY_BUCKET_SECONDS, LoadHistoryKind, history_rule_version};
use doctor_history::{AlertUpsert, HistoryStore, LoadAlertFacts, StoreChange, StoredLoadAlert};

use super::*;
use crate::audit::no_store;
use crate::audit::performance::{linear_axis, permyriad};
use crate::history::unix_now;
use crate::history_trend::{coverage_map, seconds};

const HOUR: i64 = HISTORY_BUCKET_SECONDS;
/// No load recorded the StarRocks version (shown as "not recorded").
pub(crate) const UNKNOWN_VERSION: &str = "unknown";
pub(crate) const CURRENT_HOURS: i64 = 3;
pub(crate) const BASELINE_HOURS: i64 = 168;
pub(crate) const MUTE_HOURS: i64 = 24;
/// Rises in a row the lag rule asks for (so this many + 1 hours of data).
pub(crate) const LAG_RISES: i64 = 3;
const LAG_MIN: u128 = 10_000;
const LAG_HIGH: u128 = 1_000_000;
const LAG_MIN_RATIO_PERMILLE: u128 = 2_000;

/// One rate rule's thresholds, rates in parts per million.
#[derive(Clone, Copy)]
struct RateRule {
    min_total: u128,
    min_baseline_total: u128,
    min_ppm: u32,
    min_delta_ppm: u32,
    min_ratio: u32,
    high_ppm: u32,
    /// Resolves at or below baseline + this.
    resolve_delta_ppm: u32,
}

const FAILURE_RULE: RateRule = RateRule {
    min_total: 20,
    min_baseline_total: 100,
    min_ppm: 50_000,
    min_delta_ppm: 50_000,
    min_ratio: 3,
    high_ppm: 200_000,
    resolve_delta_ppm: 25_000,
};

const FILTER_RULE: RateRule = RateRule {
    min_total: 1_000,
    min_baseline_total: 10_000,
    min_ppm: 10_000,
    min_delta_ppm: 10_000,
    min_ratio: 3,
    high_ppm: 100_000,
    resolve_delta_ppm: 5_000,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    Raise(TrendSeverity),
    Resolve,
    /// Not enough to decide either way.
    Keep,
}

fn ppm(part: u128, total: u128) -> u32 {
    u32::try_from(part.saturating_mul(1_000_000) / total.max(1)).unwrap_or(u32::MAX)
}

/// `part / total` against `ppm / 1e6`, exactly: `part * 1e6` vs `ppm * total`;
/// `None` when the products do not fit.
fn at_least(part: u128, total: u128, ppm: u32) -> Option<bool> {
    Some(part.checked_mul(1_000_000)? >= u128::from(ppm).checked_mul(total)?)
}

/// Compared on the raw counts, never on rounded rates: a baseline of 1 in
/// a million is not zero, and 4.9999 points is not 5. Counts too large to
/// compare exactly decide nothing.
fn rate_decision(rule: RateRule, current: (u128, u128), baseline: (u128, u128)) -> Decision {
    let ((total, part), (base_total, base_part)) = (current, baseline);
    if total < rule.min_total || base_total < rule.min_baseline_total {
        return Decision::Keep;
    }
    exact_rate_decision(rule, (total, part), (base_total, base_part)).unwrap_or(Decision::Keep)
}

fn exact_rate_decision(
    rule: RateRule,
    (total, part): (u128, u128),
    (base_total, base_part): (u128, u128),
) -> Option<Decision> {
    // part/total - base_part/base_total, scaled by total * base_total * 1e6.
    let cross = part.checked_mul(base_total)?;
    let base_cross = base_part.checked_mul(total)?;
    let now = cross.checked_mul(1_000_000)?;
    let before = base_cross.checked_mul(1_000_000)?;
    let scale = total.checked_mul(base_total)?;
    let delta_at_least = |ppm: u32| -> Option<bool> {
        Some(now >= before.checked_add(u128::from(ppm).checked_mul(scale)?)?)
    };
    let raise = at_least(part, total, rule.min_ppm)?
        && delta_at_least(rule.min_delta_ppm)?
        && cross >= base_cross.checked_mul(u128::from(rule.min_ratio))?;
    Some(if raise {
        Decision::Raise(if at_least(part, total, rule.high_ppm)? {
            TrendSeverity::High
        } else {
            TrendSeverity::Medium
        })
    } else if now <= before.checked_add(u128::from(rule.resolve_delta_ppm).checked_mul(scale)?)? {
        Decision::Resolve
    } else {
        Decision::Keep
    })
}

/// The difference in parts per million and the ratio in thousandths (none
/// when the baseline has no part), from the raw counts. Counts too large
/// for the exact form fall back to the difference of the two rounded rates
/// and no ratio.
fn rate_change(current: (u128, u128), baseline: (u128, u128)) -> (i32, Option<u32>) {
    let ((total, part), (base_total, base_part)) = (current, baseline);
    let exact = || -> Option<(i128, Option<u32>)> {
        let now = i128::try_from(part.checked_mul(base_total)?).ok()?;
        let before = i128::try_from(base_part.checked_mul(total)?).ok()?;
        let scale = i128::try_from(total.checked_mul(base_total)?.max(1)).ok()?;
        let delta = now.checked_sub(before)?.checked_mul(1_000_000)? / scale;
        let ratio = if base_part > 0 {
            let ratio = part.checked_mul(base_total)?.checked_mul(1_000)?
                / base_part.checked_mul(total)?.max(1);
            Some(u32::try_from(ratio).unwrap_or(u32::MAX))
        } else {
            None
        };
        Some((delta, ratio))
    };
    let (delta, ratio) = exact().unwrap_or_else(|| {
        (
            i128::from(ppm(part, total)) - i128::from(ppm(base_part, base_total)),
            None,
        )
    });
    (
        i32::try_from(delta).unwrap_or(if delta < 0 { i32::MIN } else { i32::MAX }),
        ratio,
    )
}

/// The last `LAG_RISES + 1` hourly maxima, oldest first; `None` where an
/// hour has no lag reading.
/// Every hour of the window must have a reading: one missing hour decides
/// nothing, neither a raise nor a resolve.
fn lag_decision(values: &[Option<u128>], running: bool) -> Decision {
    let Some(series) = values.iter().copied().collect::<Option<Vec<u128>>>() else {
        return Decision::Keep;
    };
    let rising = series.windows(2).all(|pair| pair[1] > pair[0]);
    let (first, before, latest) = (
        series[0],
        series[series.len() - 2],
        series[series.len() - 1],
    );
    if running
        && rising
        && latest >= LAG_MIN
        && latest.saturating_mul(1_000) >= first.saturating_mul(LAG_MIN_RATIO_PERMILLE)
    {
        Decision::Raise(if latest >= LAG_HIGH {
            TrendSeverity::High
        } else {
            TrendSeverity::Medium
        })
    } else if latest <= before || latest < LAG_MIN {
        Decision::Resolve
    } else {
        Decision::Keep
    }
}

fn severity_text(severity: TrendSeverity) -> &'static str {
    match severity {
        TrendSeverity::High => "high",
        TrendSeverity::Medium => "medium",
        _ => "low",
    }
}

fn group(kind: LoadHistoryKind) -> LoadKindGroup {
    match kind {
        LoadHistoryKind::StreamLoad => LoadKindGroup::StreamLoad,
        LoadHistoryKind::Insert => LoadKindGroup::Insert,
        LoadHistoryKind::RoutineLoad => LoadKindGroup::RoutineLoad,
        LoadHistoryKind::Broker => LoadKindGroup::Broker,
        LoadHistoryKind::Other => LoadKindGroup::Other,
    }
}

pub(crate) fn kind_label(kind: LoadKindGroup) -> &'static str {
    match kind {
        LoadKindGroup::StreamLoad => "Stream Load",
        LoadKindGroup::Insert => "INSERT",
        LoadKindGroup::RoutineLoad => "Routine Load",
        LoadKindGroup::Broker => "Broker Load",
        LoadKindGroup::Other => "Other",
    }
}

#[derive(Default)]
struct RateSums {
    loads: u128,
    failed: u128,
    loaded: u128,
    filtered: u128,
    unknown: u64,
}

struct Raised<'a> {
    store: &'a HistoryStore,
    target: &'a str,
    now: i64,
    raised: Vec<i64>,
    resolved: Vec<i64>,
}

impl Raised<'_> {
    fn raise(
        &mut self,
        rule: LoadAlertRule,
        subject: &str,
        severity: TrendSeverity,
        detail: &LoadAlertDetail,
        (engine_version, rule_version): (&str, &str),
    ) -> doctor_history::Result<()> {
        let facts = LoadAlertFacts {
            severity: severity_text(severity).to_owned(),
            facts_json: serde_json::to_string(detail)
                .map_err(|_| doctor_history::HistoryError::Internal("load_alert_facts"))?,
            engine_version: engine_version.to_owned(),
            rule_version: rule_version.to_owned(),
        };
        match self
            .store
            .upsert_load_alert(self.target, rule.as_str(), subject, &facts, self.now)?
        {
            AlertUpsert::Created(id) | AlertUpsert::Reopened(id) => self.raised.push(id),
            AlertUpsert::Updated | AlertUpsert::Suppressed => {}
        }
        Ok(())
    }

    fn resolve(&mut self, rule: LoadAlertRule, subject: &str) -> doctor_history::Result<()> {
        self.resolved.extend(self.store.resolve_load_alerts(
            self.target,
            rule.as_str(),
            subject,
            self.now,
        )?);
        Ok(())
    }
}

/// Evaluates every load rule for one target; returns the alert ids raised
/// (new, or reopened after a mute) and resolved.
pub(crate) fn evaluate_load_alerts(
    store: &HistoryStore,
    target: &str,
    now: i64,
) -> doctor_history::Result<(Vec<i64>, Vec<i64>)> {
    let end = now - now.rem_euclid(HOUR);
    let current_start = end - CURRENT_HOURS * HOUR;
    let baseline_start = current_start - BASELINE_HOURS * HOUR;
    let mut out = Raised {
        store,
        target,
        now,
        raised: Vec::new(),
        resolved: Vec::new(),
    };
    let buckets = store.load_buckets(target, baseline_start, end)?;
    // Each alert carries the version of its own evidence: the windows of a
    // rate, the snapshots of a Routine Load job.
    let rules = history_rule_version();

    // Rate rules: only on a completely collected current window.
    let coverage = coverage_map(
        &store.load_runs(target, baseline_start, end)?,
        baseline_start,
        end,
    );
    let complete = |hour: i64| coverage.get(&hour) == Some(&CoverageHourState::Complete);
    let current_complete = (0..CURRENT_HOURS).all(|index| complete(current_start + index * HOUR));
    let baseline_hours = (0..BASELINE_HOURS)
        .filter(|index| complete(baseline_start + index * HOUR))
        .count() as u16;
    if current_complete {
        type Key = (String, String, LoadKindGroup);
        let mut current: BTreeMap<Key, RateSums> = BTreeMap::new();
        let mut baseline: BTreeMap<Key, RateSums> = BTreeMap::new();
        let mut versions: BTreeMap<Key, BTreeSet<(String, String)>> = BTreeMap::new();
        for bucket in &buckets {
            let delta = &bucket.delta;
            let kind = group(delta.kind.unwrap_or(LoadHistoryKind::Other));
            if delta.system
                || !matches!(
                    kind,
                    LoadKindGroup::StreamLoad | LoadKindGroup::Insert | LoadKindGroup::Broker
                )
            {
                continue;
            }
            let in_current = delta.bucket_start >= current_start;
            if !in_current && !complete(delta.bucket_start) {
                continue;
            }
            let key = (delta.database.clone(), delta.table.clone(), kind);
            versions
                .entry(key.clone())
                .or_default()
                .insert((bucket.engine_version.clone(), bucket.rule_version.clone()));
            let sums = if in_current {
                current.entry(key).or_default()
            } else {
                baseline.entry(key).or_default()
            };
            sums.loads += u128::from(delta.loads);
            sums.failed += u128::from(delta.failed);
            sums.loaded += delta.loaded_rows.sum;
            sums.filtered += delta.filtered_rows.sum;
            sums.unknown += delta.loaded_rows.unknown + delta.filtered_rows.unknown;
        }
        for (key, now_sums) in &current {
            // A version or rule change between the windows is not comparable.
            let Some((engine, rule)) = versions
                .get(key)
                .filter(|pairs| pairs.len() == 1)
                .and_then(|pairs| pairs.first())
            else {
                continue;
            };
            let version = (engine.as_str(), rule.as_str());
            let Some(before) = baseline.get(key) else {
                continue;
            };
            let (database, table, kind) = key;
            let subject = format!("{database}.{table}|{}", kind_label(*kind));
            let window =
                |total: u128, part: u128, start: i64, stop: i64, hours: u16| LoadAlertWindow {
                    start_seconds: seconds(start),
                    end_seconds: seconds(stop),
                    total: total.to_string(),
                    part: part.to_string(),
                    rate_ppm: ppm(part, total),
                    collected_hours: hours,
                };
            match rate_decision(
                FAILURE_RULE,
                (now_sums.loads, now_sums.failed),
                (before.loads, before.failed),
            ) {
                Decision::Raise(severity) => out.raise(
                    LoadAlertRule::FailureRate,
                    &subject,
                    severity,
                    &LoadAlertDetail::FailureRate {
                        database: database.clone(),
                        table: table.clone(),
                        kind: *kind,
                        current: window(
                            now_sums.loads,
                            now_sums.failed,
                            current_start,
                            end,
                            CURRENT_HOURS as u16,
                        ),
                        baseline: window(
                            before.loads,
                            before.failed,
                            baseline_start,
                            current_start,
                            baseline_hours,
                        ),
                        delta_ppm: rate_change(
                            (now_sums.loads, now_sums.failed),
                            (before.loads, before.failed),
                        )
                        .0,
                        ratio_permille: rate_change(
                            (now_sums.loads, now_sums.failed),
                            (before.loads, before.failed),
                        )
                        .1,
                    },
                    version,
                )?,
                Decision::Resolve => out.resolve(LoadAlertRule::FailureRate, &subject)?,
                Decision::Keep => {}
            }
            // Filter rates need both counts from every load.
            if now_sums.unknown == 0 && before.unknown == 0 {
                let (now_input, before_input) = (
                    now_sums.loaded + now_sums.filtered,
                    before.loaded + before.filtered,
                );
                match rate_decision(
                    FILTER_RULE,
                    (now_input, now_sums.filtered),
                    (before_input, before.filtered),
                ) {
                    Decision::Raise(severity) => out.raise(
                        LoadAlertRule::FilterRate,
                        &subject,
                        severity,
                        &LoadAlertDetail::FilterRate {
                            database: database.clone(),
                            table: table.clone(),
                            kind: *kind,
                            current: window(
                                now_input,
                                now_sums.filtered,
                                current_start,
                                end,
                                CURRENT_HOURS as u16,
                            ),
                            baseline: window(
                                before_input,
                                before.filtered,
                                baseline_start,
                                current_start,
                                baseline_hours,
                            ),
                            delta_ppm: rate_change(
                                (now_input, now_sums.filtered),
                                (before_input, before.filtered),
                            )
                            .0,
                            ratio_permille: rate_change(
                                (now_input, now_sums.filtered),
                                (before_input, before.filtered),
                            )
                            .1,
                        },
                        version,
                    )?,
                    Decision::Resolve => out.resolve(LoadAlertRule::FilterRate, &subject)?,
                    Decision::Keep => {}
                }
            }
        }
    }

    // Routine Load rules: only while snapshots are being taken.
    let Some(last) = store.routine_snapshot_last(target)? else {
        return Ok((out.raised, out.resolved));
    };
    let (latest_at, latest_truncated) = (last.observed_at, last.truncated);
    // State alerts come from the latest snapshot and carry its version.
    let snapshot_version = (
        last.engine_version.as_deref().unwrap_or(UNKNOWN_VERSION),
        last.rule_version.as_deref().unwrap_or(rules.as_str()),
    );
    if latest_at < end - HOUR {
        return Ok((out.raised, out.resolved));
    }
    // Only a complete latest snapshot can show that a job is gone.
    let latest_complete = !latest_truncated;
    let last_seen = store.routine_last_seen(target)?;
    let gone = |job_id: &str| {
        latest_complete && last_seen.get(job_id).is_none_or(|seen| *seen < latest_at)
    };
    let recent_start = end - (LAG_RISES + 2) * HOUR;
    let snapshots = store.routine_snapshot_hours(target, recent_start, end + HOUR)?;
    let hours = store.routine_hours(target, recent_start, end + HOUR)?;
    let events = store.routine_events(target, now - 30 * 86_400, now + 1)?;
    let mut jobs: BTreeMap<&str, Vec<&doctor_history::StoredRoutineHour>> = BTreeMap::new();
    for hour in &hours {
        jobs.entry(hour.job_id.as_str()).or_default().push(hour);
    }
    // The lag window: the last complete hours, oldest first. Each must have a
    // complete snapshot under one known StarRocks version and rule set.
    let lag_hours: Vec<i64> = (0..=LAG_RISES)
        .map(|index| end - (LAG_RISES + 1 - index) * HOUR)
        .collect();
    let window_versions: Option<BTreeSet<(&str, &str)>> = lag_hours
        .iter()
        .map(|hour| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.bucket_start == *hour && snapshot.complete())
                .and_then(|snapshot| {
                    Some((
                        snapshot.engine_version.as_deref()?,
                        snapshot.rule_version.as_deref()?,
                    ))
                })
                .filter(|(engine, rule)| *engine != "*" && *rule != "*")
        })
        .collect();
    let lag_version = window_versions
        .filter(|versions| versions.len() == 1)
        .and_then(|versions| versions.first().copied());
    let lag_window_usable = lag_version.is_some();
    let running = |state: &str| matches!(state, "RUNNING" | "NEED_SCHEDULE");
    let ended = |state: &str| matches!(state, "STOPPED" | "CANCELLED");
    const JOB_RULES: [LoadAlertRule; 3] = [
        LoadAlertRule::RoutinePaused,
        LoadAlertRule::RoutineStopped,
        LoadAlertRule::LagGrowing,
    ];
    for (job_id, rows) in &jobs {
        if gone(job_id) {
            for rule in JOB_RULES {
                out.resolve(rule, job_id)?;
            }
            continue;
        }
        if last_seen.get(*job_id).is_none_or(|seen| *seen < latest_at) {
            // Not in a truncated latest snapshot: nothing is known now.
            continue;
        }
        let latest = rows
            .iter()
            .max_by_key(|row| row.bucket_start)
            .expect("rows");
        let state = latest.last_state.as_str();
        let last_event = |to: &str| {
            events
                .iter()
                .filter(|event| event.job_id == *job_id && event.to_state == to)
                .max_by_key(|event| event.observed_at)
        };
        if state == "PAUSED" {
            let event = last_event("PAUSED");
            out.raise(
                LoadAlertRule::RoutinePaused,
                job_id,
                TrendSeverity::High,
                &LoadAlertDetail::RoutinePaused {
                    job_id: (*job_id).to_owned(),
                    name: latest.name.clone(),
                    database: latest.database.clone(),
                    table: latest.table.clone(),
                    observed_seconds: seconds(event.map_or(latest.bucket_start, |e| e.observed_at)),
                    reason: event.and_then(|e| e.reason.clone()),
                    max_lag: latest.max_lag.map(|lag| lag.to_string()),
                },
                snapshot_version,
            )?;
        } else if running(state) {
            // Only running again resolves a pause; a pause that ends in a stop
            // stays until the job is gone.
            out.resolve(LoadAlertRule::RoutinePaused, job_id)?;
        }
        // Ended jobs alert only when the change itself was seen; a job first
        // seen already ended is old news. They resolve only when gone.
        if let Some(event) =
            last_event(state).filter(|event| ended(state) && event.from_state.is_some())
        {
            out.raise(
                LoadAlertRule::RoutineStopped,
                job_id,
                TrendSeverity::Medium,
                &LoadAlertDetail::RoutineStopped {
                    job_id: (*job_id).to_owned(),
                    name: latest.name.clone(),
                    database: latest.database.clone(),
                    table: latest.table.clone(),
                    state: state.to_owned(),
                    observed_seconds: seconds(event.observed_at),
                    reason: event.reason.clone(),
                },
                snapshot_version,
            )?;
        }
        let values: Vec<Option<u128>> = lag_hours
            .iter()
            .map(|hour| {
                lag_window_usable
                    .then(|| {
                        rows.iter()
                            .find(|row| row.bucket_start == *hour && row.lag_snapshots > 0)
                            .and_then(|row| row.max_lag)
                    })
                    .flatten()
            })
            .collect();
        match lag_decision(&values, running(state)) {
            Decision::Raise(severity) => {
                let series: Vec<u128> = values.iter().map(|v| v.unwrap_or(0)).collect();
                let (top, _) = linear_axis(*series.iter().max().unwrap_or(&0));
                out.raise(
                    LoadAlertRule::LagGrowing,
                    job_id,
                    severity,
                    &LoadAlertDetail::LagGrowing {
                        job_id: (*job_id).to_owned(),
                        name: latest.name.clone(),
                        database: latest.database.clone(),
                        table: latest.table.clone(),
                        hours: lag_hours
                            .iter()
                            .zip(&series)
                            .map(|(hour, lag)| LoadAlertLagHour {
                                start_seconds: seconds(*hour),
                                max_lag: lag.to_string(),
                                height_permyriad: permyriad(*lag, top),
                            })
                            .collect(),
                        ratio_permille: u32::try_from(
                            series[series.len() - 1].saturating_mul(1_000) / series[0].max(1),
                        )
                        .unwrap_or(u32::MAX),
                    },
                    lag_version.expect("usable window"),
                )?;
            }
            Decision::Resolve => out.resolve(LoadAlertRule::LagGrowing, job_id)?,
            Decision::Keep => {}
        }
    }
    // Active job alerts whose job left the recent hours altogether.
    for rule in JOB_RULES {
        for alert in store.active_load_alerts(target, rule.as_str())? {
            if !jobs.contains_key(alert.subject.as_str()) && gone(&alert.subject) {
                out.resolve(rule, &alert.subject)?;
            }
        }
    }
    Ok((out.raised, out.resolved))
}

fn rule_of(text: &str) -> Option<LoadAlertRule> {
    Some(match text {
        "routine_paused" => LoadAlertRule::RoutinePaused,
        "routine_stopped" => LoadAlertRule::RoutineStopped,
        "lag_growing" => LoadAlertRule::LagGrowing,
        "failure_rate" => LoadAlertRule::FailureRate,
        "filter_rate" => LoadAlertRule::FilterRate,
        _ => return None,
    })
}

fn state_text(state: AlertState) -> &'static str {
    match state {
        AlertState::Open => "open",
        AlertState::Acknowledged => "acknowledged",
        AlertState::Resolved => "resolved",
        AlertState::Muted => "muted",
    }
}

pub(crate) fn alert_view(alert: &StoredLoadAlert) -> Option<LoadAlertView> {
    Some(LoadAlertView {
        alert_id: alert.id.to_string(),
        rule: rule_of(&alert.rule)?,
        state: match alert.state.as_str() {
            "open" => AlertState::Open,
            "acknowledged" => AlertState::Acknowledged,
            "resolved" => AlertState::Resolved,
            "muted" => AlertState::Muted,
            _ => return None,
        },
        severity: match alert.severity.as_str() {
            "high" => TrendSeverity::High,
            "medium" => TrendSeverity::Medium,
            _ => TrendSeverity::Low,
        },
        muted_until_seconds: alert.muted_until.map(seconds),
        first_seen_seconds: seconds(alert.first_seen),
        last_seen_seconds: seconds(alert.last_seen),
        resolved_seconds: alert.resolved_at.map(seconds),
        detail: serde_json::from_str(&alert.facts_json).ok()?,
        engine_version: alert.engine_version.clone(),
        rule_version: alert.rule_version.clone(),
    })
}

/// The notification facts of one load alert: no reason text.
pub(crate) fn notify_fact(alert: &StoredLoadAlert) -> Option<doctor_notify::LoadAlertFact> {
    let view = alert_view(alert)?;
    let mut fact = doctor_notify::LoadAlertFact {
        alert_id: alert.id,
        severity: alert.severity.clone(),
        rule: alert.rule.clone(),
        subject: String::new(),
        state: None,
        baseline_ppm: None,
        current_ppm: None,
        window_count: None,
        first_lag: None,
        latest_lag: None,
        window_start: 0,
        window_end: 0,
        engine_version: alert.engine_version.clone(),
        rule_version: alert.rule_version.clone(),
    };
    match view.detail {
        LoadAlertDetail::RoutinePaused {
            name,
            database,
            table,
            observed_seconds,
            ..
        } => {
            fact.subject = format!("{name} · {database}.{table}");
            fact.window_start = i64::from(observed_seconds);
            fact.window_end = i64::from(observed_seconds);
        }
        LoadAlertDetail::RoutineStopped {
            name,
            database,
            table,
            state,
            observed_seconds,
            ..
        } => {
            fact.subject = format!("{name} · {database}.{table}");
            fact.state = Some(state);
            fact.window_start = i64::from(observed_seconds);
            fact.window_end = i64::from(observed_seconds);
        }
        LoadAlertDetail::LagGrowing {
            name,
            database,
            table,
            hours,
            ..
        } => {
            fact.subject = format!("{name} · {database}.{table}");
            fact.first_lag = hours.first().map(|hour| hour.max_lag.clone());
            fact.latest_lag = hours.last().map(|hour| hour.max_lag.clone());
            fact.window_start = hours
                .first()
                .map_or(0, |hour| i64::from(hour.start_seconds));
            fact.window_end = hours
                .last()
                .map_or(0, |hour| i64::from(hour.start_seconds) + HOUR);
        }
        LoadAlertDetail::FailureRate {
            database,
            table,
            kind,
            current,
            baseline,
            ..
        }
        | LoadAlertDetail::FilterRate {
            database,
            table,
            kind,
            current,
            baseline,
            ..
        } => {
            fact.subject = format!("{database}.{table} · {}", kind_label(kind));
            fact.baseline_ppm = Some(baseline.rate_ppm);
            fact.current_ppm = Some(current.rate_ppm);
            fact.window_count = Some(current.total);
            fact.window_start = i64::from(current.start_seconds);
            fact.window_end = i64::from(current.end_seconds);
        }
    }
    Some(fact)
}

/// Load targets first (task 58), then audit targets.
fn resolve_target(
    store: &HistoryStore,
    requested: Option<String>,
) -> doctor_history::Result<Option<String>> {
    let mut targets = store.load_targets()?;
    for target in store.targets()? {
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok(match requested {
        Some(target) => targets.into_iter().find(|known| *known == target),
        None => targets.into_iter().next(),
    })
}

pub(crate) fn thresholds() -> LoadAlertThresholds {
    LoadAlertThresholds {
        current_hours: CURRENT_HOURS as u16,
        baseline_hours: BASELINE_HOURS as u16,
        mute_hours: MUTE_HOURS as u16,
        lag_hours: LAG_RISES as u16,
        lag_min: LAG_MIN.to_string(),
        lag_high: LAG_HIGH.to_string(),
        failure_min_loads: FAILURE_RULE.min_total as u32,
        failure_min_baseline_loads: FAILURE_RULE.min_baseline_total as u32,
        failure_min_ppm: FAILURE_RULE.min_ppm,
        failure_high_ppm: FAILURE_RULE.high_ppm,
        filter_min_rows: FILTER_RULE.min_total as u32,
        filter_min_baseline_rows: FILTER_RULE.min_baseline_total as u32,
        filter_min_ppm: FILTER_RULE.min_ppm,
        filter_high_ppm: FILTER_RULE.high_ppm,
        calibrated: false,
    }
}

fn failure(state: &ApiState, status: StatusCode, code: &'static str) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        status,
        code,
        "history",
        code,
        "change_input",
    )
}

fn store_of(state: &ApiState) -> Result<Arc<HistoryStore>, ApiFailure> {
    state
        .history
        .store()
        .cloned()
        .ok_or_else(|| failure(state, StatusCode::CONFLICT, "history_disabled"))
}

pub(super) async fn alerts(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadAlertsRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_load_alerts_request",
        ));
    }
    let store = store_of(&state)?;
    let response =
        tokio::task::spawn_blocking(move || -> doctor_history::Result<LoadAlertsResponse> {
            let target = resolve_target(&store, request.target)?;
            let mut alerts = Vec::new();
            // Counted apart from the list limit; the list puts active ones first.
            let mut counts = [0u64; 4];
            if let Some(target) = target.as_deref() {
                counts = store.load_alert_counts(target)?;
                alerts.extend(
                    store
                        .load_alerts(
                            target,
                            LOAD_ALERT_LIST_LIMIT,
                            request
                                .more
                                .map(|more| (state_text(more.state), more.limit)),
                        )?
                        .iter()
                        .filter_map(alert_view),
                );
            }
            Ok(LoadAlertsResponse {
                target,
                alerts,
                list_limit: LOAD_ALERT_LIST_LIMIT,
                counts: LoadAlertCounts {
                    open: counts[0].to_string(),
                    acknowledged: counts[1].to_string(),
                    resolved: counts[2].to_string(),
                    muted: counts[3].to_string(),
                },
                thresholds: thresholds(),
                limitations: [
                    "rule_thresholds_pending_g2_calibration",
                    "unscored_and_not_comparable_raise_no_alert",
                    "lag_read_at_snapshots_only",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            })
        })
        .await
        .map_err(|_| internal_failure(&state))?
        .map_err(|_| internal_failure(&state))?;
    Ok(no_store(response))
}

pub(super) async fn action(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AlertActionRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_alert_action",
        ));
    }
    let store = store_of(&state)?;
    let id: i64 = request
        .alert_id
        .parse()
        .map_err(|_| failure(&state, StatusCode::BAD_REQUEST, "invalid_alert_action"))?;
    let action = match request.action {
        AlertAction::Acknowledge => "acknowledge",
        AlertAction::Mute => "mute",
        AlertAction::Reopen => "reopen",
    };
    match store
        .load_alert_action(id, action, unix_now(), MUTE_HOURS * 3600)
        .map_err(|_| internal_failure(&state))?
    {
        StoreChange::Applied => Ok(StatusCode::NO_CONTENT.into_response()),
        StoreChange::NotFound => Err(failure(&state, StatusCode::NOT_FOUND, "alert_not_found")),
        StoreChange::Rejected | StoreChange::ActiveItemExists => Err(failure(
            &state,
            StatusCode::CONFLICT,
            "alert_action_not_allowed",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_core::{CollectedLoad, aggregate_loads};
    use doctor_history::{RoutineSnapshotJob, RunRecord, RunState};

    #[test]
    fn audit_la_01_rate_rules_at_their_edges() {
        // Too few samples now or in the baseline: nothing is decided.
        assert_eq!(
            rate_decision(FAILURE_RULE, (19, 19), (1_000, 0)),
            Decision::Keep
        );
        assert_eq!(
            rate_decision(FAILURE_RULE, (100, 50), (99, 0)),
            Decision::Keep
        );
        // 5% now, 0 before: raised at the edge.
        assert_eq!(
            rate_decision(FAILURE_RULE, (100, 5), (1_000, 0)),
            Decision::Raise(TrendSeverity::Medium)
        );
        // Back to baseline + 2.5 points resolves; 4.9% would stay between.
        assert_eq!(
            rate_decision(FAILURE_RULE, (1_000, 25), (1_000, 0)),
            Decision::Resolve
        );
        // Not 3x the baseline: kept (between raise and resolve).
        assert_eq!(
            rate_decision(FAILURE_RULE, (100, 12), (1_000, 50)),
            Decision::Keep
        );
        assert_eq!(
            rate_decision(FAILURE_RULE, (100, 20), (1_000, 4)),
            Decision::Raise(TrendSeverity::High)
        );
        assert_eq!(
            rate_decision(FILTER_RULE, (10_000, 100), (100_000, 0)),
            Decision::Raise(TrendSeverity::Medium)
        );
        assert_eq!(
            rate_decision(FILTER_RULE, (10_000, 50), (100_000, 0)),
            Decision::Resolve
        );
    }

    #[test]
    fn audit_la_01_lag_rule_needs_rises_running_and_size() {
        let series = |values: [u128; 4]| values.map(Some);
        assert_eq!(
            lag_decision(&series([1_000, 5_000, 9_000, 12_000]), true),
            Decision::Raise(TrendSeverity::Medium)
        );
        assert_eq!(
            lag_decision(&series([100_000, 500_000, 900_000, 1_200_000]), true),
            Decision::Raise(TrendSeverity::High)
        );
        // Under 2x: keep; not running: no raise; a dip: resolve.
        assert_eq!(
            lag_decision(&series([10_000, 12_000, 15_000, 18_000]), true),
            Decision::Keep
        );
        assert_eq!(
            lag_decision(&series([1_000, 5_000, 9_000, 12_000]), false),
            Decision::Keep
        );
        assert_eq!(
            lag_decision(&series([1_000, 5_000, 20_000, 15_000]), true),
            Decision::Resolve
        );
        // A missing hour decides nothing: no raise, and no resolve either.
        assert_eq!(
            lag_decision(&[Some(1_000), None, Some(9_000), Some(12_000)], true),
            Decision::Keep
        );
        assert_eq!(
            lag_decision(&[None, Some(20_000), Some(18_000), Some(17_000)], true),
            Decision::Keep
        );
        assert_eq!(
            lag_decision(&[None, None, None, Some(9_000)], true),
            Decision::Keep
        );
        // Not running: no raise, and no resolve while it keeps growing.
        assert_eq!(
            lag_decision(&series([1_000, 5_000, 9_000, 30_000]), false),
            Decision::Keep
        );
    }

    #[test]
    fn audit_la_01_rates_compare_raw_counts_not_rounded_rates() {
        // 1/20 now against 1 in a million before: 4.9999 points, not 5.
        assert_eq!(
            rate_decision(FAILURE_RULE, (20, 1), (1_000_001, 1)),
            Decision::Keep
        );
        assert_eq!(
            rate_decision(FAILURE_RULE, (20, 1), (1_000_000, 0)),
            Decision::Raise(TrendSeverity::Medium)
        );
        let (delta, ratio) = rate_change((20, 1), (1_000_001, 1));
        assert_eq!(delta, 49_999);
        assert_eq!(ratio, Some(50_000_050));
        // A zero baseline has no ratio.
        assert_eq!(rate_change((100, 5), (1_000, 0)), (50_000, None));
        // Products too large to compare exactly decide nothing, instead of
        // saturating into a wrong answer.
        let big = 1_000_000_000_000_000_000u128;
        assert_eq!(
            rate_decision(FILTER_RULE, (big, big / 100), (big, 3 * big / 1_000)),
            Decision::Keep
        );
        assert_eq!(
            rate_change((big, big / 100), (big, 3 * big / 1_000)),
            (7_000, None)
        );
    }

    #[test]
    fn audit_la_01_lag_needs_complete_snapshots_under_one_version() {
        let store = HistoryStore::open_in_memory().unwrap();
        let end = 3_000 * HOUR;
        let job = |lag: u128| RoutineSnapshotJob {
            job_id: "5".into(),
            name: "clicks_rl".into(),
            database: "ods".into(),
            table: "clicks".into(),
            state: "RUNNING".into(),
            reason: None,
            max_lag: Some(lag),
            loaded_rows: Some(1),
            error_rows: Some(0),
            committed_tasks: Some(1),
            aborted_tasks: Some(0),
        };
        let rules = history_rule_version();
        let lags = [1_000, 5_000, 9_000, 12_000];
        let snap = |store: &HistoryStore, engines: [&str; 4]| {
            for (index, (lag, engine)) in lags.iter().zip(engines).enumerate() {
                let at = end - (4 - index as i64) * HOUR + 60;
                store
                    .record_routine_snapshot(
                        "t",
                        at,
                        900,
                        &[job(*lag)],
                        false,
                        Some((engine, rules.as_str())),
                    )
                    .unwrap();
            }
            store
                .record_routine_snapshot(
                    "t",
                    end + 60,
                    900,
                    &[job(12_500)],
                    false,
                    Some((engines[3], rules.as_str())),
                )
                .unwrap();
        };
        snap(&store, ["3.5.14"; 4]);
        let (raised, _) = evaluate_load_alerts(&store, "t", end + 120).unwrap();
        assert_eq!(raised.len(), 1);
        assert_eq!(
            store.load_alert(raised[0]).unwrap().unwrap().rule,
            "lag_growing"
        );
        // An upgrade inside the window: not compared, nothing raised.
        let upgraded = HistoryStore::open_in_memory().unwrap();
        snap(&upgraded, ["3.5.13", "3.5.13", "3.5.14", "3.5.14"]);
        assert!(
            evaluate_load_alerts(&upgraded, "t", end + 120)
                .unwrap()
                .0
                .is_empty()
        );
    }

    fn run(start: i64) -> RunRecord {
        RunRecord {
            started_at: start + HOUR,
            slice_start: start,
            slice_end: start + HOUR,
            state: RunState::Complete,
            reason: None,
            rows_read: 1,
            attributed: 1,
            partial_statements: 0,
            unavailable_statements: 0,
            duration_ms: 1,
        }
    }

    fn loads(hour: i64, count: u64, failed: u64) -> Vec<CollectedLoad> {
        (0..count)
            .map(|index| CollectedLoad {
                job_id: (hour as u64) * 1_000 + index,
                label: format!("l{index}"),
                database: "ods".into(),
                table: "orders".into(),
                kind: LoadHistoryKind::StreamLoad,
                cancelled: index < failed,
                system: false,
                finish_epoch_seconds: hour + 10,
                loaded_rows: Some(10),
                filtered_rows: Some(0),
                unselected_rows: Some(0),
                scan_bytes: None,
                total_seconds: Some(1),
                phases_ms: [None; 3],
                has_error_log: false,
            })
            .collect()
    }

    #[test]
    fn audit_la_01_failure_rate_raises_resolves_and_skips_partial_windows() {
        let store = HistoryStore::open_in_memory().unwrap();
        let now = 1_000 * HOUR + 600;
        let end = now - now.rem_euclid(HOUR);
        // 24 baseline hours with 10 loads each, no failures; then 3 current
        // hours with 10 loads, 3 failed each (30%).
        for index in 0..27 {
            let hour = end - (27 - index) * HOUR;
            let failed = if index >= 24 { 3 } else { 0 };
            store
                .record_load_slice(
                    "t",
                    "3.5.14",
                    &history_rule_version(),
                    &run(hour),
                    hour + HOUR,
                    &aggregate_loads(&loads(hour, 10, failed)),
                    &[],
                )
                .unwrap();
        }
        let (raised, _) = evaluate_load_alerts(&store, "t", now).unwrap();
        assert_eq!(raised.len(), 1);
        let alert = store.load_alert(raised[0]).unwrap().unwrap();
        assert_eq!(
            (alert.rule.as_str(), alert.severity.as_str()),
            ("failure_rate", "high")
        );
        assert_eq!(alert.subject, "ods.orders|Stream Load");
        let view = alert_view(&alert).unwrap();
        let LoadAlertDetail::FailureRate {
            current, baseline, ..
        } = view.detail
        else {
            panic!("failure detail")
        };
        assert_eq!(
            (
                current.total.as_str(),
                current.part.as_str(),
                current.rate_ppm
            ),
            ("30", "9", 300_000)
        );
        assert_eq!(
            (baseline.total.as_str(), baseline.collected_hours),
            ("240", 24)
        );
        let fact = notify_fact(&alert).unwrap();
        assert_eq!(fact.subject, "ods.orders · Stream Load");
        assert_eq!(fact.current_ppm, Some(300_000));
        // Re-evaluating raises nothing new.
        assert!(evaluate_load_alerts(&store, "t", now).unwrap().0.is_empty());
        // Three clean hours later the window is back near the baseline: resolved.
        for index in 0..3 {
            let hour = end + index * HOUR;
            store
                .record_load_slice(
                    "t",
                    "3.5.14",
                    &history_rule_version(),
                    &run(hour),
                    hour + HOUR,
                    &aggregate_loads(&loads(hour, 10, 0)),
                    &[],
                )
                .unwrap();
        }
        let (_, resolved) = evaluate_load_alerts(&store, "t", end + 3 * HOUR + 60).unwrap();
        assert_eq!(resolved, [raised[0]]);
        // A partly collected current window decides nothing.
        let other = HistoryStore::open_in_memory().unwrap();
        for index in 0..27 {
            let hour = end - (27 - index) * HOUR;
            let mut record = run(hour);
            if index == 26 {
                record.state = RunState::Partial;
            }
            other
                .record_load_slice(
                    "t",
                    "3.5.14",
                    &history_rule_version(),
                    &record,
                    hour + HOUR,
                    &aggregate_loads(&loads(hour, 10, if index >= 24 { 3 } else { 0 })),
                    &[],
                )
                .unwrap();
        }
        assert!(evaluate_load_alerts(&other, "t", now).unwrap().0.is_empty());
    }

    #[test]
    fn audit_la_01_routine_paused_stopped_and_gone() {
        let store = HistoryStore::open_in_memory().unwrap();
        let base = 2_000 * HOUR;
        let job = |state: &str, reason: Option<&str>| RoutineSnapshotJob {
            job_id: "7".into(),
            name: "events_rl".into(),
            database: "dwd".into(),
            table: "user_events".into(),
            state: state.into(),
            reason: reason.map(str::to_owned),
            max_lag: Some(42),
            loaded_rows: Some(10),
            error_rows: Some(0),
            committed_tasks: Some(1),
            aborted_tasks: Some(0),
        };
        store
            .record_routine_snapshot("t", base + 60, 900, &[job("RUNNING", None)], false, None)
            .unwrap();
        store
            .record_routine_snapshot(
                "t",
                base + 360,
                900,
                &[job("PAUSED", Some("too many errors"))],
                false,
                Some(("3.5.14-23a56ec", "rules.v9")),
            )
            .unwrap();
        let (raised, _) = evaluate_load_alerts(&store, "t", base + 400).unwrap();
        assert_eq!(raised.len(), 1);
        let paused = store.load_alert(raised[0]).unwrap().unwrap();
        assert_eq!(
            (paused.rule.as_str(), paused.severity.as_str()),
            ("routine_paused", "high")
        );
        // The version is the one the snapshot was taken under.
        assert_eq!(
            (paused.engine_version.as_str(), paused.rule_version.as_str()),
            ("3.5.14-23a56ec", "rules.v9")
        );
        let LoadAlertDetail::RoutinePaused { reason, .. } = alert_view(&paused).unwrap().detail
        else {
            panic!("paused")
        };
        assert_eq!(reason.as_deref(), Some("too many errors"));
        // Notifications never carry the reason text.
        let fact = notify_fact(&paused).unwrap();
        assert!(
            !serde_json::to_string(&fact)
                .unwrap()
                .contains("too many errors")
        );
        // Cancelled: the stop is raised; a pause that ends in a stop is not
        // "running again", so it is not resolved.
        store
            .record_routine_snapshot("t", base + 660, 900, &[job("CANCELLED", None)], false, None)
            .unwrap();
        let (raised, resolved) = evaluate_load_alerts(&store, "t", base + 700).unwrap();
        assert!(resolved.is_empty());
        assert_eq!(
            store.load_alert(raised[0]).unwrap().unwrap().rule,
            "routine_stopped"
        );
        // A truncated snapshot without the job proves nothing.
        store
            .record_routine_snapshot("t", base + 900, 900, &[], true, None)
            .unwrap();
        assert!(
            evaluate_load_alerts(&store, "t", base + 950)
                .unwrap()
                .1
                .is_empty()
        );
        // Gone from a complete snapshot within the same hour: both resolve.
        store
            .record_routine_snapshot("t", base + 1_200, 900, &[], false, None)
            .unwrap();
        let (_, mut resolved) = evaluate_load_alerts(&store, "t", base + 1_250).unwrap();
        resolved.sort_unstable();
        let mut expected = vec![paused.id, raised[0]];
        expected.sort_unstable();
        assert_eq!(resolved, expected);
        // A job first seen already cancelled raises nothing.
        let fresh = HistoryStore::open_in_memory().unwrap();
        fresh
            .record_routine_snapshot("t", base + 60, 900, &[job("CANCELLED", None)], false, None)
            .unwrap();
        assert!(
            evaluate_load_alerts(&fresh, "t", base + 100)
                .unwrap()
                .0
                .is_empty()
        );
    }
}
