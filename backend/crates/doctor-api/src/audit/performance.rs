//! Performance-overview response. Metric cells go through the sole core
//! normalizer; this layer only adds exact integer arithmetic for layout
//! (permyriad projections, axis maxima) and the Top-N bookkeeping the source
//! already ranked. It never parses units or invents a value for a gap.
use doctor_connector::{
    AUDIT_PERFORMANCE_SUMMARY_COLUMNS, AUDIT_PERFORMANCE_TOP_LIMIT,
    AuditPerformanceDimensionSource, AuditPerformanceSummarySource, AuditPerformanceWorkload,
    audit_performance_summary_cell as cell,
};
use doctor_contracts::{
    AuditFamilyExecutionDistribution, AuditMetricFact, AuditOptionalFactUnavailableReason,
    AuditPerformanceAggregate, AuditPerformanceAxes, AuditPerformanceBucket,
    AuditPerformanceDimension, AuditPerformanceDimensionId, AuditPerformanceDimensionRow,
    AuditPerformanceDimensionState, AuditPerformanceErrorRow, AuditPerformanceErrors,
    AuditPerformanceHistogram, AuditPerformanceHistogramBucket, AuditPerformanceOptionalQuantiles,
    AuditPerformanceProjection, AuditPerformanceQuantiles, AuditPerformanceRankEntry,
    AuditPerformanceRankKey, AuditPerformanceRanking, AuditPerformanceReadRequest,
    AuditPerformanceResponse, AuditPerformanceSlowCount, AuditPerformanceSummary,
    AuditPerformanceTickValue, AuditPerformanceValue, AuditPerformanceValueTick,
    AuditPerformanceVersion,
};
use doctor_core::{AuditFamilyDistributionSource, project_audit_family_distribution};

use super::*;

/// AuditLoader writes in batches; a bucket ending this close to the read may
/// still be filling.
const INGESTION_GRACE_SECONDS: u64 = 5 * 60;
/// Lower edge of the latency log axis: 1 ms, i.e. decade 0.
const LOG_AXIS_MIN_DECADES: u32 = 3;

pub(crate) async fn performance(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditPerformanceReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.scope)?;
    let window = request
        .scope
        .window
        .end_seconds
        .saturating_sub(request.scope.window.start_seconds);
    let bucket_seconds = request.bucket.resolve(window).map_err(|_| {
        ApiFailure::new(
            state.next_request_id(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "audit_performance_bucket",
            "audit_performance",
            "audit_performance_bucket",
            "change_input",
        )
    })?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let workload = sessions
                .audit_performance_controlled(
                    &connection_id,
                    request.scope,
                    bucket_seconds,
                    Some(control),
                )
                .map_err(|error| connector_failure(state, error))?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or(0);
            Ok(no_store(performance_response(state, workload, now)?))
        },
    )
    .await
}

fn number(state: &ApiState, value: &str) -> Result<u128, ApiFailure> {
    canonical_count(value)
        .and_then(|value| value.parse::<u128>().ok())
        .ok_or_else(|| invalid_response(state))
}

fn optional_number(state: &ApiState, value: &Option<String>) -> Result<Option<u128>, ApiFailure> {
    value
        .as_deref()
        .map(|value| number(state, value))
        .transpose()
}

/// Floor of `value / maximum` in ten-thousandths; 0 when the maximum is 0.
pub(crate) fn permyriad(value: u128, maximum: u128) -> u16 {
    if maximum == 0 {
        return 0;
    }
    u16::try_from(value.min(maximum).saturating_mul(10_000) / maximum).unwrap_or(10_000)
}

/// The smallest 1/2/5 x 10^k step that covers `maximum` in at most four steps.
fn axis_step(maximum: u128) -> u128 {
    let mut magnitude = 1u128;
    loop {
        for factor in [1u128, 2, 5] {
            let step = factor.saturating_mul(magnitude);
            if step.saturating_mul(4) >= maximum {
                return step;
            }
        }
        magnitude = magnitude.saturating_mul(10);
    }
}

