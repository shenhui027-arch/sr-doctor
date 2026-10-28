//! Stage D (ADR-6, ADR-7): in-app alerts from stage C regressions and the
//! governance queue. Only the local history is read; StarRocks is never
//! changed and no credential is needed. `closed` is reached only through a
//! comparator result; `pending_verification` only by recording a change.
use serde::{Deserialize, Serialize};

use doctor_contracts::{
    AlertAction, AlertActionRequest, AlertChannel, AlertCounts, AlertRule, AlertState, AlertView,
    AuditMetricId, EvidenceCompleteness, GovernanceChangeRequest, GovernanceCreateRequest,
    GovernanceEvent, GovernanceItem, GovernanceItemRequest, GovernanceListRequest,
    GovernanceListResponse, GovernanceOrigin, GovernanceSnapshot, GovernanceState,
    GovernanceTransitionRequest, GovernanceVerification, HistoryAlertsRequest,
    HistoryAlertsResponse, TrendChange, TrendComparability, TrendConfidence, TrendDirection,
    TrendSeverity, TrendVerdict, VerificationMetric, VerificationOutcome, WorkloadSignal,
    governance_manual_transitions,
};
use doctor_core::{
    ChangeVerdict, HISTORY_BUCKET_SECONDS, TREND_MIN_CURRENT_SAMPLES, TrendWindows, WindowStats,
    verify_change, window_stats,
};
use doctor_history::{AlertFacts, HistoryStore, ItemUpdate, StoreChange, StoredAlert, StoredItem};

use super::*;
use crate::audit::no_store;
use crate::history::unix_now;
use crate::history_trend::{evaluate, metric_fact, percent_text, seconds};

/// ADR-6 windows: alerts look at the most recent hours so a regression is
/// raised while it happens; the baseline is the week before.
pub(crate) const ALERT_CURRENT_HOURS: u16 = 3;
/// The "before" a governance item pins: the same recent window an alert
/// judges, so it is the state being fixed, not a mix with healthy hours.
pub(crate) const PIN_HOURS: u16 = ALERT_CURRENT_HOURS;
pub(crate) const ALERT_BASELINE_HOURS: u16 = 168;
pub(crate) const ALERT_MUTE_HOURS: u16 = 24;
const ACTOR_USER: &str = "local_user";
const ACTOR_COMPARATOR: &str = "comparator";

fn severity_rank(severity: TrendSeverity) -> u8 {
    match severity {
        TrendSeverity::Low => 0,
        TrendSeverity::Medium => 1,
        TrendSeverity::High => 2,
    }
}

