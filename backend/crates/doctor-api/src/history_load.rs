//! O2: the 30-day load view over the local history store. Only additive
//! quantities are read (executions, failures, summed query time); hours that
//! were not collected stay absent, and a StarRocks version or history rule
//! change starts a new segment that is never compared across.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    AuditMetricId, AuditPerformanceTickValue, AuditPerformanceValueTick, CoverageHourState,
    HistoryLoadAxes, HistoryLoadComparison, HistoryLoadCoverage, HistoryLoadDay,
    HistoryLoadHeatCell, HistoryLoadNotComparableReason, HistoryLoadRequest, HistoryLoadResponse,
    HistoryLoadSegment, HistoryLoadWeekRow,
};
use doctor_core::HISTORY_BUCKET_SECONDS;
use doctor_history::HistoryStore;

use super::*;
use crate::audit::no_store;
use crate::audit::performance::{linear_axis, permyriad};
use crate::history::unix_now;
use crate::history_trend::{coverage_map, metric_fact, seconds};

const DAY: i64 = 86_400;

pub(super) async fn load(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: HistoryLoadRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_history_load_request",
            "history",
            "invalid_history_load_request",
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
    let connected = request
        .connection_id
        .as_deref()
        .and_then(|id| state.history.entry_target(id));
    let worker = tokio::task::spawn_blocking(move || {
        let target = match connected {
            Some(target) => Some(target),
            None => store.targets()?.into_iter().next(),
        };
        build(&store, target, request.days, unix_now())
    });
    let response = worker
        .await
        .map_err(|_| internal_failure(&state))?
        .map_err(|_| internal_failure(&state))?;
    Ok(no_store(response))
}

#[derive(Default)]
struct Hour {
    executions: u128,
    errors: u128,
    query_time: u128,
    versions: BTreeSet<(String, String)>,
}

/// 1970-01-01 was a Thursday; 0 = Monday.
fn weekday(day_start: i64) -> u8 {
    ((day_start.div_euclid(DAY) + 3).rem_euclid(7)) as u8
}

fn count_ticks(values: Vec<u128>, top: u128) -> Vec<AuditPerformanceValueTick> {
    values
        .into_iter()
        .map(|value| AuditPerformanceValueTick {
            y_permyriad: permyriad(value, top),
            value: AuditPerformanceTickValue::Count {
                count: value.to_string(),
            },
        })
        .collect()
}

fn duration_ticks(values: Vec<u128>, top: u128) -> Vec<AuditPerformanceValueTick> {
    values
        .into_iter()
        .map(|value| AuditPerformanceValueTick {
            y_permyriad: permyriad(value, top),
            value: AuditPerformanceTickValue::Metric {
                metric: metric_fact(
                    AuditMetricId::QueryTime,
                    u64::try_from(value).unwrap_or(u64::MAX),
                ),
            },
        })
        .collect()
}

/// Rounded half up; `None` without a divisor.
fn rounded_average(sum: u128, count: u128) -> Option<u128> {
    (count > 0).then(|| (sum * 2 + count) / (count * 2))
}