/// Linear axis: an integer maximum and its evenly spaced integer ticks.
pub(crate) fn linear_axis(maximum: u128) -> (u128, Vec<u128>) {
    if maximum == 0 {
        return (0, vec![0]);
    }
    let step = axis_step(maximum);
    let top = maximum.div_ceil(step) * step;
    ((top), (0..=top / step).map(|index| index * step).collect())
}

fn log_decades(value: u128) -> u32 {
    let mut decades = 0;
    let mut bound = 1u128;
    while bound < value {
        bound = bound.saturating_mul(10);
        decades += 1;
    }
    decades
}

/// Position on the fixed log axis [1, 10^decades] in source units.
fn log_permyriad(value: u128, decades: u32) -> u16 {
    if value <= 1 || decades == 0 {
        return 0;
    }
    let position = (value as f64).log10() / f64::from(decades);
    (position.clamp(0.0, 1.0) * 10_000.0).floor() as u16
}

fn metric(id: AuditMetricId, cell: &Option<String>) -> AuditMetricFact {
    normalize_audit_metric(id, AuditMetricInput::from_sql_cell(cell.clone()))
}

fn quantiles(
    state: &ApiState,
    source: &AuditPerformanceSummarySource,
    id: AuditMetricId,
    eligible: usize,
    cells: [usize; 3],
) -> Result<AuditPerformanceQuantiles, ApiFailure> {
    let eligible_count = source.counter(eligible).to_owned();
    let present = eligible_count != "0";
    let quantile = |index: usize| -> Result<Option<AuditMetricFact>, ApiFailure> {
        match (&source.cells[index], present) {
            (None, false) => Ok(None),
            (Some(_), true) => Ok(Some(metric(id.clone(), &source.cells[index]))),
            _ => Err(invalid_response(state)),
        }
    };
    Ok(AuditPerformanceQuantiles {
        eligible_count,
        p50: quantile(cells[0])?,
        p95: quantile(cells[1])?,
        p99: quantile(cells[2])?,
    })
}

fn aggregate(
    state: &ApiState,
    source: &AuditPerformanceSummarySource,
    id: AuditMetricId,
    aggregation: AuditAggregation,
    nonnegative: usize,
    value: usize,
) -> Result<AuditPerformanceAggregate, ApiFailure> {
    let executions = number(state, source.counter(cell::EXECUTIONS))?;
    let included = number(state, source.counter(nonnegative))?;
    let excluded = executions
        .checked_sub(included)
        .ok_or_else(|| invalid_response(state))?;
    Ok(AuditPerformanceAggregate {
        aggregate: AuditAggregateFact {
            aggregation,
            metric: metric(id, &source.cells[value]),
            source_nonnegative_count: Some(source.counter(nonnegative).to_owned()),
        },
        excluded_count: excluded.to_string(),
    })
}

fn summary(
    state: &ApiState,
    source: &AuditPerformanceSummarySource,
    pending_time_column: bool,
) -> Result<AuditPerformanceSummary, ApiFailure> {
    let pending_time = if pending_time_column {
        AuditPerformanceOptionalQuantiles::Available {
            quantiles: Box::new(quantiles(
                state,
                source,
                AuditMetricId::PendingTime,
                cell::PENDING_TIME_ELIGIBLE,
                [
                    cell::PENDING_TIME_P50,
                    cell::PENDING_TIME_P95,
                    cell::PENDING_TIME_P99,
                ],
            )?),
            conflicting_count: source.counter(cell::PENDING_TIME_CONFLICTING).to_owned(),
        }
    } else {
        AuditPerformanceOptionalQuantiles::Unavailable {
            reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        }
    };
    let executions = number(state, source.counter(cell::EXECUTIONS))?;
    let failed = number(state, source.counter(cell::ERRORS))?;
    if failed > executions {
        return Err(invalid_response(state));
    }
    Ok(AuditPerformanceSummary {
        execution_count: source.counter(cell::EXECUTIONS).to_owned(),
        error_count: source.counter(cell::ERRORS).to_owned(),
        error_share_permyriad: (executions > 0).then(|| permyriad(failed, executions)),
        query_time: quantiles(
            state,
            source,
            AuditMetricId::QueryTime,
            cell::QUERY_TIME_ELIGIBLE,
            [
                cell::QUERY_TIME_P50,
                cell::QUERY_TIME_P95,
                cell::QUERY_TIME_P99,
            ],
        )?,
        query_time_sum: aggregate(
            state,
            source,
            AuditMetricId::QueryTime,
            AuditAggregation::Sum,
            cell::QUERY_TIME_NONNEGATIVE,
            cell::QUERY_TIME_SUM,
        )?,
        scan_bytes_sum: aggregate(
            state,
            source,
            AuditMetricId::ScanBytes,
            AuditAggregation::Sum,
            cell::SCAN_BYTES_NONNEGATIVE,
            cell::SCAN_BYTES_SUM,
        )?,
        cpu_cost_sum: aggregate(
            state,
            source,
            AuditMetricId::CpuCost,
            AuditAggregation::Sum,
            cell::CPU_COST_NONNEGATIVE,
            cell::CPU_COST_SUM,
        )?,
        memory_cost_max: aggregate(
            state,
            source,
            AuditMetricId::MemoryCost,
            AuditAggregation::Max,
            cell::MEMORY_COST_NONNEGATIVE,
            cell::MEMORY_COST_MAX,
        )?,
        pending_time,
    })
}