fn text_of<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// Evaluates the stage C verdicts of one target into alerts: regressions of
/// at least medium severity raise or refresh one alert per template;
/// templates back inside the band resolve theirs; unscored and not
/// comparable verdicts change nothing.
pub(crate) fn evaluate_alerts(
    store: &HistoryStore,
    target: &str,
    now: i64,
) -> doctor_history::Result<()> {
    let windows = TrendWindows::ending_at(now, ALERT_CURRENT_HOURS, ALERT_BASELINE_HOURS);
    for (template, trend) in evaluate(store, target, windows, windows.current_start)?.trends {
        let key = &template.template.template_key;
        match trend.verdict {
            TrendVerdict::Regression { severity, workload }
                if severity_rank(severity) >= severity_rank(TrendSeverity::Medium) =>
            {
                let exemplar = store
                    .exemplars(key, windows.current_start, windows.end, 1)?
                    .into_iter()
                    .next()
                    .map(|e| e.query_id);
                let bucket_start = windows.end - HISTORY_BUCKET_SECONDS;
                let facts = AlertFacts {
                    severity: text_of(&severity),
                    confidence: text_of(&trend.confidence.unwrap_or(TrendConfidence::Low)),
                    evidence: text_of(&trend.evidence),
                    workload: text_of(&workload),
                    baseline_median: trend.baseline.median,
                    current_median: trend.current.median,
                    relative_permille: trend
                        .change
                        .as_ref()
                        .and_then(|change| change.relative_permille),
                    bucket_start,
                    exemplar,
                    engine_version: trend.engine_version.clone(),
                    rule_version: trend.rule_version.clone(),
                };
                store.upsert_alert(target, key, &facts, now)?;
            }
            TrendVerdict::Stable | TrendVerdict::Improvement => {
                store.resolve_alerts(target, key, now)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn change_between(
    before: Option<u64>,
    after: Option<u64>,
    metric: AuditMetricId,
) -> Option<TrendChange> {
    let (before, after) = (before?, after?);
    Some(TrendChange {
        direction: match after.cmp(&before) {
            std::cmp::Ordering::Greater => TrendDirection::Up,
            std::cmp::Ordering::Less => TrendDirection::Down,
            std::cmp::Ordering::Equal => TrendDirection::None,
        },
        magnitude: metric_fact(metric, after.abs_diff(before)),
        relative_percent: (before > 0).then(|| {
            percent_text(
                ((i128::from(after) - i128::from(before)) * 1000 / i128::from(before)) as i64,
            )
        }),
    })
}

fn alert_view(
    alert: StoredAlert,
    template: Option<&doctor_history::StoredTemplate>,
) -> Option<AlertView> {
    let f = &alert.facts;
    Some(AlertView {
        alert_id: alert.id.to_string(),
        target: alert.target,
        template_key: alert.template_key,
        template: template.and_then(|t| t.template.template.clone()),
        kind: template
            .map(|t| t.template.kind.as_str().to_owned())
            .unwrap_or_default(),
        state: parse(&alert.state)?,
        muted_until_seconds: alert.muted_until.map(seconds),
        first_seen_seconds: seconds(alert.first_seen),
        last_seen_seconds: seconds(alert.last_seen),
        resolved_seconds: alert.resolved_at.map(seconds),
        severity: parse(&f.severity)?,
        confidence: parse(&f.confidence)?,
        evidence: parse(&f.evidence)?,
        workload: parse::<WorkloadSignal>(&f.workload)?,
        baseline_median: f
            .baseline_median
            .map(|v| metric_fact(AuditMetricId::QueryTime, v)),
        current_median: f
            .current_median
            .map(|v| metric_fact(AuditMetricId::QueryTime, v)),
        change: change_between(
            f.baseline_median,
            f.current_median,
            AuditMetricId::QueryTime,
        ),
        bucket_start_seconds: seconds(f.bucket_start),
        exemplar_query_id: f.exemplar.clone(),
        comparability: TrendComparability {
            engine_version: f.engine_version.clone(),
            rule_version: f.rule_version.clone(),
        },
        governance_item_id: alert.governance_item_id.map(|id| id.to_string()),
    })
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

fn resolve_target(
    store: &HistoryStore,
    requested: Option<String>,
) -> doctor_history::Result<Option<String>> {
    let targets = store.targets()?;
    Ok(match requested {
        Some(target) => targets.into_iter().find(|known| *known == target),
        None => targets.into_iter().next(),
    })
}

pub(super) async fn alerts(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: HistoryAlertsRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_history_alerts_request",
        ));
    }
    let store = store_of(&state)?;
    let worker =
        tokio::task::spawn_blocking(move || -> doctor_history::Result<HistoryAlertsResponse> {
            let now = unix_now();
            let target = resolve_target(&store, request.target)?;
            let mut alerts = Vec::new();
            let mut counts = [0u64; 4];
            if let Some(target) = target.as_deref() {
                evaluate_alerts(&store, target, now)?;
                let templates = store.templates(target)?;
                for alert in store.alerts(target, 500)? {
                    let template = templates
                        .iter()
                        .find(|t| t.template.template_key == alert.template_key);
                    if let Some(view) = alert_view(alert, template) {
                        counts[view.state as usize] += 1;
                        alerts.push(view);
                    }
                }
            }
            Ok(HistoryAlertsResponse {
                target,
                alerts,
                counts: AlertCounts {
                    open: counts[AlertState::Open as usize].to_string(),
                    acknowledged: counts[AlertState::Acknowledged as usize].to_string(),
                    resolved: counts[AlertState::Resolved as usize].to_string(),
                    muted: counts[AlertState::Muted as usize].to_string(),
                },
                channels: vec![
                    AlertChannel {
                        name: "in_app".into(),
                        available: true,
                    },
                    AlertChannel {
                        name: "webhook".into(),
                        available: false,
                    },
                    AlertChannel {
                        name: "email".into(),
                        available: false,
                    },
                ],
                rule: AlertRule {
                    min_severity: TrendSeverity::Medium,
                    mute_hours: ALERT_MUTE_HOURS,
                    current_hours: ALERT_CURRENT_HOURS,
                    baseline_hours: ALERT_BASELINE_HOURS,
                    calibrated: false,
                },
                limitations: [
                    "alerts_only_from_regression_verdicts",
                    "unscored_and_not_comparable_raise_no_alert",
                    "in_app_only_no_outbound_notification",
                    "rule_thresholds_pending_g2_calibration",
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

pub(super) async fn alert_action(
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
    let change = store
        .alert_action(id, action, unix_now(), i64::from(ALERT_MUTE_HOURS) * 3600)
        .map_err(|_| internal_failure(&state))?;
    match change {
        StoreChange::Applied => Ok(StatusCode::NO_CONTENT.into_response()),
        StoreChange::NotFound => Err(failure(&state, StatusCode::NOT_FOUND, "alert_not_found")),
        StoreChange::Rejected => Err(failure(
            &state,
            StatusCode::CONFLICT,
            "alert_action_not_allowed",
        )),
        StoreChange::ActiveItemExists => Err(failure(
            &state,
            StatusCode::CONFLICT,
            "governance_item_already_open",
        )),
    }
}

/// What a pinned snapshot keeps besides its view: the source-unit numbers
/// the comparator needs. Literal-free.
#[derive(Deserialize, Serialize)]
struct PinnedSnapshot {
    view: GovernanceSnapshot,
    executions: u64,
    valid: u64,
    query_time_median: Option<u64>,
    scan_bytes_median: Option<u64>,
    cpu_cost_median: Option<u64>,
}

impl PinnedSnapshot {
    fn stats(&self) -> WindowStats {
        WindowStats {
            executions: self.executions,
            valid: self.valid,
            query_time_median: self.query_time_median,
            scan_bytes_median: self.scan_bytes_median,
            cpu_cost_median: self.cpu_cost_median,
            engine_version: self.view.comparability.engine_version.clone(),
            rule_version: self.view.comparability.rule_version.clone(),
        }
    }
}

fn item_view(
    store: &HistoryStore,
    item: StoredItem,
) -> doctor_history::Result<Option<GovernanceItem>> {
    let Some(state) = parse::<GovernanceState>(&item.state) else {
        return Ok(None);
    };
    let Some(origin) = parse::<GovernanceOrigin>(&item.origin) else {
        return Ok(None);
    };
    let template = store
        .templates(&item.target)?
        .into_iter()
        .find(|t| t.template.template_key == item.template_key);
    let snapshot = item
        .snapshot_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<PinnedSnapshot>(json).ok())
        .map(|pinned| pinned.view);
    let events = store
        .item_events(item.id)?
        .into_iter()
        .filter_map(|event| {
            Some(GovernanceEvent {
                at_seconds: seconds(event.at),
                kind: event.kind,
                from: match event.from_state.as_deref() {
                    Some(text) => Some(parse(text)?),
                    None => None,
                },
                to: parse(&event.to_state)?,
                actor: event.actor,
                note: event.note,
            })
        })
        .collect();
    Ok(Some(GovernanceItem {
        item_id: item.id.to_string(),
        target: item.target,
        template_key: item.template_key,
        template: template
            .as_ref()
            .and_then(|t| t.template.template.clone())
            .or_else(|| snapshot.as_ref().and_then(|s| s.template.clone())),
        kind: template
            .map(|t| t.template.kind.as_str().to_owned())
            .unwrap_or_default(),
        title: item.title,
        origin,
        alert_id: item.alert_id.map(|id| id.to_string()),
        state,
        created_seconds: seconds(item.created_at),
        updated_seconds: seconds(item.updated_at),
        change_seconds: item.change_at.map(seconds),
        snapshot,
        verification: item
            .verification_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok()),
        events,
        allowed_transitions: governance_manual_transitions(state).to_vec(),
    }))
}

pub(super) async fn governance_list(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: GovernanceListRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_governance_request",
        ));
    }
    let store = store_of(&state)?;
    let worker =
        tokio::task::spawn_blocking(move || -> doctor_history::Result<GovernanceListResponse> {
            let mut items = Vec::new();
            for item in store.items(request.target.as_deref())? {
                if let Some(view) = item_view(&store, item)? {
                    items.push(view);
                }
            }
            Ok(GovernanceListResponse {
                items,
                limitations: [
                    "records_only_changes_are_made_outside_this_tool",
                    "verification_compares_history_windows_not_profiles",
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

async fn single_item(
    state: &ApiState,
    store: Arc<HistoryStore>,
    id: i64,
) -> Result<Response, ApiFailure> {
    let item =
        tokio::task::spawn_blocking(move || -> doctor_history::Result<Option<GovernanceItem>> {
            match store.item(id)? {
                Some(item) => item_view(&store, item),
                None => Ok(None),
            }
        })
        .await
        .map_err(|_| internal_failure(state))?
        .map_err(|_| internal_failure(state))?;
    item.map(no_store)
        .ok_or_else(|| failure(state, StatusCode::NOT_FOUND, "governance_item_not_found"))
}

fn item_id(state: &ApiState, text: &str) -> Result<i64, ApiFailure> {
    text.parse()
        .map_err(|_| failure(state, StatusCode::BAD_REQUEST, "invalid_governance_request"))
}

fn applied(
    state: &ApiState,
    change: StoreChange,
    rejected: &'static str,
) -> Result<(), ApiFailure> {
    match change {
        StoreChange::Applied => Ok(()),
        StoreChange::NotFound => Err(failure(
            state,
            StatusCode::NOT_FOUND,
            "governance_item_not_found",
        )),
        StoreChange::Rejected => Err(failure(state, StatusCode::CONFLICT, rejected)),
        StoreChange::ActiveItemExists => Err(failure(
            state,
            StatusCode::CONFLICT,
            "governance_item_already_open",
        )),
    }
}

pub(super) async fn governance_create(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: GovernanceCreateRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_governance_request",
        ));
    }
    let store = store_of(&state)?;
    let template = store
        .templates(&request.target)
        .map_err(|_| internal_failure(&state))?
        .into_iter()
        .find(|t| t.template.template_key == request.template_key)
        .ok_or_else(|| failure(&state, StatusCode::NOT_FOUND, "history_template_not_found"))?;
    let alert_id = match request.alert_id.as_deref() {
        Some(text) => {
            let id = item_id(&state, text)?;
            let alert = store.alert(id).map_err(|_| internal_failure(&state))?;
            if !alert.is_some_and(|alert| {
                alert.template_key == request.template_key && alert.target == request.target
            }) {
                return Err(failure(&state, StatusCode::NOT_FOUND, "alert_not_found"));
            }
            Some(id)
        }
        None => None,
    };
    // A neutral default title; the template itself is shown beside it.
    let title = request.title.clone().unwrap_or_else(|| {
        let identity: String = template.template.identity.chars().take(12).collect();
        format!(
            "{}.{} · {identity}",
            template.template.catalog, template.template.database
        )
    });
    let id = store
        .create_item(
            &request.target,
            &request.template_key,
            title.trim(),
            &text_of(&request.origin),
            alert_id,
            unix_now(),
        )
        .map_err(|_| internal_failure(&state))?
        .ok_or_else(|| failure(&state, StatusCode::CONFLICT, "governance_item_already_open"))?;
    single_item(&state, store, id).await
}

/// RD-02: only the hand-driven transitions of `governance_manual_transitions`.
pub(super) async fn governance_transition(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: GovernanceTransitionRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_governance_request",
        ));
    }
    let store = store_of(&state)?;
    let id = item_id(&state, &request.item_id)?;
    let item = store
        .item(id)
        .map_err(|_| internal_failure(&state))?
        .ok_or_else(|| failure(&state, StatusCode::NOT_FOUND, "governance_item_not_found"))?;
    let from = parse::<GovernanceState>(&item.state).ok_or_else(|| internal_failure(&state))?;
    if !governance_manual_transitions(from).contains(&request.to) {
        return Err(failure(
            &state,
            StatusCode::CONFLICT,
            "illegal_governance_transition",
        ));
    }
    let update = ItemUpdate {
        from: item.state,
        to: text_of(&request.to),
        kind: "transition".into(),
        actor: ACTOR_USER.into(),
        note: request.note.map(|note| note.trim().to_owned()),
        ..ItemUpdate::default()
    };
    applied(
        &state,
        store
            .update_item(id, &update, unix_now())
            .map_err(|_| internal_failure(&state))?,
        "illegal_governance_transition",
    )?;
    single_item(&state, store, id).await
}

/// Pins the template's current window as the "before" side. Allowed until
/// a change is recorded.
pub(super) async fn governance_pin(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: GovernanceItemRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_governance_request",
        ));
    }
    let store = store_of(&state)?;
    let id = item_id(&state, &request.item_id)?;
    let item = store
        .item(id)
        .map_err(|_| internal_failure(&state))?
        .ok_or_else(|| failure(&state, StatusCode::NOT_FOUND, "governance_item_not_found"))?;
    if !matches!(item.state.as_str(), "new" | "triaged" | "in_progress") {
        return Err(failure(&state, StatusCode::CONFLICT, "snapshot_locked"));
    }
    let now = unix_now();
    let windows = TrendWindows::ending_at(now, PIN_HOURS, ALERT_BASELINE_HOURS);
    let evaluated = evaluate(&store, &item.target, windows, windows.current_start)
        .map_err(|_| internal_failure(&state))?;
    let buckets = evaluated
        .buckets
        .get(&item.template_key)
        .cloned()
        .unwrap_or_default();
    let stats = window_stats(&buckets, windows.current_start, windows.end, None)
        .ok_or_else(|| failure(&state, StatusCode::CONFLICT, "no_current_samples_to_pin"))?;
    let verdict = evaluated
        .trends
        .into_iter()
        .find(|(t, _)| t.template.template_key == item.template_key)
        .map(|(_, trend)| trend.verdict);
    let template = evaluated
        .templates
        .into_iter()
        .find(|t| t.template.template_key == item.template_key)
        .and_then(|t| t.template.template);
    let exemplar = store
        .exemplars(&item.template_key, windows.current_start, windows.end, 1)
        .map_err(|_| internal_failure(&state))?
        .into_iter()
        .next();
    let pinned = PinnedSnapshot {
        view: GovernanceSnapshot {
            pinned_seconds: seconds(now),
            template,
            window_start_seconds: seconds(windows.current_start),
            window_end_seconds: seconds(windows.end),
            comparability: TrendComparability {
                engine_version: stats.engine_version.clone(),
                rule_version: stats.rule_version.clone(),
            },
            executions: stats.executions.to_string(),
            valid_samples: stats.valid.to_string(),
            query_time_median: stats
                .query_time_median
                .map(|v| metric_fact(AuditMetricId::QueryTime, v)),
            scan_bytes_median: stats
                .scan_bytes_median
                .map(|v| metric_fact(AuditMetricId::ScanBytes, v)),
            cpu_cost_median: stats
                .cpu_cost_median
                .map(|v| metric_fact(AuditMetricId::CpuCost, v)),
            exemplar_query_id: exemplar.map(|e| e.query_id),
            verdict,
        },
        executions: stats.executions,
        valid: stats.valid,
        query_time_median: stats.query_time_median,
        scan_bytes_median: stats.scan_bytes_median,
        cpu_cost_median: stats.cpu_cost_median,
    };
    let update = ItemUpdate {
        from: item.state.clone(),
        to: item.state,
        kind: "pinned".into(),
        actor: ACTOR_USER.into(),
        snapshot_json: Some(serde_json::to_string(&pinned).map_err(|_| internal_failure(&state))?),
        ..ItemUpdate::default()
    };
    applied(
        &state,
        store
            .update_item(id, &update, now)
            .map_err(|_| internal_failure(&state))?,
        "snapshot_locked",
    )?;
    single_item(&state, store, id).await
}

