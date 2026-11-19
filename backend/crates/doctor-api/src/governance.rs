//! Stage D (ADR-6, ADR-7): in-app alerts from stage C regressions and the
//! governance queue. Only the local history is read; StarRocks is never
//! changed and no credential is needed. `closed` is reached only through a
//! comparator result; `pending_verification` only by recording a change.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use doctor_contracts::{
    AlertAction, AlertActionRequest, AlertChannel, AlertCounts, AlertRule, AlertState, AlertView,
    AlertsRuleChangedResolveResponse, AuditMetricId, EvidenceCompleteness, GovernanceChangeRequest,
    GovernanceCreateRequest, GovernanceEvent, GovernanceItem, GovernanceItemRequest,
    GovernanceListRequest, GovernanceListResponse, GovernanceOrigin, GovernanceSnapshot,
    GovernanceState, GovernanceTransitionRequest, GovernanceVerification, HistoryAlertsRequest,
    HistoryAlertsResponse, TrendChange, TrendComparability, TrendConfidence, TrendDirection,
    TrendNotComparableReason, TrendSeverity, TrendVerdict, VerificationMetric, VerificationOutcome,
    WorkloadSignal, governance_manual_transitions,
};
use doctor_core::{
    ChangeVerdict, HISTORY_BUCKET_SECONDS, TREND_MIN_CURRENT_SAMPLES, TrendBucket, TrendWindows,
    WindowStats, verify_change, window_stats,
};
use doctor_history::{
    AlertFacts, AlertUpsert, HistoryStore, ItemUpdate, StoreChange, StoredAlert, StoredItem,
};

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
/// at least medium severity raise or refresh an alert for its semantics;
/// templates back inside the band resolve matching alerts; unscored and not
/// comparable verdicts change nothing.
pub(crate) fn evaluate_alerts(
    store: &HistoryStore,
    target: &str,
    now: i64,
) -> doctor_history::Result<()> {
    evaluate_alerts_with_loads(store, target, now, (Vec::new(), Vec::new()))
}