/// A bucket with no source row: an exact zero-execution count for this read.
fn empty_summary() -> AuditPerformanceSummarySource {
    let mut cells = vec![None; AUDIT_PERFORMANCE_SUMMARY_COLUMNS];
    for index in [
        cell::EXECUTIONS,
        cell::ERRORS,
        cell::QUERY_TIME_NONNEGATIVE,
        cell::QUERY_TIME_ELIGIBLE,
        cell::SCAN_BYTES_NONNEGATIVE,
        cell::CPU_COST_NONNEGATIVE,
        cell::MEMORY_COST_NONNEGATIVE,
        cell::PENDING_TIME_ELIGIBLE,
        cell::PENDING_TIME_CONFLICTING,
        cell::QUERY_TIME_SQL_NULL,
        cell::QUERY_TIME_NEGATIVE,
        cell::QUERY_TIME_OVERFLOW,
    ] {
        cells[index] = Some("0".into());
    }
    AuditPerformanceSummarySource { cells }
}

fn clock_label(seconds: u64, with_date: bool) -> String {
    match time::OffsetDateTime::from_unix_timestamp(seconds as i64) {
        Ok(at) if with_date => format!(
            "{:02}-{:02} {:02}:{:02}",
            u8::from(at.month()),
            at.day(),
            at.hour(),
            at.minute()
        ),
        Ok(at) => format!("{:02}:{:02}", at.hour(), at.minute()),
        Err(_) => seconds.to_string(),
    }
}

fn tick_values(
    id: Option<AuditMetricId>,
    values: Vec<u128>,
    top: u128,
) -> Vec<AuditPerformanceValueTick> {
    values
        .into_iter()
        .map(|value| AuditPerformanceValueTick {
            y_permyriad: permyriad(value, top),
            value: match &id {
                None => AuditPerformanceTickValue::Count {
                    count: value.to_string(),
                },
                Some(id) => AuditPerformanceTickValue::Metric {
                    metric: metric(id.clone(), &Some(value.to_string())),
                },
            },
        })
        .collect()
}

fn dimension_value(source: &AuditPerformanceDimensionSource) -> AuditPerformanceValue {
    match (&source.value, source.agrees) {
        (_, false) => AuditPerformanceValue::Conflicting,
        (None, true) => AuditPerformanceValue::SqlNull,
        (Some(value), true) => AuditPerformanceValue::Text {
            value: value.clone(),
        },
    }
}