/// Records the change made outside this tool: `in_progress` →
/// `pending_verification`. Needs a pinned snapshot from before the change.
pub(super) async fn governance_change(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: GovernanceChangeRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_governance_request",
        ));
    }
    let store = store_of(&state)?;
    let id = item_id(&state, &request.item_id)?;
    let item = store
        .item(id)
        .map_err(|_| internal_failure(&state))?
        .ok_or_else(|| failure(&state, StatusCode::NOT_FOUND, "governance_item_not_found"))?;
    if item.state != "in_progress" {
        return Err(failure(
            &state,
            StatusCode::CONFLICT,
            "illegal_governance_transition",
        ));
    }
    let pinned = item
        .snapshot_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<PinnedSnapshot>(json).ok())
        .ok_or_else(|| failure(&state, StatusCode::CONFLICT, "snapshot_required"))?;
    let now = unix_now();
    let change_at = request.change_seconds.map_or(now, i64::from);
    if change_at > now || change_at < i64::from(pinned.view.pinned_seconds) {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "change_time_out_of_range",
        ));
    }
    let update = ItemUpdate {
        from: item.state,
        to: text_of(&GovernanceState::PendingVerification),
        kind: "change_recorded".into(),
        actor: ACTOR_USER.into(),
        note: Some(request.note.trim().to_owned()),
        change_at: Some(change_at),
        ..ItemUpdate::default()
    };
    applied(
        &state,
        store
            .update_item(id, &update, now)
            .map_err(|_| internal_failure(&state))?,
        "illegal_governance_transition",
    )?;
    single_item(&state, store, id).await
}