/// The SQL alerts plus the load alerts of the same cycle (task 59), sent in
/// one notification.
pub(crate) fn evaluate_alerts_with_loads(
    store: &HistoryStore,
    target: &str,
    now: i64,
    loads: (Vec<i64>, Vec<i64>),
) -> doctor_history::Result<()> {
    let windows = TrendWindows::ending_at(now, ALERT_CURRENT_HOURS, ALERT_BASELINE_HOURS);
    let (mut raised, mut resolved) = (Vec::new(), Vec::new());
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
                match store.upsert_alert(target, key, &facts, now)? {
                    AlertUpsert::Created(id) | AlertUpsert::Reopened(id) => raised.push(id),
                    AlertUpsert::Updated | AlertUpsert::Suppressed => {}
                }
            }
            TrendVerdict::Stable | TrendVerdict::Improvement => {
                resolved.extend(store.resolve_alerts(
                    target,
                    key,
                    &trend.engine_version,
                    &trend.rule_version,
                    now,
                )?);
            }
            _ => {}
        }
    }
    // Outbound notifications are best effort: a queue problem never blocks
    // the in-app alerts.
    let _ = crate::notify::enqueue_alert_events(
        store, target, &raised, &resolved, &loads.0, &loads.1, now,
    );
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
        rule_changed: alert.superseded
            || f.rule_version != TrendBucket::active_storage_rule_version(),
        comparability: TrendComparability {
            engine_version: f.engine_version.clone(),
            rule_version: TrendBucket::rule_version_label(&f.rule_version).to_owned(),
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
            let mut rule_changed = 0u64;
            let outbound = crate::notify::channel_states(&store)?;
            if let Some(target) = target.as_deref() {
                evaluate_alerts(&store, target, now)?;
                rule_changed = store.count_rule_changed_alerts(
                    target,
                    &TrendBucket::active_storage_rule_version(),
                )?;
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
                    rule_changed: rule_changed.to_string(),
                },
                channels: std::iter::once(AlertChannel {
                    name: "in_app".into(),
                    available: true,
                })
                .chain(outbound.into_iter().map(|(kind, available)| AlertChannel {
                    name: kind.as_str().into(),
                    available,
                }))
                .collect(),
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

/// Manually resolves the target's active alerts from superseded semantics.
pub(super) async fn alerts_resolve_rule_changed(
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
    let worker = tokio::task::spawn_blocking(
        move || -> doctor_history::Result<AlertsRuleChangedResolveResponse> {
            let target = resolve_target(&store, request.target)?;
            let resolved = match target.as_deref() {
                Some(target) => store.resolve_rule_changed_alerts(
                    target,
                    &TrendBucket::active_storage_rule_version(),
                    unix_now(),
                )?,
                None => 0,
            };
            Ok(AlertsRuleChangedResolveResponse {
                target,
                resolved: resolved.to_string(),
            })
        },
    );
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
    #[serde(default)]
    window_semantics: Option<BTreeSet<(String, String)>>,
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

fn verification_metric(
    outcome: &VerificationOutcome,
    name: &str,
    metric: AuditMetricId,
    before: Option<u64>,
    after: Option<u64>,
) -> VerificationMetric {
    VerificationMetric {
        metric: name.into(),
        before: before.map(|value| metric_fact(metric.clone(), value)),
        after: after.map(|value| metric_fact(metric.clone(), value)),
        change: if matches!(outcome, VerificationOutcome::NotComparable { .. }) {
            None
        } else {
            change_between(before, after, metric)
        },
    }
}

fn read_verification(json: &str) -> Option<GovernanceVerification> {
    let mut verification: GovernanceVerification = serde_json::from_str(json).ok()?;
    if matches!(
        verification.outcome,
        VerificationOutcome::NotComparable { .. }
    ) {
        for metric in &mut verification.metrics {
            metric.change = None;
        }
    }
    Some(verification)
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
        .map(|pinned| {
            let mut view = pinned.view;
            view.comparability.rule_version =
                TrendBucket::rule_version_label(&view.comparability.rule_version).to_owned();
            view
        });
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
            .and_then(read_verification),
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
    let window_semantics = buckets
        .iter()
        .filter(|bucket| (windows.current_start..windows.end).contains(&bucket.bucket_start))
        .map(|bucket| (bucket.engine_version.clone(), bucket.rule_version.clone()))
        .collect();
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
        window_semantics: Some(window_semantics),
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
    let buckets: Vec<TrendBucket> = store
        .buckets(&item.target, after_start, end, Some(&item.template_key))
        .map_err(|_| internal_failure(&state))?
        .into_iter()
        .map(|bucket| TrendBucket {
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
    let observed_versions: Vec<_> = pinned
        .window_semantics
        .iter()
        .flatten()
        .cloned()
        .chain(
            buckets
                .iter()
                .map(|b| (b.engine_version.clone(), b.rule_version.clone())),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let active_rule = TrendBucket::active_storage_rule_version();
    let active_pack = TrendBucket::version_pack_in(&active_rule);
    let observed_packs = std::iter::once(&before.rule_version)
        .chain(observed_versions.iter().map(|(_, rule)| rule))
        .map(|rule| TrendBucket::version_pack_in(rule));
    let pack_changed = observed_packs
        .clone()
        .flatten()
        .any(|pack| Some(pack) != active_pack);
    let semantics_unknown = pinned
        .window_semantics
        .as_ref()
        .is_none_or(BTreeSet::is_empty)
        || observed_packs.clone().any(|pack| pack.is_none());
    let (verdict, semantics_limitation) =
        match verify_change(&before, after.as_ref(), &observed_versions) {
            verdict @ ChangeVerdict::NotComparable(
                TrendNotComparableReason::EngineVersionChanged,
            ) => (verdict, None),
            _ if pack_changed => (
                ChangeVerdict::NotComparable(TrendNotComparableReason::RuleVersionChanged),
                Some("version_pack_changed"),
            ),
            _ if semantics_unknown => (
                ChangeVerdict::NotComparable(TrendNotComparableReason::RuleVersionChanged),
                Some("version_pack_semantics_unknown"),
            ),
            _ if before.rule_version != active_rule => (
                ChangeVerdict::NotComparable(TrendNotComparableReason::RuleVersionChanged),
                None,
            ),
            verdict => (verdict, None),
        };
    let runs = store
        .runs(&item.target, after_start, end, u32::MAX)
        .map_err(|_| internal_failure(&state))?;
    let coverage = crate::history_trend::coverage_map(&runs, after_start, end);
    let mut limitations = vec!["verification_compares_history_windows_not_profiles".to_owned()];
    if let Some(limitation) = semantics_limitation {
        limitations.push(limitation.into());
    }
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
    let metrics = vec![
        verification_metric(
            &outcome,
            "query_time",
            AuditMetricId::QueryTime,
            before.query_time_median,
            after.as_ref().and_then(|a| a.query_time_median),
        ),
        verification_metric(
            &outcome,
            "scan_bytes",
            AuditMetricId::ScanBytes,
            before.scan_bytes_median,
            after.as_ref().and_then(|a| a.scan_bytes_median),
        ),
        verification_metric(
            &outcome,
            "cpu_cost",
            AuditMetricId::CpuCost,
            before.cpu_cost_median,
            after.as_ref().and_then(|a| a.cpu_cost_median),
        ),
    ];
    let verification = GovernanceVerification {
        verified_seconds: seconds(now),
        after_start_seconds: seconds(after_start),
        after_end_seconds: seconds(end),
        outcome,
        metrics,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use doctor_contracts::{
        HistoryLoadComparison, HistoryLoadNotComparableReason, NotifyEvent, NotifySettings,
        TrendUnscoredReason,
    };
    use doctor_core::{
        HistoryBucketDelta, HistoryMetric, HistoryTemplate, HistoryTemplateKind, trend_series,
    };
    use doctor_history::{DeliveryFilter, RunRecord, RunState};
    use doctor_notify::AlertBatch;
    use serde_json::{Value, json};

    use crate::notify::tests::{email_channel, email_target};

    fn sample_bucket(
        key: &str,
        start: i64,
        count: u64,
        latency: u64,
        errors: u64,
    ) -> HistoryBucketDelta {
        let metric = |value: u64| {
            let mut metric = HistoryMetric {
                valid_count: count,
                sum: u128::from(value) * u128::from(count),
                max: Some(u128::from(value)),
                ..HistoryMetric::default()
            };
            for _ in 0..count {
                metric.histogram.record(value);
            }
            metric
        };
        HistoryBucketDelta {
            template_key: key.into(),
            bucket_start: start,
            executions: count,
            errors,
            query_time: metric(latency),
            scan_bytes: metric(3_000),
            ..HistoryBucketDelta::default()
        }
    }

    fn collect_bucket(
        store: &HistoryStore,
        target: &str,
        engine: &str,
        slice: std::ops::Range<i64>,
        bucket: HistoryBucketDelta,
    ) {
        let count = bucket.executions;
        assert!(
            store
                .record_slice(
                    target,
                    engine,
                    &doctor_core::history_rule_version(),
                    &RunRecord {
                        started_at: slice.end,
                        slice_start: slice.start,
                        slice_end: slice.end,
                        state: RunState::Complete,
                        reason: None,
                        rows_read: count,
                        attributed: count,
                        partial_statements: 0,
                        unavailable_statements: 0,
                        duration_ms: 1,
                    },
                    slice.end,
                    &[HistoryTemplate {
                        template_key: bucket.template_key.clone(),
                        kind: HistoryTemplateKind::OfficialDigest,
                        identity: "digest".into(),
                        catalog: "c".into(),
                        database: "d".into(),
                        template: None,
                    }],
                    &[bucket],
                )
                .unwrap()
        );
    }

    fn json_post(body: Value) -> Request {
        Request::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn item_request(id: i64) -> Request {
        json_post(json!({"item_id": id.to_string()}))
    }

    #[tokio::test]
    async fn bare_key_alert_survives_upgrade_regression_recovery_and_reopen() {
        let path = std::env::temp_dir().join(format!(
            "doctor-task78-alert-handoff-{}.sqlite",
            std::process::id()
        ));
        let hour = HISTORY_BUCKET_SECONDS;
        let now = (unix_now().div_euclid(hour) - 4) * hour;
        let windows = TrendWindows::ending_at(now, ALERT_CURRENT_HOURS, ALERT_BASELINE_HOURS);
        let old = {
            let store = HistoryStore::open(&path).unwrap();
            let facts = AlertFacts {
                severity: "high".into(),
                confidence: "high".into(),
                evidence: "complete".into(),
                workload: "stable".into(),
                baseline_median: Some(100),
                current_median: Some(250),
                bucket_start: windows.current_start - 7 * hour,
                engine_version: "3.5.14".into(),
                rule_version: doctor_core::history_rule_version(),
                ..AlertFacts::default()
            };
            let AlertUpsert::Created(id) = store
                .upsert_alert("t", "k", &facts, facts.bucket_start)
                .unwrap()
            else {
                panic!("expected a new legacy alert");
            };
            assert_eq!(
                store
                    .alert_action(id, "acknowledge", facts.bucket_start + 1, 0)
                    .unwrap(),
                StoreChange::Applied
            );
            assert_eq!(
                store
                    .alert_action(id, "mute", facts.bucket_start + 2, 24 * hour)
                    .unwrap(),
                StoreChange::Applied
            );
            store.alert(id).unwrap().unwrap()
        };
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DROP TABLE collection_slice_evidence; DROP TABLE audit_slice_deltas;
                 UPDATE meta SET value = '8' WHERE key = 'schema_version';",
            )
            .unwrap();
        let state = ApiState::new(ApiConfig {
            history: HistoryConfig {
                path: Some(path.clone()),
                ..HistoryConfig::default()
            },
            ..ApiConfig::default()
        })
        .unwrap();
        let store = state.history.store().unwrap().as_ref();
        assert_eq!(store.alert(old.id).unwrap(), Some(old.clone()));
        email_channel(store, true, 25);
        let recipient = email_target(store, "dba", "medium");
        store
            .set_notify_settings(&NotifySettings {
                notify_resolved: true,
                ..NotifySettings::default()
            })
            .unwrap();
        let queued = || store.deliveries(&DeliveryFilter::default(), 10).unwrap();
        for index in (1..=6).rev() {
            let start = windows.current_start - index * hour;
            collect_bucket(
                store,
                "t",
                "3.5.14",
                start..start + hour,
                sample_bucket("k", start, 3, 100, 0),
            );
        }
        collect_bucket(
            store,
            "t",
            "3.5.14",
            now..now + hour,
            sample_bucket("k", now, 10, 400, 0),
        );
        evaluate_alerts(store, "t", now).unwrap();
        let alerts = store.alerts("t", 10).unwrap();
        assert_eq!(alerts.len(), 2);
        let fresh = alerts.iter().find(|alert| alert.id != old.id).unwrap();
        assert_eq!(
            fresh.facts.rule_version,
            TrendBucket::active_storage_rule_version()
        );
        assert_eq!(
            (fresh.state.as_str(), fresh.first_seen, fresh.muted_until),
            ("open", now, None)
        );
        let original = StoredAlert {
            superseded: true,
            ..old
        };
        assert_eq!(store.alert(original.id).unwrap(), Some(original.clone()));
        assert!(alert_view(original.clone(), None).unwrap().rule_changed);
        assert!(!alert_view(fresh.clone(), None).unwrap().rule_changed);

        let handoff_queue = queued();
        assert_eq!(handoff_queue.len(), 1);
        let delivery = &handoff_queue[0];
        assert_eq!(delivery.target_id, Some(recipient));
        let batch: AlertBatch = serde_json::from_str(&delivery.payload_json).unwrap();
        assert_eq!(batch.event, NotifyEvent::NewAlerts);
        assert_eq!(batch.alerts.len(), 1);
        assert_eq!(batch.alerts[0].alert_id, fresh.id);
        evaluate_alerts(store, "t", now + 1).unwrap();
        assert_eq!(queued(), handoff_queue);

        let recovery = now + i64::from(ALERT_CURRENT_HOURS) * hour;
        collect_bucket(
            store,
            "t",
            "3.5.14",
            recovery..recovery + hour,
            sample_bucket("k", recovery, 10, 100, 0),
        );
        evaluate_alerts(store, "t", recovery).unwrap();
        let resolved = store.alert(fresh.id).unwrap().unwrap();
        assert_eq!(
            (resolved.state.as_str(), resolved.resolved_at),
            ("resolved", Some(recovery))
        );
        assert_eq!(store.alert(original.id).unwrap(), Some(original.clone()));
        let recovery_queue = queued();
        assert_eq!(recovery_queue.len(), 2);
        let batch: AlertBatch = serde_json::from_str(&recovery_queue[0].payload_json).unwrap();
        assert_eq!(batch.event, NotifyEvent::Resolved);
        assert_eq!(batch.alerts.len(), 1);
        assert_eq!(batch.alerts[0].alert_id, fresh.id);
        let response = alert_action(
            State(state.clone()),
            json_post(json!({"alert_id": fresh.id.to_string(), "action": "reopen"})),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let reopened = store.alert(fresh.id).unwrap().unwrap();
        assert_eq!(
            (reopened.state.as_str(), reopened.resolved_at),
            ("open", None)
        );
        assert_eq!(store.alert(original.id).unwrap(), Some(original.clone()));

        assert_eq!(queued(), recovery_queue);
        let response =
            alerts_resolve_rule_changed(State(state.clone()), json_post(json!({"target": "t"})))
                .await
                .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let result: AlertsRuleChangedResolveResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(result.resolved, "1");
        assert_eq!(store.alert(original.id).unwrap().unwrap().state, "resolved");
        assert_eq!(store.alert(fresh.id).unwrap(), Some(reopened));
        assert_eq!(queued(), recovery_queue);
        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn pin_captures_semantic_changes_inside_the_baseline_window() {
        let path =
            std::env::temp_dir().join(format!("doctor-task78-pin-{}.sqlite", std::process::id()));
        let state = ApiState::new(ApiConfig {
            history: HistoryConfig {
                path: Some(path.clone()),
                ..HistoryConfig::default()
            },
            ..ApiConfig::default()
        })
        .unwrap();
        let store = state.history.store().unwrap();
        let hour = HISTORY_BUCKET_SECONDS;
        let current = unix_now().div_euclid(hour) * hour;
        let rule = doctor_core::history_rule_version();
        let active_rule = TrendBucket::active_storage_rule_version();
        let old_pack = format!("{rule}|vp=starrocks-3.5.14@0.1.0:{:064x}:{:064x}", 1, 2);
        use TrendNotComparableReason::{EngineVersionChanged, RuleVersionChanged};
        for (target, first_engine, first_rule, reason, limitation) in [
            ("engine", "3.5.13", &rule, EngineVersionChanged, None),
            (
                "pack",
                "3.5.14",
                &old_pack,
                RuleVersionChanged,
                Some("version_pack_changed"),
            ),
            (
                "bare",
                "3.5.14",
                &rule,
                RuleVersionChanged,
                Some("version_pack_semantics_unknown"),
            ),
        ] {
            let first = current - 2 * hour;
            let last = current - hour;
            collect_bucket(
                store,
                target,
                first_engine,
                first..last,
                sample_bucket(target, first, 20, 400, 0),
            );
            collect_bucket(
                store,
                target,
                "3.5.14",
                last..current,
                sample_bucket(target, last, 20, 400, 0),
            );
            rusqlite::Connection::open(&path).unwrap().execute(
                "UPDATE buckets SET rule_version = ?1 WHERE template_key = ?2 AND bucket_start = ?3",
                rusqlite::params![first_rule, target, first],
            ).unwrap();
            let id = store
                .create_item(target, target, "case", "manual", None, current)
                .unwrap()
                .unwrap();
            governance_pin(State(state.clone()), item_request(id))
                .await
                .unwrap();
            let item = store.item(id).unwrap().unwrap();
            let pinned: PinnedSnapshot =
                serde_json::from_str(item.snapshot_json.as_deref().unwrap()).unwrap();
            assert_eq!(pinned.view.comparability.engine_version, "3.5.14");
            assert_eq!(pinned.view.comparability.rule_version, active_rule);
            assert_eq!(
                pinned.window_semantics,
                Some(BTreeSet::from([
                    (first_engine.into(), first_rule.clone()),
                    ("3.5.14".into(), active_rule.clone()),
                ]))
            );

            collect_bucket(
                store,
                target,
                "3.5.14",
                current..current + hour,
                sample_bucket(target, current, 20, 100, 0),
            );
            assert_eq!(
                store
                    .update_item(
                        id,
                        &ItemUpdate {
                            from: "new".into(),
                            to: "pending_verification".into(),
                            kind: "change_recorded".into(),
                            actor: ACTOR_USER.into(),
                            change_at: Some(current),
                            ..ItemUpdate::default()
                        },
                        current
                    )
                    .unwrap(),
                StoreChange::Applied
            );
            let response = governance_verify(State(state.clone()), item_request(id))
                .await
                .unwrap();
            let result: GovernanceItem =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(result.state, GovernanceState::PendingVerification);
            let verification = result.verification.unwrap();
            assert_eq!(
                verification.outcome,
                VerificationOutcome::NotComparable { reason },
                "{target}"
            );
            assert_eq!(verification.executions_after, "20");
            for pack_limitation in ["version_pack_changed", "version_pack_semantics_unknown"] {
                assert_eq!(
                    verification
                        .limitations
                        .iter()
                        .any(|entry| entry == pack_limitation),
                    limitation == Some(pack_limitation)
                );
            }
        }
        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_workload_cannot_resolve_an_open_alert() {
        let now = 1000 * HISTORY_BUCKET_SECONDS;
        let windows = TrendWindows::ending_at(now, ALERT_CURRENT_HOURS, ALERT_BASELINE_HOURS);
        let hour = HISTORY_BUCKET_SECONDS;
        let rule = doctor_core::history_rule_version();
        for current_latency in [100, 400] {
            let store = HistoryStore::open_in_memory().unwrap();
            let template = HistoryTemplate {
                template_key: "k".into(),
                kind: HistoryTemplateKind::OfficialDigest,
                identity: "digest".into(),
                catalog: "c".into(),
                database: "d".into(),
                template: None,
            };
            for index in 0..7 {
                let (start, executions, latency) = if index == 6 {
                    (windows.end - hour, 10, current_latency)
                } else {
                    (windows.current_start - (6 - index) * hour, 3, 400)
                };
                let mut bucket = HistoryBucketDelta {
                    template_key: "k".into(),
                    bucket_start: start,
                    executions,
                    query_time: HistoryMetric {
                        valid_count: executions,
                        sum: u128::from(latency) * u128::from(executions),
                        max: Some(u128::from(latency)),
                        ..HistoryMetric::default()
                    },
                    ..HistoryBucketDelta::default()
                };
                for _ in 0..executions {
                    bucket.query_time.histogram.record(latency);
                }
                store
                    .record_slice(
                        "t",
                        "3.5.14",
                        &rule,
                        &RunRecord {
                            started_at: start + hour,
                            slice_start: start,
                            slice_end: start + hour,
                            state: RunState::Complete,
                            reason: None,
                            rows_read: executions,
                            attributed: executions,
                            partial_statements: 0,
                            unavailable_statements: 0,
                            duration_ms: 1,
                        },
                        start + hour,
                        std::slice::from_ref(&template),
                        &[bucket],
                    )
                    .unwrap();
            }
            let facts = AlertFacts {
                severity: "high".into(),
                confidence: "high".into(),
                evidence: "complete".into(),
                workload: "stable".into(),
                baseline_median: Some(400),
                current_median: Some(800),
                relative_permille: Some(1_000),
                bucket_start: windows.current_start,
                exemplar: None,
                engine_version: "3.5.14".into(),
                rule_version: TrendBucket::active_storage_rule_version(),
            };
            let AlertUpsert::Created(id) =
                store.upsert_alert("t", "k", &facts, now - hour).unwrap()
            else {
                panic!("expected a new alert");
            };
            let evaluated = evaluate(&store, "t", windows, windows.current_start).unwrap();
            assert_eq!(
                evaluated.trends[0].1.verdict,
                TrendVerdict::Unscored {
                    reason: TrendUnscoredReason::WorkloadSignalMissing,
                }
            );
            evaluate_alerts_with_loads(&store, "t", now, (Vec::new(), Vec::new())).unwrap();
            let alert = store.alert(id).unwrap().unwrap();
            assert_eq!(alert.state, "open");
            assert_eq!(alert.resolved_at, None);
        }
    }

    #[tokio::test]
    async fn pack_boundary_is_enforced_in_series_load_and_governance() {
        let rule = doctor_core::history_rule_version();
        let active_rule = TrendBucket::active_storage_rule_version();
        let old_rule = format!("{rule}|vp=starrocks-3.5.14@0.1.0:{:064x}:{:064x}", 1, 2);
        assert!(TrendBucket::version_pack_in(&active_rule).is_some());
        for release in ["3.5.14", "3.5.14-23a56ec"] {
            assert_eq!(
                TrendBucket::storage_rule_version(release, &rule),
                active_rule
            );
        }
        for release in ["3.5.14-rc1", "4.2.0"] {
            let storage = TrendBucket::storage_rule_version(release, &rule);
            assert_eq!(storage, format!("{rule}|vp=unknown"));
            assert_eq!(TrendBucket::version_pack_in(&storage), None);
        }

        let path = std::env::temp_dir().join(format!(
            "doctor-task78-pack-boundary-{}.sqlite",
            std::process::id()
        ));
        let state = ApiState::new(ApiConfig {
            history: HistoryConfig {
                path: Some(path.clone()),
                ..HistoryConfig::default()
            },
            ..ApiConfig::default()
        })
        .unwrap();
        let store = state.history.store().unwrap();
        let now = unix_now();
        let hour = HISTORY_BUCKET_SECONDS;
        let day = 24 * hour;
        let today = now.div_euclid(day) * day;
        let baseline = today - 8 * day;
        let current = today - day;
        for (target, before_rule) in [("different_pack", &old_rule), ("same_pack", &active_rule)] {
            let different_pack = before_rule != &active_rule;
            let template = HistoryTemplate {
                template_key: format!("{target}-k"),
                kind: HistoryTemplateKind::OfficialDigest,
                identity: "digest".into(),
                catalog: "c".into(),
                database: "d".into(),
                template: None,
            };
            for (start, latency) in [(baseline, 250), (current, 150)] {
                let mut bucket = HistoryBucketDelta {
                    template_key: template.template_key.clone(),
                    bucket_start: start,
                    executions: 20,
                    query_time: HistoryMetric {
                        valid_count: 20,
                        sum: u128::from(latency) * 20,
                        max: Some(u128::from(latency)),
                        ..HistoryMetric::default()
                    },
                    scan_bytes: HistoryMetric {
                        valid_count: 20,
                        sum: 60_000,
                        max: Some(3_000),
                        ..HistoryMetric::default()
                    },
                    ..HistoryBucketDelta::default()
                };
                for _ in 0..20 {
                    bucket.query_time.histogram.record(latency);
                    bucket.scan_bytes.histogram.record(3_000);
                }
                store
                    .record_slice(
                        target,
                        "3.5.14",
                        &rule,
                        &RunRecord {
                            started_at: start + day,
                            slice_start: start,
                            slice_end: start + day,
                            state: RunState::Complete,
                            reason: None,
                            rows_read: 20,
                            attributed: 20,
                            partial_statements: 0,
                            unavailable_statements: 0,
                            duration_ms: 1,
                        },
                        start + day,
                        std::slice::from_ref(&template),
                        &[bucket],
                    )
                    .unwrap();
            }
            if different_pack {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute(
                        "UPDATE buckets SET rule_version = ?1 WHERE template_key = ?2 AND bucket_start = ?3",
                        rusqlite::params![before_rule, template.template_key, baseline],
                    )
                    .unwrap();
            }
            let windows = TrendWindows {
                baseline_start: baseline,
                current_start: current,
                end: today,
            };
            let evaluated = evaluate(store, target, windows, baseline).unwrap();
            let own = &evaluated.buckets[&template.template_key];
            let points = trend_series(
                own,
                &evaluated.coverage,
                baseline,
                today,
                "3.5.14",
                &active_rule,
            );
            assert_eq!(points[0].comparable, !different_pack);
            assert_eq!(points[0].median.is_some(), !different_pack);
            assert!(points[7 * 24].comparable);
            if different_pack {
                let trend = &evaluated.trends[0].1;
                assert_eq!(
                    trend.verdict,
                    TrendVerdict::NotComparable {
                        reason: TrendNotComparableReason::RuleVersionChanged
                    }
                );
                assert!(trend.limitations.contains(&"version_pack_changed"));
                assert!(trend.change.is_none());

                // An hour with a matching bucket and a second Pack is also a gap in the series.
                let mut mixed = own.clone();
                let mut other = own[0].clone();
                other.bucket_start = current;
                mixed.push(other);
                let point = &trend_series(
                    &mixed,
                    &evaluated.coverage,
                    current,
                    current + hour,
                    "3.5.14",
                    &active_rule,
                )[0];
                assert!(!point.comparable);
                assert_eq!(point.median, None);
                assert_eq!(point.executions, Some(40));
            }
            let load = crate::history_load::build(store, Some(target.into()), 14, now).unwrap();
            let week = load
                .week
                .iter()
                .find(|row| i64::from(row.current_day_seconds) == current)
                .unwrap();
            if different_pack {
                assert_eq!(
                    week.comparison,
                    HistoryLoadComparison::NotComparable {
                        reason: HistoryLoadNotComparableReason::VersionOrRuleChanged
                    }
                );
                assert_eq!(load.segments.len(), 2);
                assert_eq!(
                    load.segments[0].rule_versions,
                    load.segments[1].rule_versions
                );
            } else {
                assert_eq!(
                    week.comparison,
                    HistoryLoadComparison::Exact {
                        delta: "0".into(),
                        permille: Some(0)
                    }
                );
            }
            let facts = AlertFacts {
                severity: "high".into(),
                confidence: "high".into(),
                evidence: "complete".into(),
                workload: "stable".into(),
                baseline_median: Some(100),
                current_median: Some(250),
                relative_permille: Some(1_500),
                bucket_start: baseline,
                exemplar: None,
                engine_version: "3.5.14".into(),
                rule_version: before_rule.to_owned(),
            };
            let AlertUpsert::Created(alert_id) = store
                .upsert_alert(target, &template.template_key, &facts, current)
                .unwrap()
            else {
                panic!("expected a new alert");
            };
            let resolved = store
                .resolve_alerts(target, &template.template_key, "3.5.14", &active_rule, now)
                .unwrap();
            assert_eq!(
                resolved,
                if different_pack {
                    Vec::new()
                } else {
                    vec![alert_id]
                }
            );
            assert_eq!(
                store.alert(alert_id).unwrap().unwrap().resolved_at,
                (!different_pack).then_some(now)
            );

            let id = store
                .create_item(
                    target,
                    &template.template_key,
                    "case",
                    "manual",
                    None,
                    baseline,
                )
                .unwrap()
                .unwrap();
            store
                .update_item(
                    id,
                    &ItemUpdate {
                        from: "new".into(),
                        to: "pending_verification".into(),
                        kind: "change_recorded".into(),
                        actor: "local_user".into(),
                        change_at: Some(current),
                        snapshot_json: Some(json!({
                            "view": {
                                "pinned_seconds": baseline + hour,
                                "template": null,
                                "window_start_seconds": baseline,
                                "window_end_seconds": baseline + hour,
                                "comparability": {"engine_version": "3.5.14", "rule_version": before_rule},
                                "executions":"20", "valid_samples":"20"
                            },
                            "executions":20, "valid":20, "query_time_median":250,
                            "scan_bytes_median":3000, "window_semantics":[["3.5.14", before_rule]]
                        }).to_string()),
                        ..ItemUpdate::default()
                    },
                    current,
                )
                .unwrap();
            let request = Request::builder()
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(json!({"item_id":id.to_string()}).to_string()))
                .unwrap();
            let response = governance_verify(State(state.clone()), request)
                .await
                .unwrap();
            let result: GovernanceItem =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(result.snapshot.unwrap().comparability.rule_version, rule);
            let verification = result.verification.unwrap();
            if different_pack {
                assert_eq!(result.state, GovernanceState::PendingVerification);
                assert_eq!(
                    verification.outcome,
                    VerificationOutcome::NotComparable {
                        reason: TrendNotComparableReason::RuleVersionChanged
                    }
                );
                assert!(
                    verification
                        .limitations
                        .iter()
                        .any(|text| text == "version_pack_changed")
                );
            } else {
                assert_eq!(result.state, GovernanceState::Closed);
                assert_eq!(verification.outcome, VerificationOutcome::Improved);
            }
        }
        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn v8_upgrade_preserves_history_as_unknown() {
        let path =
            std::env::temp_dir().join(format!("doctor-task78-v8-{}.sqlite", std::process::id()));
        let hour = HISTORY_BUCKET_SECONDS;
        let day = 24 * hour;
        let now = unix_now().div_euclid(day) * day - 12 * hour;
        let windows = TrendWindows::ending_at(now, ALERT_CURRENT_HOURS, ALERT_BASELINE_HOURS);
        let change_at = windows.current_start;
        let split = change_at + hour / 2;
        let rule = doctor_core::history_rule_version();
        let snapshot = json!({
            "view": {
                "pinned_seconds": change_at - 1,
                "template": null,
                "window_start_seconds": change_at - 6 * hour,
                "window_end_seconds": change_at,
                "comparability": {"engine_version":"3.5.14", "rule_version":rule},
                "executions":"18", "valid_samples":"18"
            },
            "executions":18, "valid":18, "query_time_median":400, "scan_bytes_median":3000
        });
        let (item_id, alert_id);
        {
            let store = HistoryStore::open(&path).unwrap();
            for index in 0..7 {
                let start = change_at + (index - 6) * hour;
                let (end, count, latency, errors) = if index == 6 {
                    (split, 10, 100, 2)
                } else {
                    (start + hour, 3, 400, 0)
                };
                collect_bucket(
                    &store,
                    "t",
                    "3.5.14",
                    start..end,
                    sample_bucket("k", start, count, latency, errors),
                );
            }
            let facts = AlertFacts {
                severity: "high".into(),
                confidence: "high".into(),
                evidence: "complete".into(),
                workload: "stable".into(),
                baseline_median: Some(100),
                current_median: Some(400),
                relative_permille: Some(3_000),
                bucket_start: change_at - hour,
                exemplar: None,
                engine_version: "3.5.14".into(),
                rule_version: rule.clone(),
            };
            let AlertUpsert::Created(id) = store.upsert_alert("t", "k", &facts, change_at).unwrap()
            else {
                panic!("expected a new alert");
            };
            alert_id = id;
            item_id = store
                .create_item("t", "k", "legacy", "manual", None, change_at - hour)
                .unwrap()
                .unwrap();
            store
                .update_item(
                    item_id,
                    &ItemUpdate {
                        from: "new".into(),
                        to: "pending_verification".into(),
                        kind: "change_recorded".into(),
                        actor: "local_user".into(),
                        change_at: Some(change_at),
                        snapshot_json: Some(snapshot.to_string()),
                        ..ItemUpdate::default()
                    },
                    change_at,
                )
                .unwrap();
        }
        {
            // v8 used the same tables, with a bare rule string in bucket keys.
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute("UPDATE buckets SET rule_version = ?1", [&rule])
                .unwrap();
            connection
                .execute_batch(
                    "DROP TABLE collection_slice_evidence; DROP TABLE audit_slice_deltas;
                 UPDATE meta SET value = '8' WHERE key = 'schema_version';",
                )
                .unwrap();
        }
        for attempt in 0..2 {
            let state = ApiState::new(ApiConfig {
                history: HistoryConfig {
                    path: Some(path.clone()),
                    ..HistoryConfig::default()
                },
                ..ApiConfig::default()
            })
            .unwrap();
            let db = rusqlite::Connection::open(&path).unwrap();
            let version: String = db
                .query_row(
                    "SELECT value FROM meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(version, "10");
            let collection_tables: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table'
                       AND name IN ('collection_slice_evidence', 'audit_slice_deltas')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(collection_tables, 2);
            drop(db);
            let store = state.history.store().unwrap();
            let buckets: Vec<_> = store
                .buckets("t", windows.baseline_start, windows.end, None)
                .unwrap()
                .into_iter()
                .filter(|bucket| bucket.rule_version == rule)
                .collect();
            assert_eq!(buckets.len(), 7);
            assert_eq!(
                buckets
                    .iter()
                    .map(|bucket| bucket.delta.executions)
                    .sum::<u64>(),
                28
            );
            assert_eq!(
                buckets
                    .iter()
                    .map(|bucket| bucket.delta.query_time.sum)
                    .sum::<u128>(),
                8_200
            );
            assert_eq!(
                store.target_state("t").unwrap().watermark,
                Some(if attempt == 0 {
                    split
                } else {
                    change_at + hour
                })
            );
            let item = store.item(item_id).unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(item.snapshot_json.as_deref().unwrap()).unwrap(),
                snapshot
            );
            assert_eq!(item.state, "pending_verification");
            assert_eq!(store.item_events(item_id).unwrap().len(), 2 + attempt);

            let evaluated = evaluate(store, "t", windows, windows.current_start).unwrap();
            let trend = &evaluated.trends[0].1;
            assert_eq!(
                trend.verdict,
                TrendVerdict::NotComparable {
                    reason: TrendNotComparableReason::RuleVersionChanged
                }
            );
            assert!(
                trend
                    .limitations
                    .contains(&"version_pack_semantics_unknown")
            );
            assert_eq!(trend.change, None);
            evaluate_alerts_with_loads(store, "t", now, (Vec::new(), Vec::new())).unwrap();
            let alert = store.alert(alert_id).unwrap().unwrap();
            assert_eq!(alert.state, "open");
            assert_eq!(alert.resolved_at, None);
            assert!(alert_view(alert, None).unwrap().rule_changed);

            let request = Request::builder()
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"item_id":item_id.to_string()}).to_string(),
                ))
                .unwrap();
            let response = governance_verify(State(state.clone()), request)
                .await
                .unwrap();
            let result: GovernanceItem =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(result.state, GovernanceState::PendingVerification);
            let verification = result.verification.unwrap();
            assert_eq!(
                verification.outcome,
                VerificationOutcome::NotComparable {
                    reason: TrendNotComparableReason::RuleVersionChanged
                }
            );
            assert!(
                verification
                    .limitations
                    .iter()
                    .any(|text| text == "version_pack_semantics_unknown")
            );

            let fresh = sample_bucket("k", change_at, 7, 250, 3);
            if attempt == 0 {
                collect_bucket(store, "t", "3.5.14", split..change_at + hour, fresh.clone());
            }
            let active_rule = TrendBucket::active_storage_rule_version();
            let evaluated = evaluate(store, "t", windows, change_at).unwrap();
            let own = &evaluated.buckets["k"];
            let same_hour: Vec<_> = own.iter().filter(|b| b.bucket_start == change_at).collect();
            assert_eq!(same_hour.len(), 2);
            for (key, expected) in [
                (&rule, sample_bucket("k", change_at, 10, 100, 2)),
                (&active_rule, fresh),
            ] {
                let actual = same_hour.iter().find(|b| b.rule_version == *key).unwrap();
                assert_eq!(actual.delta, expected);
            }
            let points = trend_series(
                own,
                &evaluated.coverage,
                change_at,
                change_at + hour,
                "3.5.14",
                &active_rule,
            );
            assert_eq!(points[0].executions, Some(17));
            assert_eq!(points[0].median, None);

            let load = crate::history_load::build(store, Some("t".into()), 2, now).unwrap();
            let day_start = change_at.div_euclid(day) * day;
            let daily = load
                .days
                .iter()
                .find(|row| i64::from(row.start_seconds) == day_start)
                .unwrap();
            assert_eq!(daily.executions.as_deref(), Some("35"));
            assert_eq!(daily.errors.as_deref(), Some("5"));
            assert_eq!(daily.query_time_sum, None);
            assert_eq!(daily.query_time_permyriad, None);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn not_comparable_governance_verification_has_no_metric_changes() {
        let path = std::env::temp_dir().join(format!(
            "doctor-task78-legacy-verification-{}.sqlite",
            std::process::id()
        ));
        let state = ApiState::new(ApiConfig {
            history: HistoryConfig {
                path: Some(path.clone()),
                ..HistoryConfig::default()
            },
            ..ApiConfig::default()
        })
        .unwrap();
        let store = state.history.store().unwrap();
        let hour = HISTORY_BUCKET_SECONDS;
        let start = unix_now().div_euclid(hour) * hour - hour;
        let rule = "older-rule";
        let metric = |value: u64| {
            let mut metric = HistoryMetric {
                valid_count: 20,
                sum: u128::from(value) * 20,
                max: Some(u128::from(value)),
                ..HistoryMetric::default()
            };
            for _ in 0..20 {
                metric.histogram.record(value);
            }
            metric
        };
        store
            .record_slice(
                "t",
                "3.5.14",
                rule,
                &RunRecord {
                    started_at: start + hour,
                    slice_start: start,
                    slice_end: start + hour,
                    state: RunState::Complete,
                    reason: None,
                    rows_read: 20,
                    attributed: 20,
                    partial_statements: 0,
                    unavailable_statements: 0,
                    duration_ms: 1,
                },
                start + hour,
                &[HistoryTemplate {
                    template_key: "k".into(),
                    kind: HistoryTemplateKind::OfficialDigest,
                    identity: "digest".into(),
                    catalog: "c".into(),
                    database: "d".into(),
                    template: None,
                }],
                &[HistoryBucketDelta {
                    template_key: "k".into(),
                    bucket_start: start,
                    executions: 20,
                    query_time: metric(150),
                    scan_bytes: metric(2_000),
                    cpu_cost: metric(30),
                    ..HistoryBucketDelta::default()
                }],
            )
            .unwrap();
        let id = store
            .create_item("t", "k", "old rule", "manual", None, start - hour)
            .unwrap()
            .unwrap();
        store.update_item(
            id,
            &ItemUpdate {
                from: "new".into(),
                to: "pending_verification".into(),
                kind: "change_recorded".into(),
                actor: "local_user".into(),
                change_at: Some(start),
                snapshot_json: Some(json!({
                    "view": {
                        "pinned_seconds":start - 1, "window_start_seconds":start - hour,
                        "template":null,
                        "window_end_seconds":start,
                        "comparability":{
                            "engine_version":"3.5.14",
                            "rule_version":TrendBucket::storage_rule_version("3.5.14", rule)
                        },
                        "executions":"20", "valid_samples":"20"
                    },
                    "executions":20, "valid":20, "query_time_median":250, "scan_bytes_median":3000,
                    "cpu_cost_median":50,
                    "window_semantics":[["3.5.14", TrendBucket::storage_rule_version("3.5.14", rule)]]
                }).to_string()),
                ..ItemUpdate::default()
            },
            start,
        ).unwrap();
        let request = Request::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(json!({"item_id":id.to_string()}).to_string()))
            .unwrap();
        let response = governance_verify(State(state.clone()), request)
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["state"], "pending_verification");
        assert_eq!(
            body["verification"]["outcome"],
            json!({"state":"not_comparable","reason":"rule_version_changed"})
        );
        assert_eq!(body["verification"]["evidence"], "unavailable");
        let metrics = body["verification"]["metrics"].as_array().unwrap();
        assert_eq!(metrics.len(), 3);
        for metric in metrics {
            assert!(metric["before"].is_object());
            assert!(metric["after"].is_object());
            assert_eq!(metric["change"], Value::Null);
        }
        let item = store.item(id).unwrap().unwrap();
        let stored: Value =
            serde_json::from_str(item.verification_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored["metrics"], body["verification"]["metrics"]);
        drop(state);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn governance_item_read_hides_historical_incomparable_deltas() {
        let store = HistoryStore::open_in_memory().unwrap();
        let id = store
            .create_item("t", "k", "old verification", "manual", None, 1)
            .unwrap()
            .unwrap();
        let mut verification = GovernanceVerification {
            verified_seconds: 2,
            after_start_seconds: 1,
            after_end_seconds: 2,
            outcome: VerificationOutcome::NotComparable {
                reason: TrendNotComparableReason::RuleVersionChanged,
            },
            metrics: vec![verification_metric(
                &VerificationOutcome::Improved,
                "query_time",
                AuditMetricId::QueryTime,
                Some(250),
                Some(150),
            )],
            executions_before: "20".into(),
            executions_after: "20".into(),
            confidence: None,
            evidence: EvidenceCompleteness::Unavailable,
            limitations: Vec::new(),
        };
        assert_eq!(
            verification.metrics[0]
                .change
                .as_ref()
                .unwrap()
                .relative_percent
                .as_deref(),
            Some("-40.0")
        );
        let stored_json = serde_json::to_string(&verification).unwrap();
        store
            .update_item(
                id,
                &ItemUpdate {
                    from: "new".into(),
                    to: "pending_verification".into(),
                    kind: "verified".into(),
                    actor: "comparator".into(),
                    verification_json: Some(stored_json.clone()),
                    ..ItemUpdate::default()
                },
                2,
            )
            .unwrap();
        let events = store.item_events(id).unwrap();
        let item = store.item(id).unwrap().unwrap();
        let response = item_view(&store, item).unwrap().unwrap();
        verification.metrics[0].change = None;
        assert_eq!(response.verification, Some(verification));
        assert_eq!(
            store
                .item(id)
                .unwrap()
                .unwrap()
                .verification_json
                .as_deref(),
            Some(stored_json.as_str())
        );
        assert_eq!(store.item_events(id).unwrap(), events);
    }

    #[test]
    fn comparable_zero_baseline_keeps_absolute_change_only() {
        let metric = verification_metric(
            &VerificationOutcome::Improved,
            "scan_bytes",
            AuditMetricId::ScanBytes,
            Some(0),
            Some(20),
        );
        let change = metric.change.unwrap();
        assert_eq!(change.direction, TrendDirection::Up);
        assert_eq!(change.magnitude, metric_fact(AuditMetricId::ScanBytes, 20));
        assert_eq!(change.relative_percent, None);
    }
}