fn ranking(
    state: &ApiState,
    rows: &[AuditPerformanceDimensionSource],
    key: AuditPerformanceRankKey,
    total: Option<u128>,
    distinct: u128,
) -> Result<AuditPerformanceRanking, ApiFailure> {
    let mut ranked = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let (rank, value) = match key {
            AuditPerformanceRankKey::ExecutionCount => (
                row.rank_by_count,
                Some(number(state, &row.execution_count)?),
            ),
            AuditPerformanceRankKey::QueryTimeSum => (
                row.rank_by_query_time,
                optional_number(state, &row.query_time_sum)?,
            ),
            AuditPerformanceRankKey::ErrorCount => {
                (row.rank_by_errors, Some(number(state, &row.error_count)?))
            }
        };
        match value {
            Some(value) if value > 0 && rank <= AUDIT_PERFORMANCE_TOP_LIMIT => {
                ranked.push((rank, index, value));
            }
            _ => {}
        }
    }
    ranked.sort_unstable();
    let widest = ranked.first().map(|entry| entry.2).unwrap_or(0);
    let listed = ranked
        .iter()
        .try_fold(0u128, |sum, entry| sum.checked_add(entry.2))
        .ok_or_else(|| invalid_response(state))?;
    let share = |value: u128| {
        total
            .filter(|total| *total > 0)
            .map(|total| permyriad(value, total))
    };
    let remainder_values = distinct
        .checked_sub(ranked.len() as u128)
        .ok_or_else(|| invalid_response(state))?;
    let remainder = match total {
        Some(total) => Some(
            total
                .checked_sub(listed)
                .ok_or_else(|| invalid_response(state))?,
        ),
        None => None,
    };
    Ok(AuditPerformanceRanking {
        key,
        entries: ranked
            .iter()
            .map(|(_, index, value)| {
                Ok(AuditPerformanceRankEntry {
                    row_index: u16::try_from(*index).map_err(|_| invalid_response(state))?,
                    share_permyriad: share(*value),
                    width_permyriad: permyriad(*value, widest),
                })
            })
            .collect::<Result<Vec<_>, ApiFailure>>()?,
        remainder_value_count: remainder_values.to_string(),
        remainder_share_permyriad: remainder.and_then(share),
    })
}

fn dimension(
    state: &ApiState,
    workload: &AuditPerformanceWorkload,
    id: AuditPerformanceDimensionId,
    totals: (u128, Option<u128>, u128),
) -> Result<AuditPerformanceDimension, ApiFailure> {
    if id == AuditPerformanceDimensionId::User && !workload.user_column {
        return Ok(AuditPerformanceDimension {
            dimension: id,
            state: AuditPerformanceDimensionState::Unavailable {
                reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
            },
        });
    }
    let mut rows = workload
        .source
        .dimensions
        .iter()
        .filter(|row| row.dimension == id)
        .cloned()
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.rank_by_count);
    let distinct = match workload.source.dimension_value_counts.get(&id) {
        Some(count) => number(state, count)?,
        None if totals.0 == 0 => 0,
        None => return Err(invalid_response(state)),
    };
    let (executions, query_time, errors) = totals;
    let rankings = vec![
        ranking(
            state,
            &rows,
            AuditPerformanceRankKey::ExecutionCount,
            Some(executions),
            distinct,
        )?,
        ranking(
            state,
            &rows,
            AuditPerformanceRankKey::QueryTimeSum,
            query_time,
            distinct,
        )?,
        ranking(
            state,
            &rows,
            AuditPerformanceRankKey::ErrorCount,
            Some(errors),
            distinct,
        )?,
    ];
    let rows = rows
        .iter()
        .map(|row| {
            let executions = number(state, &row.execution_count)?;
            let included = number(state, &row.query_time_nonnegative_count)?;
            Ok(AuditPerformanceDimensionRow {
                value: dimension_value(row),
                execution_count: row.execution_count.clone(),
                error_count: row.error_count.clone(),
                query_time_sum: AuditPerformanceAggregate {
                    aggregate: AuditAggregateFact {
                        aggregation: AuditAggregation::Sum,
                        metric: metric(AuditMetricId::QueryTime, &row.query_time_sum),
                        source_nonnegative_count: Some(row.query_time_nonnegative_count.clone()),
                    },
                    excluded_count: executions
                        .checked_sub(included)
                        .ok_or_else(|| invalid_response(state))?
                        .to_string(),
                },
            })
        })
        .collect::<Result<Vec<_>, ApiFailure>>()?;
    Ok(AuditPerformanceDimension {
        dimension: id,
        state: AuditPerformanceDimensionState::Available {
            distinct_value_count: distinct.to_string(),
            rows,
            rankings,
        },
    })
}