/// RD-03: the comparator decides. `improved` closes the item; any other
/// outcome records the result and leaves it pending.
pub(super) async fn governance_verify(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: GovernanceItemRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_governance_request",
        ));
    }
    let store = store_of(&state)?;
    let id = item_id(&state, &request.item_id)?;
    let item = store
        .item(id)
        .map_err(|_| internal_failure(&state))?
        .ok_or_else(|| failure(&state, StatusCode::NOT_FOUND, "governance_item_not_found"))?;
    if item.state != "pending_verification" {
        return Err(failure(
            &state,
            StatusCode::CONFLICT,
            "illegal_governance_transition",
        ));
    }
    let (Some(pinned), Some(change_at)) = (
        item.snapshot_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<PinnedSnapshot>(json).ok()),
        item.change_at,
    ) else {
        return Err(failure(&state, StatusCode::CONFLICT, "snapshot_required"));
    };
    let now = unix_now();
    // The after window starts at the first whole hour after the change.
    let after_start = (change_at + HISTORY_BUCKET_SECONDS - 1).div_euclid(HISTORY_BUCKET_SECONDS)
        * HISTORY_BUCKET_SECONDS;
    let end = (now.div_euclid(HISTORY_BUCKET_SECONDS) + 1) * HISTORY_BUCKET_SECONDS;
    let before = pinned.stats();
    let buckets: Vec<doctor_core::TrendBucket> = store
        .buckets(&item.target, after_start, end, Some(&item.template_key))
        .map_err(|_| internal_failure(&state))?
        .into_iter()
        .map(|bucket| doctor_core::TrendBucket {
            bucket_start: bucket.delta.bucket_start,
            engine_version: bucket.engine_version,
            rule_version: bucket.rule_version,
            delta: bucket.delta,
        })
        .collect();
    let after = window_stats(
        &buckets,
        after_start,
        end,
        Some((&before.engine_version, &before.rule_version)),
    );
    let other_versions: Vec<(String, String)> = buckets
        .iter()
        .filter(|b| {
            b.engine_version != before.engine_version || b.rule_version != before.rule_version
        })
        .map(|b| (b.engine_version.clone(), b.rule_version.clone()))
        .collect();
    let verdict = if before.rule_version != doctor_core::history_rule_version() {
        ChangeVerdict::NotComparable(doctor_contracts::TrendNotComparableReason::RuleVersionChanged)
    } else {
        verify_change(&before, after.as_ref(), &other_versions)
    };
    let runs = store
        .runs(&item.target, after_start, end, u32::MAX)
        .map_err(|_| internal_failure(&state))?;
    let coverage = crate::history_trend::coverage_map(&runs, after_start, end);
    let mut limitations = vec!["verification_compares_history_windows_not_profiles".to_owned()];
    let gaps = coverage
        .values()
        .any(|state| *state != doctor_contracts::CoverageHourState::Complete);
    if gaps {
        limitations.push("after_window_not_fully_collected".into());
    }
    let after_valid = after.as_ref().map_or(0, |a| a.valid);
    let evidence = match verdict {
        ChangeVerdict::NotComparable(_) => EvidenceCompleteness::Unavailable,
        _ if gaps || after.as_ref().is_some_and(|a| a.valid < a.executions) => {
            EvidenceCompleteness::Partial
        }
        _ => EvidenceCompleteness::Complete,
    };
    let outcome = match verdict {
        ChangeVerdict::Improved => VerificationOutcome::Improved,
        ChangeVerdict::NotImproved => VerificationOutcome::NotImproved,
        ChangeVerdict::InsufficientSamples => VerificationOutcome::InsufficientSamples,
        ChangeVerdict::WorkloadSignalMissing => VerificationOutcome::WorkloadSignalMissing,
        ChangeVerdict::NotComparable(reason) => VerificationOutcome::NotComparable { reason },
    };
    let confidence = matches!(
        verdict,
        ChangeVerdict::Improved | ChangeVerdict::NotImproved
    )
    .then(|| {
        if evidence == EvidenceCompleteness::Partial {
            TrendConfidence::Low
        } else if after_valid >= 3 * u64::from(TREND_MIN_CURRENT_SAMPLES) {
            TrendConfidence::High
        } else {
            TrendConfidence::Medium
        }
    });
    let metric =
        |name: &str, id: AuditMetricId, b: Option<u64>, a: Option<u64>| VerificationMetric {
            metric: name.into(),
            before: b.map(|v| metric_fact(id.clone(), v)),
            after: a.map(|v| metric_fact(id.clone(), v)),
            change: change_between(b, a, id),
        };
    let verification = GovernanceVerification {
        verified_seconds: seconds(now),
        after_start_seconds: seconds(after_start),
        after_end_seconds: seconds(end),
        outcome,
        metrics: vec![
            metric(
                "query_time",
                AuditMetricId::QueryTime,
                before.query_time_median,
                after.as_ref().and_then(|a| a.query_time_median),
            ),
            metric(
                "scan_bytes",
                AuditMetricId::ScanBytes,
                before.scan_bytes_median,
                after.as_ref().and_then(|a| a.scan_bytes_median),
            ),
            metric(
                "cpu_cost",
                AuditMetricId::CpuCost,
                before.cpu_cost_median,
                after.as_ref().and_then(|a| a.cpu_cost_median),
            ),
        ],
        executions_before: before.executions.to_string(),
        executions_after: after.as_ref().map_or(0, |a| a.executions).to_string(),
        confidence,
        evidence,
        limitations,
    };
    let to = if verdict == ChangeVerdict::Improved {
        GovernanceState::Closed
    } else {
        GovernanceState::PendingVerification
    };
    let update = ItemUpdate {
        from: item.state,
        to: text_of(&to),
        kind: "verified".into(),
        actor: ACTOR_COMPARATOR.into(),
        verification_json: Some(
            serde_json::to_string(&verification).map_err(|_| internal_failure(&state))?,
        ),
        ..ItemUpdate::default()
    };
    applied(
        &state,
        store
            .update_item(id, &update, now)
            .map_err(|_| internal_failure(&state))?,
        "illegal_governance_transition",
    )?;
    single_item(&state, store, id).await
}