pub(crate) fn build(
    store: &HistoryStore,
    target: Option<String>,
    days: u16,
    now: i64,
) -> doctor_history::Result<HistoryLoadResponse> {
    let today = now - now.rem_euclid(DAY);
    let start = today - (i64::from(days) - 1) * DAY;
    let end = today + DAY;
    // Hours after the current one have not happened yet: neither collected
    // nor missing, so they are left out of every count.
    let elapsed_end =
        (now - now.rem_euclid(HISTORY_BUCKET_SECONDS) + HISTORY_BUCKET_SECONDS).min(end);
    let (runs, totals) = match target.as_deref() {
        Some(target) => (
            store.runs(target, start, end, u32::MAX)?,
            store.hourly_totals(target, start, end)?,
        ),
        None => (Vec::new(), Vec::new()),
    };
    let coverage = coverage_map(&runs, start, elapsed_end);
    let mut hours: BTreeMap<i64, Hour> = BTreeMap::new();
    for total in totals {
        let hour = hours.entry(total.bucket_start).or_default();
        hour.executions += u128::from(total.executions);
        hour.errors += u128::from(total.errors);
        hour.query_time += total.query_time_sum;
        hour.versions
            .insert((total.engine_version, total.rule_version));
    }

    struct DayFacts {
        start: i64,
        coverage: HistoryLoadCoverage,
        complete: u16,
        partial: u16,
        missing: u16,
        sums: Option<(u128, u128, u128)>,
        versions: BTreeSet<(String, String)>,
    }
    let mut day_facts = Vec::new();
    for index in 0..i64::from(days) {
        let day_start = start + index * DAY;
        let (mut complete, mut partial, mut missing) = (0u16, 0u16, 0u16);
        let mut sums = (0u128, 0u128, 0u128);
        let mut versions = BTreeSet::new();
        for hour_start in (day_start..day_start + DAY).step_by(HISTORY_BUCKET_SECONDS as usize) {
            match coverage.get(&hour_start) {
                Some(CoverageHourState::Complete) => complete += 1,
                Some(CoverageHourState::Partial) => partial += 1,
                Some(CoverageHourState::NotCollected) => missing += 1,
                None => {}
            }
            if let Some(hour) = hours.get(&hour_start) {
                sums.0 += hour.executions;
                sums.1 += hour.errors;
                sums.2 += hour.query_time;
                versions.extend(hour.versions.iter().cloned());
            }
        }
        let read = complete + partial;
        let finished = day_start + DAY <= now;
        let state = if read == 0 && versions.is_empty() {
            HistoryLoadCoverage::NotCollected
        } else if partial == 0 && missing == 0 && finished {
            HistoryLoadCoverage::Complete
        } else {
            HistoryLoadCoverage::Partial
        };
        day_facts.push(DayFacts {
            start: day_start,
            coverage: state,
            complete,
            partial,
            missing,
            sums: (state != HistoryLoadCoverage::NotCollected).then_some(sums),
            versions,
        });
    }

    // A day under one version pair continues the segment of that pair; a day
    // with another pair, or with several, starts a new one. Days without data
    // inherit the running segment.
    let mut segments: Vec<HistoryLoadSegment> = Vec::new();
    let mut segment_of = Vec::with_capacity(day_facts.len());
    let mut current: Option<BTreeSet<(String, String)>> = None;
    for day in &day_facts {
        if !day.versions.is_empty() && current.as_ref() != Some(&day.versions) {
            segments.push(HistoryLoadSegment {
                engine_versions: day
                    .versions
                    .iter()
                    .map(|pair| pair.0.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                rule_versions: day
                    .versions
                    .iter()
                    .map(|pair| pair.1.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                first_day_seconds: seconds(day.start),
            });
            current = Some(day.versions.clone());
        }
        segment_of.push(segments.len().saturating_sub(1) as u16);
    }

    let max_of = |pick: fn(&(u128, u128, u128)) -> u128| {
        day_facts
            .iter()
            .filter_map(|day| day.sums.as_ref().map(pick))
            .max()
            .unwrap_or(0)
    };
    let (executions_top, executions_ticks) = linear_axis(max_of(|sums| sums.0));
    let (errors_top, errors_ticks) = linear_axis(max_of(|sums| sums.1));
    let (time_top, time_ticks) = linear_axis(max_of(|sums| sums.2));
    let days_out = day_facts
        .iter()
        .zip(&segment_of)
        .map(|(day, segment)| HistoryLoadDay {
            start_seconds: seconds(day.start),
            coverage: day.coverage,
            complete_hours: day.complete,
            partial_hours: day.partial,
            not_collected_hours: day.missing,
            executions: day.sums.map(|sums| sums.0.to_string()),
            errors: day.sums.map(|sums| sums.1.to_string()),
            query_time_sum: day.sums.map(|sums| {
                metric_fact(
                    AuditMetricId::QueryTime,
                    u64::try_from(sums.2).unwrap_or(u64::MAX),
                )
            }),
            segment: *segment,
            executions_permyriad: day.sums.map(|sums| permyriad(sums.0, executions_top)),
            errors_permyriad: day.sums.map(|sums| permyriad(sums.1, errors_top)),
            query_time_permyriad: day.sums.map(|sums| permyriad(sums.2, time_top)),
        })
        .collect::<Vec<_>>();

    // Only completely read hours enter an average: a partly read hour would
    // pull it down without saying so.
    let mut cells = [[(0u128, 0u128); 24]; 7];
    for (hour_start, state) in &coverage {
        if *state != CoverageHourState::Complete {
            continue;
        }
        let day_start = hour_start - hour_start.rem_euclid(DAY);
        let cell = &mut cells[usize::from(weekday(day_start))]
            [(hour_start.rem_euclid(DAY) / HISTORY_BUCKET_SECONDS) as usize];
        cell.0 += hours
            .get(hour_start)
            .map(|hour| hour.executions)
            .unwrap_or(0);
        cell.1 += 1;
    }
    let averages = cells.map(|row| row.map(|(sum, count)| (rounded_average(sum, count), count)));
    let busiest = averages
        .iter()
        .flatten()
        .filter_map(|(average, _)| *average)
        .max()
        .unwrap_or(0);
    let mut heatmap = Vec::with_capacity(7 * 24);
    for (weekday, row) in averages.iter().enumerate() {
        for (hour, (average, count)) in row.iter().enumerate() {
            heatmap.push(HistoryLoadHeatCell {
                weekday: weekday as u8,
                hour: hour as u8,
                collected_hours: *count as u16,
                average_executions: average.map(|value| value.to_string()),
                level: average.map(|value| {
                    (value * 5)
                        .checked_div(busiest)
                        .map_or(0, |level| u8::try_from(level.min(5)).unwrap_or(5))
                }),
            });
        }
    }

    let index_of = |day_start: i64| usize::try_from((day_start - start) / DAY).ok();
    let mut week = Vec::new();
    for back in (0..7).rev() {
        let current_day = today - back * DAY;
        let previous_day = current_day - 7 * DAY;
        let (Some(current_index), Some(previous_index)) =
            (index_of(current_day), index_of(previous_day))
        else {
            continue;
        };
        if previous_day < start {
            continue;
        }
        let (current, previous) = (&day_facts[current_index], &day_facts[previous_index]);
        let comparison = if current.coverage != HistoryLoadCoverage::Complete
            || previous.coverage != HistoryLoadCoverage::Complete
        {
            HistoryLoadComparison::NotComparable {
                reason: HistoryLoadNotComparableReason::PartialCoverage,
            }
        } else if segment_of[current_index] != segment_of[previous_index] {
            HistoryLoadComparison::NotComparable {
                reason: HistoryLoadNotComparableReason::VersionOrRuleChanged,
            }
        } else {
            let now_value = current.sums.map(|sums| sums.0 as i128).unwrap_or(0);
            let before = previous.sums.map(|sums| sums.0 as i128).unwrap_or(0);
            HistoryLoadComparison::Exact {
                delta: (now_value - before).to_string(),
                permille: (before > 0).then(|| {
                    i32::try_from((now_value - before) * 1000 / before).unwrap_or(i32::MAX)
                }),
            }
        };
        week.push(HistoryLoadWeekRow {
            current_day_seconds: seconds(current_day),
            previous_day_seconds: seconds(previous_day),
            current_executions: current.sums.map(|sums| sums.0.to_string()),
            previous_executions: previous.sums.map(|sums| sums.0.to_string()),
            comparison,
        });
    }

    let collected = coverage
        .values()
        .filter(|state| **state != CoverageHourState::NotCollected)
        .count();
    let collected_days = day_facts.iter().filter(|day| day.sums.is_some());
    let totals = collected_days.fold(None, |sum: Option<(u128, u128)>, day| {
        let (executions, errors, _) = day.sums.unwrap_or_default();
        let (total_executions, total_errors) = sum.unwrap_or_default();
        Some((total_executions + executions, total_errors + errors))
    });
    Ok(HistoryLoadResponse {
        target,
        start_seconds: seconds(start),
        end_seconds: seconds(end),
        days: days_out,
        segments,
        heatmap,
        week,
        axes: HistoryLoadAxes {
            executions: count_ticks(executions_ticks, executions_top),
            errors: count_ticks(errors_ticks, errors_top),
            query_time: duration_ticks(time_ticks, time_top),
        },
        collected_hours: collected as u16,
        expected_hours: coverage.len() as u16,
        rows_read: runs
            .iter()
            .map(|run| u128::from(run.rows_read))
            .sum::<u128>()
            .to_string(),
        rows_attributed: runs
            .iter()
            .map(|run| u128::from(run.attributed))
            .sum::<u128>()
            .to_string(),
        executions: totals.map(|sums| sums.0.to_string()),
        errors: totals.map(|sums| sums.1.to_string()),
        limitations: [
            "only_executions_attributed_to_a_template_are_stored",
            "quantiles_are_not_combined_from_hourly_aggregates",
            "hours_not_collected_are_never_interpolated",
            "version_or_rule_changes_split_comparisons",
            "weekday_and_hour_are_utc",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_history::{RunRecord, RunState};

    fn run(store: &HistoryStore, start: i64, end: i64, state: RunState) {
        store
            .record_run(
                "t",
                &RunRecord {
                    started_at: end,
                    slice_start: start,
                    slice_end: end,
                    state,
                    reason: None,
                    rows_read: 10,
                    attributed: 8,
                    partial_statements: 0,
                    unavailable_statements: 0,
                    duration_ms: 1,
                },
                Some(end),
            )
            .unwrap();
    }

    #[test]
    fn weekdays_count_from_monday_in_utc() {
        assert_eq!(weekday(0), 3);
        // 2026-10-26 is a Monday.
        assert_eq!(weekday(1_792_972_800), 0);
    }

    #[test]
    fn an_empty_target_draws_nothing_and_claims_no_zero() {
        let store = HistoryStore::open_in_memory().unwrap();
        let now = 1_792_972_800 + 10 * 3_600 + 5;
        let response = build(&store, None, 14, now).unwrap();
        assert_eq!(response.days.len(), 14);
        assert!(response.days.iter().all(
            |day| day.coverage == HistoryLoadCoverage::NotCollected && day.executions.is_none()
        ));
        assert!(
            response
                .heatmap
                .iter()
                .all(|cell| cell.average_executions.is_none())
        );
        assert!(
            response
                .week
                .iter()
                .all(|row| matches!(row.comparison, HistoryLoadComparison::NotComparable { .. }))
        );
        assert_eq!(response.executions, None);
        // Only elapsed hours are expected: 13 whole days plus today's 11 hours.
        assert_eq!(response.expected_hours, 13 * 24 + 11);
    }

    #[test]
    fn collected_days_sum_hours_and_a_partly_read_day_is_never_compared() {
        let store = HistoryStore::open_in_memory().unwrap();
        let monday = 1_792_972_800;
        let now = monday + 7 * DAY + 3_600 * 23 + 1;
        // Last Monday fully read with no rows; this Monday read except one hour.
        run(&store, monday, monday + DAY, RunState::Complete);
        run(
            &store,
            monday + 7 * DAY,
            monday + 7 * DAY + 23 * 3_600,
            RunState::Complete,
        );
        let response = build(&store, Some("t".into()), 14, now).unwrap();
        let last_monday = response
            .days
            .iter()
            .find(|day| i64::from(day.start_seconds) == monday)
            .unwrap();
        assert_eq!(last_monday.coverage, HistoryLoadCoverage::Complete);
        // Read with no rows is a real 0, unlike a day that was never read.
        assert_eq!(last_monday.executions.as_deref(), Some("0"));
        let row = response
            .week
            .iter()
            .find(|row| i64::from(row.current_day_seconds) == monday + 7 * DAY)
            .unwrap();
        assert_eq!(
            row.comparison,
            HistoryLoadComparison::NotComparable {
                reason: HistoryLoadNotComparableReason::PartialCoverage
            }
        );
        let cell = response
            .heatmap
            .iter()
            .find(|cell| cell.weekday == 0 && cell.hour == 5)
            .unwrap();
        assert_eq!(cell.collected_hours, 2);
        assert_eq!(cell.average_executions.as_deref(), Some("0"));
        assert_eq!(response.rows_read, "20");
    }

    #[test]
    fn rounded_average_is_half_up() {
        assert_eq!(rounded_average(5, 2), Some(3));
        assert_eq!(rounded_average(4, 3), Some(1));
        assert_eq!(rounded_average(1, 0), None);
    }
}