fn errors(
    state: &ApiState,
    workload: &AuditPerformanceWorkload,
    error_total: u128,
) -> Result<AuditPerformanceErrors, ApiFailure> {
    let error_count = error_total.to_string();
    let Some(source) = workload.source.errors.as_ref() else {
        return Ok(AuditPerformanceErrors::Unavailable {
            error_count,
            reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        });
    };
    let mut rows = source
        .iter()
        .map(|row| Ok((number(state, &row.count)?, row)))
        .collect::<Result<Vec<_>, ApiFailure>>()?;
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.agrees.cmp(&a.1.agrees))
            .then(a.1.code.is_none().cmp(&b.1.code.is_none()))
            .then(a.1.code.cmp(&b.1.code))
    });
    let listed = rows
        .iter()
        .try_fold(0u128, |sum, row| sum.checked_add(row.0))
        .ok_or_else(|| invalid_response(state))?;
    let codes = number(state, &workload.source.error_code_count)?;
    Ok(AuditPerformanceErrors::Available {
        error_count,
        rows: rows
            .iter()
            .map(|(count, row)| {
                Ok(AuditPerformanceErrorRow {
                    code: match (&row.code, row.agrees) {
                        (_, false) => AuditPerformanceValue::Conflicting,
                        (None, true) => AuditPerformanceValue::SqlNull,
                        (Some(value), true) => AuditPerformanceValue::Text {
                            value: value.clone(),
                        },
                    },
                    count: row.count.clone(),
                    share_permyriad: permyriad(*count, error_total),
                    last_event_milliseconds: number(state, &row.last_event_seconds)?
                        .checked_mul(1_000)
                        .ok_or_else(|| invalid_response(state))?
                        .to_string(),
                })
            })
            .collect::<Result<Vec<_>, ApiFailure>>()?,
        remainder_count: error_total
            .checked_sub(listed)
            .ok_or_else(|| invalid_response(state))?
            .to_string(),
        remainder_code_count: codes
            .checked_sub(rows.len() as u128)
            .ok_or_else(|| invalid_response(state))?
            .to_string(),
    })
}

fn histogram(
    state: &ApiState,
    workload: &AuditPerformanceWorkload,
) -> Result<AuditPerformanceHistogram, ApiFailure> {
    let cells = &workload.source.histogram;
    if cells.len() != 12 {
        return Err(invalid_response(state));
    }
    let projected = project_audit_family_distribution(&AuditFamilyDistributionSource {
        population_count: workload.source.window.counter(cell::EXECUTIONS).to_owned(),
        eligible_count: cells[11].clone(),
        sql_null_count: cells[8].clone(),
        negative_count: cells[9].clone(),
        overflow_count: cells[10].clone(),
        bucket_counts: cells[..8]
            .to_vec()
            .try_into()
            .map_err(|_| invalid_response(state))?,
    })
    .map_err(|_| invalid_response(state))?;
    let AuditFamilyExecutionDistribution::Exact {
        eligible_count,
        exclusions,
        buckets,
        ..
    } = projected;
    let eligible = number(state, &eligible_count)?;
    let counts = buckets
        .iter()
        .map(|bucket| number(state, &bucket.count))
        .collect::<Result<Vec<_>, _>>()?;
    let fullest = counts.iter().copied().max().unwrap_or(0);
    let slow = [3usize, 4, 5]
        .into_iter()
        .map(|from| {
            let count = counts[from..].iter().sum::<u128>();
            AuditPerformanceSlowCount {
                threshold_ms: buckets[from].lower_ms_inclusive.clone(),
                count: count.to_string(),
                share_permyriad: permyriad(count, eligible),
            }
        })
        .collect();
    Ok(AuditPerformanceHistogram {
        eligible_count,
        exclusions,
        buckets: buckets
            .into_iter()
            .zip(counts)
            .map(|(bucket, count)| AuditPerformanceHistogramBucket {
                bucket,
                share_permyriad: permyriad(count, eligible),
                width_permyriad: permyriad(count, fullest),
            })
            .collect(),
        slow,
    })
}

pub(crate) fn performance_response(
    state: &ApiState,
    workload: AuditPerformanceWorkload,
    now_milliseconds: u128,
) -> Result<AuditPerformanceResponse, ApiFailure> {
    let scope = workload.scope.clone();
    let start = u64::from(scope.window.start_seconds);
    let end = u64::from(scope.window.end_seconds);
    let width = u64::from(workload.bucket_seconds);
    let duration = end
        .checked_sub(start)
        .filter(|duration| *duration > 0)
        .ok_or_else(|| invalid_response(state))?;
    let window = summary(state, &workload.source.window, workload.pending_time_column)?;
    let empty = empty_summary();
    let mut sources = Vec::new();
    let mut at = start;
    let mut index = 0u32;
    while at < end {
        let bucket_end = (at + width).min(end);
        let source = workload.source.buckets.get(&index).unwrap_or(&empty);
        sources.push((at, bucket_end, source));
        at = bucket_end;
        index += 1;
    }
    let series_max = |cell_index: usize| -> Result<u128, ApiFailure> {
        sources.iter().try_fold(0u128, |max, (_, _, source)| {
            Ok(max.max(optional_number(state, &source.cells[cell_index])?.unwrap_or(0)))
        })
    };
    let (count_top, count_ticks) = linear_axis(series_max(cell::EXECUTIONS)?);
    let (scan_top, scan_ticks) = linear_axis(series_max(cell::SCAN_BYTES_SUM)?);
    let (cpu_top, cpu_ticks) = linear_axis(series_max(cell::CPU_COST_SUM)?);
    let (memory_top, memory_ticks) = linear_axis(series_max(cell::MEMORY_COST_MAX)?);
    let (pending_top, pending_ticks) = linear_axis(series_max(cell::PENDING_TIME_P95)?);
    let decades = log_decades(series_max(cell::QUERY_TIME_P99)?).max(LOG_AXIS_MIN_DECADES);
    let now_seconds = u64::try_from(now_milliseconds / 1_000).unwrap_or(u64::MAX);
    let mut buckets = Vec::with_capacity(sources.len());
    for (bucket_start, bucket_end, source) in &sources {
        let value = |index: usize| optional_number(state, &source.cells[index]);
        let linear = |index: usize, top: u128| -> Result<Option<u16>, ApiFailure> {
            Ok(value(index)?.map(|value| permyriad(value, top)))
        };
        let log = |index: usize| -> Result<Option<u16>, ApiFailure> {
            Ok(value(index)?.map(|value| log_permyriad(value, decades)))
        };
        buckets.push(AuditPerformanceBucket {
            start_milliseconds: (u128::from(*bucket_start) * 1_000).to_string(),
            end_milliseconds: (u128::from(*bucket_end) * 1_000).to_string(),
            x_permyriad: permyriad(u128::from(bucket_start - start), u128::from(duration)),
            width_permyriad: permyriad(u128::from(bucket_end - bucket_start), u128::from(duration)),
            summary: summary(state, source, workload.pending_time_column)?,
            projection: AuditPerformanceProjection {
                execution_permyriad: permyriad(
                    number(state, source.counter(cell::EXECUTIONS))?,
                    count_top,
                ),
                error_permyriad: permyriad(number(state, source.counter(cell::ERRORS))?, count_top),
                query_time_p50_permyriad: log(cell::QUERY_TIME_P50)?,
                query_time_p95_permyriad: log(cell::QUERY_TIME_P95)?,
                query_time_p99_permyriad: log(cell::QUERY_TIME_P99)?,
                scan_bytes_permyriad: linear(cell::SCAN_BYTES_SUM, scan_top)?,
                cpu_cost_permyriad: linear(cell::CPU_COST_SUM, cpu_top)?,
                memory_cost_permyriad: linear(cell::MEMORY_COST_MAX, memory_top)?,
                pending_time_p95_permyriad: linear(cell::PENDING_TIME_P95, pending_top)?,
            },
            recent_ingestion: bucket_end.saturating_add(INGESTION_GRACE_SECONDS) > now_seconds,
        });
    }
    let with_date = duration >= 24 * 60 * 60;
    let time_ticks = (0..=4u64)
        .map(|index| {
            let at = start + duration * index / 4;
            AuditConcurrencyTimeTick {
                at_milliseconds: (u128::from(at) * 1_000).to_string(),
                x_permyriad: (index * 2_500) as u16,
                label: clock_label(at, with_date),
            }
        })
        .collect();
    let log_ticks = (0..=decades)
        .map(|decade| AuditPerformanceValueTick {
            y_permyriad: permyriad(u128::from(decade), u128::from(decades)),
            value: AuditPerformanceTickValue::Metric {
                metric: metric(
                    AuditMetricId::QueryTime,
                    &Some(10u128.pow(decade).to_string()),
                ),
            },
        })
        .collect();
    let executions = number(state, &window.execution_count)?;
    let error_total = number(state, &window.error_count)?;
    let query_time_total =
        optional_number(state, &workload.source.window.cells[cell::QUERY_TIME_SUM])?;
    let dimensions = [
        AuditPerformanceDimensionId::User,
        AuditPerformanceDimensionId::Database,
        AuditPerformanceDimensionId::ResourceGroup,
        AuditPerformanceDimensionId::Catalog,
    ]
    .into_iter()
    .map(|id| {
        dimension(
            state,
            &workload,
            id,
            (executions, query_time_total, error_total),
        )
    })
    .collect::<Result<Vec<_>, ApiFailure>>()?;
    Ok(AuditPerformanceResponse {
        schema_version: AuditPerformanceVersion::V1,
        connection_id: workload.session_identity.connection_id.clone(),
        scope,
        bucket_seconds: workload.bucket_seconds,
        read_at_milliseconds: now_milliseconds.to_string(),
        axes: AuditPerformanceAxes {
            time_ticks,
            execution: tick_values(None, count_ticks, count_top),
            query_time_log: log_ticks,
            scan_bytes: tick_values(Some(AuditMetricId::ScanBytes), scan_ticks, scan_top),
            cpu_cost: tick_values(Some(AuditMetricId::CpuCost), cpu_ticks, cpu_top),
            memory_cost: tick_values(Some(AuditMetricId::MemoryCost), memory_ticks, memory_top),
            pending_time: tick_values(Some(AuditMetricId::PendingTime), pending_ticks, pending_top),
        },
        latency_histogram: histogram(state, &workload)?,
        errors: errors(state, &workload, error_total)?,
        dimensions,
        window,
        buckets,
        cluster_identity: public_identity(&workload.session_identity),
        audit_timestamp_time_zone: public_time_zone(&workload.session_identity),
        limitations: [
            "starrocks_percentile_disc_returns_the_sorted_value_at_index_ceil_p_n_minus_1",
            "bucket_quantiles_are_never_combined_across_buckets",
            "empty_buckets_are_exact_zero_counts_for_this_read",
            "audit_loader_ingestion_async_completeness_unknown",
            "cpu_cost_sum_is_not_elapsed_time",
            "memory_cost_max_is_one_execution_not_concurrent_total",
            "dimension_rankings_are_exact_top_n_per_key",
            "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_axes_use_whole_1_2_5_steps() {
        assert_eq!(linear_axis(0), (0, vec![0]));
        assert_eq!(linear_axis(7), (8, vec![0, 2, 4, 6, 8]));
        assert_eq!(
            linear_axis(3_470),
            (4_000, vec![0, 1_000, 2_000, 3_000, 4_000])
        );
        assert_eq!(linear_axis(10), (10, vec![0, 5, 10]));
    }

    #[test]
    fn log_axis_starts_at_one_unit_and_covers_the_largest_value() {
        assert_eq!(log_decades(1_000), 3);
        assert_eq!(log_decades(1_001), 4);
        assert_eq!(log_permyriad(1, 4), 0);
        assert_eq!(log_permyriad(100, 4), 5_000);
        assert_eq!(log_permyriad(10_000, 4), 10_000);
    }

    #[test]
    fn permyriad_is_floored_and_never_divides_by_zero() {
        assert_eq!(permyriad(1, 3), 3_333);
        assert_eq!(permyriad(5, 0), 0);
        assert_eq!(permyriad(9, 9), 10_000);
    }

    #[test]
    fn clock_labels_follow_the_window_length() {
        assert_eq!(clock_label(1_800_000_000, true), "01-15 08:00");
        assert_eq!(clock_label(1_800_000_000, false), "08:00");
    }
}
