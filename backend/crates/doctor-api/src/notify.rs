//! Outbound alert notifications (v1.0.1). Channels and targets live in the
//! history store with their secrets sealed under the master key; views say
//! only whether a secret is saved. New alerts (and, if chosen, resolved
//! ones) are queued per target by `enqueue_alert_events`; one bounded worker
//! sends them in order with fixed timeouts and a short retry schedule.
use std::collections::BTreeMap;

use doctor_contracts::{
    NOTIFY_MAX_TARGETS_PER_CHANNEL, NotifyChannelConfig, NotifyChannelKind,
    NotifyChannelSaveRequest, NotifyChannelView, NotifyDeliveriesRequest, NotifyDeliveriesResponse,
    NotifyDeliveryBrief, NotifyDeliveryState, NotifyDeliveryView, NotifyErrorClass, NotifyEvent,
    NotifyKeySource, NotifyOverviewResponse, NotifySettings, NotifySeverity, NotifySmsUsage,
    NotifyTargetDeleteRequest, NotifyTargetInput, NotifyTargetView, NotifyTestRequest,
    NotifyTestResponse,
};
use doctor_history::{
    DeliveryAttempt, DeliveryFilter, HistoryStore, NewDelivery, SecretUpdate, StoredAlert,
    StoredChannel, StoredDelivery, StoredTarget, TargetWrite,
};
use doctor_notify::message::severity_rank;
use doctor_notify::{AlertBatch, AlertFact, Context, Recipient, SecretKey, SendError, validate};
use serde::{Deserialize, Serialize};

use super::*;
use crate::audit::no_store;
use crate::history::unix_now;

/// Retries after the 1st, 2nd and 3rd failed attempt; the 4th failure is final.
const RETRY_DELAYS: [i64; 3] = [60, 300, 1_800];
const WORKER_PERIOD: Duration = Duration::from_secs(10);
const WORKER_BATCH: u32 = 20;
const MAX_PENDING: u32 = 500;
const DELIVERY_PAGE: u32 = 200;
pub(crate) const DELIVERY_RETENTION_DAYS: u16 = 30;

/// The master key and where it came from; `None` keeps notifications off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotifyKey {
    pub key: SecretKey,
    pub source: NotifyKeySource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NotifyOverviewRequest {}

/// Private target fields, sealed together.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TargetSecret {
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signing_secret: Option<String>,
}

fn channel_purpose(kind: NotifyChannelKind) -> String {
    format!("channel:{}", kind.as_str())
}

const TARGET_PURPOSE: &str = "target";

fn failure(state: &ApiState, status: StatusCode, code: &'static str) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        status,
        code,
        "notify",
        code,
        "change_input",
    )
}

fn ready(state: &ApiState) -> Result<(Arc<HistoryStore>, NotifyKey), ApiFailure> {
    let store = state
        .history
        .store()
        .cloned()
        .ok_or_else(|| failure(state, StatusCode::CONFLICT, "history_disabled"))?;
    let key = state
        .config
        .notify_key
        .clone()
        .ok_or_else(|| failure(state, StatusCode::CONFLICT, "notify_key_unavailable"))?;
    Ok((store, key))
}

fn text_of<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn parse_text<T: for<'de> Deserialize<'de>>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

fn seconds(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn open_target_secret(key: &SecretKey, target: &StoredTarget) -> Option<TargetSecret> {
    let blob = target.secret.as_deref()?;
    serde_json::from_slice(&key.open(TARGET_PURPOSE, blob)?).ok()
}

fn open_channel_secret(key: &SecretKey, channel: &StoredChannel) -> Option<String> {
    let blob = channel.secret.as_deref()?;
    String::from_utf8(key.open(&channel_purpose(channel.kind), blob)?).ok()
}

fn brief(delivery: &StoredDelivery) -> Option<NotifyDeliveryBrief> {
    Some(NotifyDeliveryBrief {
        at_seconds: seconds(delivery.last_attempt_at.unwrap_or(delivery.created_at)),
        state: parse_text(&delivery.state)?,
        error: delivery.error_class.as_deref().and_then(parse_text),
        provider_code: delivery.provider_code.clone(),
    })
}

/// A bot channel needs no settings of its own; until saved it is off.
fn default_config(kind: NotifyChannelKind) -> Option<NotifyChannelConfig> {
    match kind {
        NotifyChannelKind::Feishu => Some(NotifyChannelConfig::Feishu {}),
        NotifyChannelKind::Dingtalk => Some(NotifyChannelConfig::Dingtalk {}),
        _ => None,
    }
}

fn overview(
    state: &ApiState,
    store: &HistoryStore,
    key: &NotifyKey,
) -> doctor_history::Result<NotifyOverviewResponse> {
    let channels = store.notify_channels()?;
    let targets = store.notify_targets()?;
    let latest: BTreeMap<i64, StoredDelivery> = store
        .latest_deliveries()?
        .into_iter()
        .filter_map(|delivery| delivery.target_id.map(|id| (id, delivery)))
        .collect();
    let target_views: Vec<NotifyTargetView> = targets
        .iter()
        .map(|target| {
            let secret = open_target_secret(&key.key, target);
            NotifyTargetView {
                target_id: target.id.to_string(),
                kind: target.kind,
                name: target.name.clone(),
                enabled: target.enabled,
                min_severity: parse_text(&target.min_severity).unwrap_or(NotifySeverity::Medium),
                address_hint: target.address_hint.clone(),
                signing_secret_saved: secret
                    .as_ref()
                    .is_some_and(|secret| secret.signing_secret.is_some()),
                secret_unreadable: target.secret.is_some() && secret.is_none(),
                last: latest.get(&target.id).and_then(brief),
            }
        })
        .collect();
    let channel_views = NotifyChannelKind::ALL
        .into_iter()
        .map(|kind| {
            let stored = channels.iter().find(|channel| channel.kind == kind);
            let mine: Vec<&NotifyTargetView> = target_views
                .iter()
                .filter(|target| target.kind == kind)
                .collect();
            NotifyChannelView {
                kind,
                enabled: stored.is_some_and(|channel| channel.enabled),
                config: stored
                    .map(|channel| channel.config.clone())
                    .or_else(|| default_config(kind)),
                secret_saved: stored.is_some_and(|channel| channel.secret.is_some()),
                secret_unreadable: stored.is_some_and(|channel| {
                    channel.secret.is_some() && open_channel_secret(&key.key, channel).is_none()
                }),
                targets: mine.len() as u16,
                enabled_targets: mine.iter().filter(|target| target.enabled).count() as u16,
                failing_targets: mine
                    .iter()
                    .filter(|target| {
                        target
                            .last
                            .as_ref()
                            .is_some_and(|last| last.state == NotifyDeliveryState::Failed)
                    })
                    .count() as u16,
            }
        })
        .collect();
    Ok(NotifyOverviewResponse {
        settings: store.notify_settings()?,
        channels: channel_views,
        targets: target_views,
        remote_collection_allowed: state.config.history.allow_remote_collection,
        key_source: key.source,
        max_targets_per_channel: NOTIFY_MAX_TARGETS_PER_CHANNEL,
        limitations: [
            "notifications_only_while_collection_runs",
            "provider_delivery_pending_real_accounts",
            "rule_thresholds_pending_g2_calibration",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    })
}

async fn blocking<T: Send + 'static>(
    state: &ApiState,
    work: impl FnOnce() -> Result<T, &'static str> + Send + 'static,
) -> Result<T, ApiFailure> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err("internal")) | Err(_) => Err(internal_failure(state)),
        Ok(Err(code)) => Err(failure(state, StatusCode::BAD_REQUEST, code)),
    }
}

fn internal<E>(_: E) -> &'static str {
    "internal"
}

pub(super) async fn overview_handler(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let _: NotifyOverviewRequest = connector_json_request(&state, request).await?;
    let (store, key) = ready(&state)?;
    let inner = state.clone();
    let response = blocking(&state, move || {
        overview(&inner, &store, &key).map_err(internal)
    })
    .await?;
    Ok(no_store(response))
}

pub(super) async fn settings(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let settings: NotifySettings = connector_json_request(&state, request).await?;
    if !validate::settings(&settings) {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_notify_settings",
        ));
    }
    let (store, key) = ready(&state)?;
    let inner = state.clone();
    let response = blocking(&state, move || {
        let settings = NotifySettings {
            public_url: settings
                .public_url
                .filter(|url| !url.is_empty())
                .map(|url| url.trim_end_matches('/').to_owned()),
            ..settings
        };
        store.set_notify_settings(&settings).map_err(internal)?;
        overview(&inner, &store, &key).map_err(internal)
    })
    .await?;
    Ok(no_store(response))
}

pub(super) async fn channel(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: NotifyChannelSaveRequest = connector_json_request(&state, request).await?;
    let secret_ok = request.secret.as_deref().is_none_or(validate::secret);
    if !validate::channel(&request.config)
        || !secret_ok
        || (request.clear_secret && request.secret.is_some())
    {
        return Err(failure(
            &state,
            StatusCode::BAD_REQUEST,
            "invalid_notify_channel",
        ));
    }
    let (store, key) = ready(&state)?;
    let inner = state.clone();
    let response = blocking(&state, move || {
        let kind = request.config.kind();
        let update = match (&request.secret, request.clear_secret) {
            (Some(secret), _) => SecretUpdate::Set(
                key.key
                    .seal(&channel_purpose(kind), secret.as_bytes())
                    .ok_or("internal")?,
            ),
            (None, true) => SecretUpdate::Clear,
            (None, false) => SecretUpdate::Keep,
        };
        store
            .save_notify_channel(&request.config, request.enabled, &update, unix_now())
            .map_err(internal)?;
        overview(&inner, &store, &key).map_err(internal)
    })
    .await?;
    Ok(no_store(response))
}

/// The target as it would be stored, and the plaintext it would send with.
struct ResolvedTarget {
    write: TargetWrite,
    secret: TargetSecret,
    existing_id: Option<i64>,
}

fn resolve_target(
    store: &HistoryStore,
    key: &SecretKey,
    input: &NotifyTargetInput,
) -> Result<ResolvedTarget, &'static str> {
    if !validate::target_name(&input.name)
        || input
            .signing_secret
            .as_deref()
            .is_some_and(|secret| !validate::secret(secret))
        || (input.clear_signing_secret && input.signing_secret.is_some())
    {
        return Err("invalid_notify_target");
    }
    let existing_id = match &input.target_id {
        Some(id) => Some(id.parse::<i64>().map_err(|_| "invalid_notify_target")?),
        None => None,
    };
    let existing = match existing_id {
        Some(id) => Some(
            store
                .notify_target(id)
                .map_err(internal)?
                .filter(|target| target.kind == input.kind)
                .ok_or("notify_target_not_found")?,
        ),
        None => None,
    };
    let saved = match &existing {
        Some(target) if target.secret.is_some() => {
            Some(open_target_secret(key, target).ok_or("notify_secret_unreadable")?)
        }
        _ => None,
    }
    .unwrap_or_default();
    let mut secret = TargetSecret::default();
    let (address, address_hint) = match input.kind {
        NotifyChannelKind::Email => {
            let address = input.address.clone().ok_or("invalid_notify_target")?;
            if !validate::email_address(&address) {
                return Err("invalid_notify_target");
            }
            (Some(address.clone()), address)
        }
        NotifyChannelKind::AliyunSms | NotifyChannelKind::TencentSms => {
            let phone = match &input.address {
                Some(phone) => validate::phone(phone).ok_or("invalid_notify_target")?,
                None => saved.phone.clone().ok_or("invalid_notify_target")?,
            };
            let hint = validate::mask_phone(&phone);
            secret.phone = Some(phone);
            (None, hint)
        }
        NotifyChannelKind::Feishu | NotifyChannelKind::Dingtalk => {
            let webhook = match &input.webhook {
                Some(webhook) if validate::webhook(input.kind, webhook) => webhook.clone(),
                Some(_) => return Err("invalid_notify_webhook"),
                None => saved.webhook.clone().ok_or("invalid_notify_webhook")?,
            };
            secret.signing_secret = match (&input.signing_secret, input.clear_signing_secret) {
                (Some(new), _) => Some(new.clone()),
                (None, true) => None,
                (None, false) => saved.signing_secret.clone(),
            };
            let hint = validate::webhook_hint(&webhook);
            secret.webhook = Some(webhook);
            (None, hint)
        }
    };
    let sealed = if input.kind == NotifyChannelKind::Email {
        SecretUpdate::Clear
    } else {
        let plain = serde_json::to_vec(&secret).map_err(internal)?;
        SecretUpdate::Set(key.seal(TARGET_PURPOSE, &plain).ok_or("internal")?)
    };
    Ok(ResolvedTarget {
        write: TargetWrite {
            kind: input.kind,
            name: input.name.clone(),
            enabled: input.enabled,
            min_severity: text_of(&input.min_severity),
            address,
            address_hint,
            secret: sealed,
        },
        secret,
        existing_id,
    })
}

pub(super) async fn target(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let input: NotifyTargetInput = connector_json_request(&state, request).await?;
    let (store, key) = ready(&state)?;
    let inner = state.clone();
    let response = blocking(&state, move || {
        let resolved = resolve_target(&store, &key.key, &input)?;
        let now = unix_now();
        match resolved.existing_id {
            Some(id) => {
                if !store
                    .update_notify_target(id, &resolved.write, now)
                    .map_err(internal)?
                {
                    return Err("notify_target_not_found");
                }
            }
            None => {
                store
                    .insert_notify_target(&resolved.write, NOTIFY_MAX_TARGETS_PER_CHANNEL, now)
                    .map_err(internal)?
                    .ok_or("notify_target_limit")?;
            }
        }
        overview(&inner, &store, &key).map_err(internal)
    })
    .await?;
    Ok(no_store(response))
}

pub(super) async fn target_delete(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: NotifyTargetDeleteRequest = connector_json_request(&state, request).await?;
    let id: i64 = request
        .target_id
        .parse()
        .map_err(|_| failure(&state, StatusCode::BAD_REQUEST, "invalid_notify_target"))?;
    let (store, key) = ready(&state)?;
    let inner = state.clone();
    let response = blocking(&state, move || {
        if !store.delete_notify_target(id).map_err(internal)? {
            return Err("notify_target_not_found");
        }
        overview(&inner, &store, &key).map_err(internal)
    })
    .await?;
    Ok(no_store(response))
}

fn context<'a>(settings: &'a NotifySettings) -> Context<'a> {
    Context {
        language: settings.language,
        link: settings.public_url.as_deref(),
        calibrated: false,
    }
}

fn recipient<'a>(
    kind: NotifyChannelKind,
    address: Option<&'a str>,
    secret: &'a TargetSecret,
) -> Option<Recipient<'a>> {
    Some(match kind {
        NotifyChannelKind::Email => Recipient::Email(address?),
        NotifyChannelKind::Feishu | NotifyChannelKind::Dingtalk => Recipient::Bot {
            webhook: secret.webhook.as_deref()?,
            signing_secret: secret.signing_secret.as_deref(),
        },
        NotifyChannelKind::AliyunSms | NotifyChannelKind::TencentSms => {
            Recipient::Phone(secret.phone.as_deref()?)
        }
    })
}

struct Outcome {
    result: Result<(), SendError>,
    duration_ms: u32,
}

/// Sends one batch to one target, or says why it cannot be sent.
fn send_once(
    store: &HistoryStore,
    key: &SecretKey,
    kind: NotifyChannelKind,
    address: Option<&str>,
    secret: &TargetSecret,
    batch: &AlertBatch,
    now: i64,
) -> Result<Outcome, NotifyErrorClass> {
    let channel = match store
        .notify_channel(kind)
        .map_err(|_| NotifyErrorClass::NotConfigured)?
    {
        Some(channel) => channel,
        None => StoredChannel {
            kind,
            enabled: false,
            config: default_config(kind).ok_or(NotifyErrorClass::NotConfigured)?,
            secret: None,
        },
    };
    let channel_secret = match &channel.secret {
        Some(_) => {
            Some(open_channel_secret(key, &channel).ok_or(NotifyErrorClass::SecretUnreadable)?)
        }
        None => None,
    };
    if let Some(limit) = channel.config.hourly_limit() {
        let used = store
            .sent_since(kind, now - 3_600)
            .map_err(|_| NotifyErrorClass::NotConfigured)?;
        if used >= u32::from(limit) {
            return Err(NotifyErrorClass::RateLimited);
        }
    }
    let recipient = recipient(kind, address, secret).ok_or(NotifyErrorClass::NotConfigured)?;
    let settings = store
        .notify_settings()
        .map_err(|_| NotifyErrorClass::NotConfigured)?;
    let started = std::time::Instant::now();
    let result = doctor_notify::send(
        &channel.config,
        channel_secret.as_deref(),
        &recipient,
        batch,
        &context(&settings),
        now,
    );
    Ok(Outcome {
        result,
        duration_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

pub(super) async fn test(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: NotifyTestRequest = connector_json_request(&state, request).await?;
    let (store, key) = ready(&state)?;
    let response = blocking(&state, move || {
        let input = request.target;
        let resolved = resolve_target(&store, &key.key, &input)?;
        let now = unix_now();
        let batch = AlertBatch::test();
        let outcome = send_once(
            &store,
            &key.key,
            input.kind,
            resolved.write.address.as_deref(),
            &resolved.secret,
            &batch,
            now,
        );
        let (state, error, provider_code, duration_ms) = match outcome {
            Ok(Outcome {
                result: Ok(()),
                duration_ms,
            }) => (NotifyDeliveryState::Sent, None, None, duration_ms),
            Ok(Outcome {
                result: Err(error),
                duration_ms,
            }) => (
                NotifyDeliveryState::Failed,
                Some(error.class),
                error.provider_code,
                duration_ms,
            ),
            Err(NotifyErrorClass::RateLimited) => (
                NotifyDeliveryState::Skipped,
                Some(NotifyErrorClass::RateLimited),
                None,
                0,
            ),
            Err(class) => (NotifyDeliveryState::Failed, Some(class), None, 0),
        };
        let record = NewDelivery {
            kind: input.kind,
            target_id: resolved.existing_id,
            target_name: input.name.clone(),
            event: text_of(&NotifyEvent::Test),
            cluster: None,
            alert_count: 0,
            payload_json: "{}".to_owned(),
        };
        let attempt = DeliveryAttempt {
            state: text_of(&state),
            next_attempt_at: None,
            error_class: error.map(|class| text_of(&class)),
            provider_code: provider_code.clone(),
            duration_ms: Some(duration_ms),
            attempted: state != NotifyDeliveryState::Skipped,
        };
        store
            .record_delivery(&record, &attempt, now)
            .map_err(internal)?;
        Ok(NotifyTestResponse {
            state,
            error,
            provider_code,
            duration_ms,
        })
    })
    .await?;
    Ok(no_store(response))
}

pub(super) async fn deliveries(
    State(state): State<ApiState>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: NotifyDeliveriesRequest = connector_json_request(&state, request).await?;
    let target_id = match &request.target_id {
        Some(id) => Some(
            id.parse::<i64>()
                .map_err(|_| failure(&state, StatusCode::BAD_REQUEST, "invalid_notify_target"))?,
        ),
        None => None,
    };
    let (store, _) = ready(&state)?;
    let response = blocking(&state, move || {
        let now = unix_now();
        let filter = DeliveryFilter {
            kind: request.kind,
            target_id,
            state: request.state.map(|state| text_of(&state)),
        };
        let deliveries = store
            .deliveries(&filter, DELIVERY_PAGE)
            .map_err(internal)?
            .iter()
            .filter_map(delivery_view)
            .collect();
        let (sent_24h, failed_24h, pending) =
            store.delivery_counts(now - 86_400).map_err(internal)?;
        let mut sms_usage = Vec::new();
        for channel in store.notify_channels().map_err(internal)? {
            if let Some(hourly_limit) = channel.config.hourly_limit() {
                sms_usage.push(NotifySmsUsage {
                    kind: channel.kind,
                    used: store
                        .sent_since(channel.kind, now - 3_600)
                        .map_err(internal)?
                        .min(u32::from(u16::MAX)) as u16,
                    hourly_limit,
                });
            }
        }
        Ok(NotifyDeliveriesResponse {
            deliveries,
            sent_24h,
            failed_24h,
            pending,
            sms_usage,
            retention_days: DELIVERY_RETENTION_DAYS,
        })
    })
    .await?;
    Ok(no_store(response))
}

fn delivery_view(delivery: &StoredDelivery) -> Option<NotifyDeliveryView> {
    Some(NotifyDeliveryView {
        delivery_id: delivery.id.to_string(),
        at_seconds: seconds(delivery.last_attempt_at.unwrap_or(delivery.created_at)),
        kind: delivery.kind,
        target_id: delivery.target_id.map(|id| id.to_string()),
        target_name: delivery.target_name.clone(),
        event: parse_text(&delivery.event)?,
        cluster: delivery.cluster.clone(),
        alert_count: u16::try_from(delivery.alert_count).unwrap_or(u16::MAX),
        state: parse_text(&delivery.state)?,
        attempts: u8::try_from(delivery.attempts).unwrap_or(u8::MAX),
        next_attempt_seconds: delivery
            .next_attempt_at
            .filter(|_| delivery.state == "pending")
            .map(seconds),
        error: delivery.error_class.as_deref().and_then(parse_text),
        provider_code: delivery.provider_code.clone(),
        duration_ms: delivery.duration_ms,
    })
}

fn alert_fact(
    alert: &StoredAlert,
    template: Option<&doctor_history::StoredTemplate>,
    include_template: bool,
) -> AlertFact {
    AlertFact {
        alert_id: alert.id,
        severity: alert.facts.severity.clone(),
        confidence: alert.facts.confidence.clone(),
        evidence: alert.facts.evidence.clone(),
        fingerprint_kind: template.map_or_else(
            || "unofficial".to_owned(),
            |template| template.template.kind.as_str().to_owned(),
        ),
        template_key: alert.template_key.clone(),
        template: template
            .filter(|_| include_template)
            .and_then(|template| template.template.template.clone()),
        baseline_median_ms: alert.facts.baseline_median,
        current_median_ms: alert.facts.current_median,
        relative_permille: alert.facts.relative_permille,
        bucket_start: alert.facts.bucket_start,
        engine_version: alert.facts.engine_version.clone(),
        rule_version: alert.facts.rule_version.clone(),
    }
}

/// Queues one message per enabled target of each enabled channel for the
/// alerts one evaluation raised (and, if chosen, resolved), filtered by the
/// target's lowest severity.
pub(crate) fn enqueue_alert_events(
    store: &HistoryStore,
    target: &str,
    raised: &[i64],
    resolved: &[i64],
    load_raised: &[i64],
    load_resolved: &[i64],
    now: i64,
) -> doctor_history::Result<()> {
    if raised.is_empty()
        && resolved.is_empty()
        && load_raised.is_empty()
        && load_resolved.is_empty()
    {
        return Ok(());
    }
    let channels: Vec<NotifyChannelKind> = store
        .notify_channels()?
        .into_iter()
        .filter(|channel| channel.enabled)
        .map(|channel| channel.kind)
        .collect();
    let recipients: Vec<StoredTarget> = store
        .notify_targets()?
        .into_iter()
        .filter(|recipient| recipient.enabled && channels.contains(&recipient.kind))
        .collect();
    if recipients.is_empty() {
        return Ok(());
    }
    let settings = store.notify_settings()?;
    let templates = store.templates(target)?;
    let facts = |ids: &[i64]| -> doctor_history::Result<Vec<AlertFact>> {
        let mut facts = Vec::new();
        for id in ids {
            if let Some(alert) = store.alert(*id)? {
                let template = templates
                    .iter()
                    .find(|t| t.template.template_key == alert.template_key);
                facts.push(alert_fact(&alert, template, settings.include_template));
            }
        }
        Ok(facts)
    };
    let load_facts = |ids: &[i64]| -> doctor_history::Result<Vec<doctor_notify::LoadAlertFact>> {
        let mut facts = Vec::new();
        for id in ids {
            if let Some(fact) = store
                .load_alert(*id)?
                .as_ref()
                .and_then(crate::load_alerts::notify_fact)
            {
                facts.push(fact);
            }
        }
        Ok(facts)
    };
    let mut events = vec![(
        NotifyEvent::NewAlerts,
        facts(raised)?,
        load_facts(load_raised)?,
    )];
    if settings.notify_resolved {
        events.push((
            NotifyEvent::Resolved,
            facts(resolved)?,
            load_facts(load_resolved)?,
        ));
    }
    let mut queued = Vec::new();
    for (event, alerts, load_alerts) in events {
        for recipient in &recipients {
            let floor = severity_rank(&recipient.min_severity);
            let mut mine: Vec<AlertFact> = alerts
                .iter()
                .filter(|alert| severity_rank(&alert.severity) >= floor)
                .cloned()
                .collect();
            let mine_loads: Vec<doctor_notify::LoadAlertFact> = load_alerts
                .iter()
                .filter(|alert| severity_rank(&alert.severity) >= floor)
                .cloned()
                .collect();
            if (mine.is_empty() && mine_loads.is_empty())
                || (event == NotifyEvent::Resolved && recipient.kind.is_sms())
            {
                continue;
            }
            if recipient.kind.is_sms() {
                // SMS carries counts only; templates never leave in an SMS.
                mine.iter_mut().for_each(|alert| alert.template = None);
            }
            let batch = AlertBatch {
                cluster: target.to_owned(),
                event,
                alerts: mine,
                load_alerts: mine_loads,
            };
            queued.push(NewDelivery {
                kind: recipient.kind,
                target_id: Some(recipient.id),
                target_name: recipient.name.clone(),
                event: text_of(&event),
                cluster: Some(target.to_owned()),
                alert_count: batch.count() as u32,
                payload_json: serde_json::to_string(&batch)
                    .map_err(|_| doctor_history::HistoryError::Internal("notify_payload"))?,
            });
        }
    }
    if !store.enqueue_deliveries(&queued, MAX_PENDING, now)? {
        eprintln!("notify_queue_full pending_limit={MAX_PENDING}");
    }
    Ok(())
}

/// Each outbound channel and whether it would send now: switched on with
/// at least one switched-on target.
pub(crate) fn channel_states(
    store: &HistoryStore,
) -> doctor_history::Result<Vec<(NotifyChannelKind, bool)>> {
    let channels = store.notify_channels()?;
    let targets = store.notify_targets()?;
    Ok(NotifyChannelKind::ALL
        .into_iter()
        .map(|kind| {
            let on = channels
                .iter()
                .any(|channel| channel.kind == kind && channel.enabled);
            (
                kind,
                on && targets
                    .iter()
                    .any(|target| target.kind == kind && target.enabled),
            )
        })
        .collect())
}

fn retryable(error: &SendError) -> bool {
    match error.class {
        NotifyErrorClass::Network
        | NotifyErrorClass::Timeout
        | NotifyErrorClass::InvalidResponse => true,
        // An HTTP 5xx from the provider is worth another try.
        NotifyErrorClass::Rejected => error
            .provider_code
            .as_deref()
            .and_then(|code| code.parse::<u16>().ok())
            .is_some_and(|code| (500..600).contains(&code)),
        _ => false,
    }
}

/// One pass over due deliveries, sent one after another.
pub(crate) fn process_due(
    store: &HistoryStore,
    key: &SecretKey,
    now: i64,
) -> doctor_history::Result<usize> {
    let due = store.due_deliveries(now, WORKER_BATCH)?;
    for delivery in &due {
        let skip = |class: NotifyErrorClass| DeliveryAttempt {
            state: text_of(&NotifyDeliveryState::Skipped),
            next_attempt_at: None,
            error_class: Some(text_of(&class)),
            provider_code: None,
            duration_ms: None,
            attempted: false,
        };
        let channel_on = store
            .notify_channel(delivery.kind)?
            .is_some_and(|channel| channel.enabled);
        let target = delivery
            .target_id
            .map(|id| store.notify_target(id))
            .transpose()?
            .flatten();
        let (Some(target), true) = (target.filter(|target| target.enabled), channel_on) else {
            store.finish_delivery(delivery.id, &skip(NotifyErrorClass::NotConfigured), now)?;
            continue;
        };
        let Ok(batch) = serde_json::from_str::<AlertBatch>(&delivery.payload_json) else {
            store.finish_delivery(delivery.id, &skip(NotifyErrorClass::NotConfigured), now)?;
            continue;
        };
        let secret = match &target.secret {
            Some(_) => match open_target_secret(key, &target) {
                Some(secret) => secret,
                None => {
                    let attempt = DeliveryAttempt {
                        state: text_of(&NotifyDeliveryState::Failed),
                        attempted: false,
                        ..skip(NotifyErrorClass::SecretUnreadable)
                    };
                    store.finish_delivery(delivery.id, &attempt, now)?;
                    continue;
                }
            },
            None => TargetSecret::default(),
        };
        let attempt = match send_once(
            store,
            key,
            delivery.kind,
            target.address.as_deref(),
            &secret,
            &batch,
            now,
        ) {
            Err(NotifyErrorClass::RateLimited) => skip(NotifyErrorClass::RateLimited),
            Err(class) => DeliveryAttempt {
                state: text_of(&NotifyDeliveryState::Failed),
                ..skip(class)
            },
            Ok(Outcome {
                result: Ok(()),
                duration_ms,
            }) => DeliveryAttempt {
                state: text_of(&NotifyDeliveryState::Sent),
                next_attempt_at: None,
                error_class: None,
                provider_code: None,
                duration_ms: Some(duration_ms),
                attempted: true,
            },
            Ok(Outcome {
                result: Err(error),
                duration_ms,
            }) => {
                let attempts = delivery.attempts as usize + 1;
                let retry = retryable(&error)
                    .then(|| RETRY_DELAYS.get(attempts - 1))
                    .flatten();
                DeliveryAttempt {
                    state: text_of(&if retry.is_some() {
                        NotifyDeliveryState::Pending
                    } else {
                        NotifyDeliveryState::Failed
                    }),
                    next_attempt_at: retry.map(|delay| now + delay),
                    error_class: Some(text_of(&error.class)),
                    provider_code: error.provider_code,
                    duration_ms: Some(duration_ms),
                    attempted: true,
                }
            }
        };
        store.finish_delivery(delivery.id, &attempt, now)?;
    }
    Ok(due.len())
}

/// The single delivery worker. It runs while the server runs; each pass is
/// bounded and sends sequentially, so a slow provider delays, never floods.
pub(crate) async fn run_worker(state: ApiState) {
    let (Some(store), Some(key)) = (
        state.history.store().cloned(),
        state.config.notify_key.clone(),
    ) else {
        return;
    };
    let mut ticker = tokio::time::interval(WORKER_PERIOD);
    loop {
        ticker.tick().await;
        let store = Arc::clone(&store);
        let key = key.key.clone();
        let _ = tokio::task::spawn_blocking(move || process_due(&store, &key, unix_now())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::EmailSecurity;
    use doctor_history::AlertFacts;

    fn facts(severity: &str) -> AlertFacts {
        AlertFacts {
            severity: severity.into(),
            confidence: "high".into(),
            evidence: "complete".into(),
            workload: "increased".into(),
            baseline_median: Some(100),
            current_median: Some(400),
            relative_permille: Some(3_000),
            bucket_start: 3_600,
            exemplar: None,
            engine_version: "3.5.14".into(),
            rule_version: "v1".into(),
        }
    }

    fn email_target(store: &HistoryStore, name: &str, min_severity: &str) -> i64 {
        store
            .insert_notify_target(
                &TargetWrite {
                    kind: NotifyChannelKind::Email,
                    name: name.into(),
                    enabled: true,
                    min_severity: min_severity.into(),
                    address: Some(format!("{name}@example.com")),
                    address_hint: format!("{name}@example.com"),
                    secret: SecretUpdate::Clear,
                },
                20,
                1,
            )
            .unwrap()
            .unwrap()
    }

    fn email_channel(store: &HistoryStore, enabled: bool, port: u16) {
        store
            .save_notify_channel(
                &NotifyChannelConfig::Email {
                    host: "127.0.0.1".into(),
                    port,
                    security: EmailSecurity::Plain,
                    username: None,
                    from_address: "alerts@example.com".into(),
                    from_name: None,
                },
                enabled,
                &SecretUpdate::Keep,
                1,
            )
            .unwrap();
    }

    #[test]
    fn n03_new_alerts_are_queued_per_target_by_severity_and_merged() {
        let store = HistoryStore::open_in_memory().unwrap();
        email_channel(&store, true, 25);
        let all = email_target(&store, "dba", "medium");
        let high = email_target(&store, "lead", "high");
        let doctor_history::AlertUpsert::Created(a) =
            store.upsert_alert("t", "k1", &facts("high"), 10).unwrap()
        else {
            panic!()
        };
        let doctor_history::AlertUpsert::Created(b) =
            store.upsert_alert("t", "k2", &facts("medium"), 10).unwrap()
        else {
            panic!()
        };
        enqueue_alert_events(&store, "t", &[a, b], &[], &[], &[], 10).unwrap();
        let rows = store.due_deliveries(10, 10).unwrap();
        assert_eq!(rows.len(), 2);
        let count = |id: i64| {
            rows.iter()
                .find(|row| row.target_id == Some(id))
                .unwrap()
                .alert_count
        };
        assert_eq!((count(all), count(high)), (2, 1));
        let batch: AlertBatch = serde_json::from_str(&rows[0].payload_json).unwrap();
        assert!(batch.alerts.iter().all(|alert| alert.template.is_none()));
        // Resolved alerts are sent only when the user chose it.
        enqueue_alert_events(&store, "t", &[], &[a], &[], &[], 11).unwrap();
        assert_eq!(store.due_deliveries(11, 10).unwrap().len(), 2);
        store
            .set_notify_settings(&NotifySettings {
                notify_resolved: true,
                ..NotifySettings::default()
            })
            .unwrap();
        enqueue_alert_events(&store, "t", &[], &[a], &[], &[], 12).unwrap();
        assert_eq!(store.due_deliveries(12, 10).unwrap().len(), 4);
    }

    #[test]
    fn n03_disabled_channels_and_targets_queue_nothing() {
        let store = HistoryStore::open_in_memory().unwrap();
        email_channel(&store, false, 25);
        email_target(&store, "dba", "medium");
        let doctor_history::AlertUpsert::Created(a) =
            store.upsert_alert("t", "k1", &facts("high"), 10).unwrap()
        else {
            panic!()
        };
        enqueue_alert_events(&store, "t", &[a], &[], &[], &[], 10).unwrap();
        assert!(store.due_deliveries(10, 10).unwrap().is_empty());
    }

    #[test]
    fn n04_failures_retry_on_schedule_then_fail_and_switched_off_targets_skip() {
        let store = HistoryStore::open_in_memory().unwrap();
        let key = SecretKey::generate().unwrap();
        // Nothing listens on this port: every attempt is a network failure.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        email_channel(&store, true, port);
        let id = email_target(&store, "dba", "medium");
        let doctor_history::AlertUpsert::Created(a) =
            store.upsert_alert("t", "k1", &facts("high"), 10).unwrap()
        else {
            panic!()
        };
        enqueue_alert_events(&store, "t", &[a], &[], &[], &[], 100).unwrap();
        let mut now = 100;
        for (delay, state) in [
            (60, "pending"),
            (300, "pending"),
            (1_800, "pending"),
            (0, "failed"),
        ] {
            assert_eq!(process_due(&store, &key, now).unwrap(), 1);
            let row = &store.deliveries(&DeliveryFilter::default(), 1).unwrap()[0];
            assert_eq!(row.state, state);
            assert_eq!(row.error_class.as_deref(), Some("network"));
            if state == "pending" {
                assert_eq!(row.next_attempt_at, Some(now + delay));
                assert_eq!(process_due(&store, &key, now + delay - 1).unwrap(), 0);
                now += delay;
            } else {
                assert_eq!(row.attempts, 4);
            }
        }
        // A target switched off before its turn is skipped, not sent.
        enqueue_alert_events(&store, "t", &[a], &[], &[], &[], now).unwrap();
        let mut write = store.notify_target(id).unwrap().unwrap();
        write.enabled = false;
        store
            .update_notify_target(
                id,
                &TargetWrite {
                    kind: write.kind,
                    name: write.name,
                    enabled: false,
                    min_severity: write.min_severity,
                    address: write.address,
                    address_hint: write.address_hint,
                    secret: SecretUpdate::Keep,
                },
                now,
            )
            .unwrap();
        process_due(&store, &key, now).unwrap();
        let row = &store.deliveries(&DeliveryFilter::default(), 1).unwrap()[0];
        assert_eq!(
            (row.state.as_str(), row.error_class.as_deref(), row.attempts),
            ("skipped", Some("not_configured"), 0)
        );
    }

    #[test]
    fn n04_sms_over_the_hourly_limit_is_skipped() {
        let store = HistoryStore::open_in_memory().unwrap();
        let key = SecretKey::generate().unwrap();
        let config = NotifyChannelConfig::AliyunSms {
            access_key_id: "LTAI".into(),
            sign_name: "x".into(),
            template_code: "SMS_1".into(),
            region: doctor_contracts::AliyunSmsRegion::CnHangzhou,
            hourly_limit: 1,
        };
        store
            .save_notify_channel(
                &config,
                true,
                &SecretUpdate::Set(
                    key.seal(&channel_purpose(NotifyChannelKind::AliyunSms), b"s")
                        .unwrap(),
                ),
                1,
            )
            .unwrap();
        let sealed = key
            .seal(TARGET_PURPOSE, br#"{"phone":"13800001024"}"#)
            .unwrap();
        let id = store
            .insert_notify_target(
                &TargetWrite {
                    kind: NotifyChannelKind::AliyunSms,
                    name: "a".into(),
                    enabled: true,
                    min_severity: "medium".into(),
                    address: None,
                    address_hint: "138****1024".into(),
                    secret: SecretUpdate::Set(sealed),
                },
                20,
                1,
            )
            .unwrap()
            .unwrap();
        let sent = DeliveryAttempt {
            state: "sent".into(),
            next_attempt_at: None,
            error_class: None,
            provider_code: None,
            duration_ms: Some(1),
            attempted: true,
        };
        let record = NewDelivery {
            kind: NotifyChannelKind::AliyunSms,
            target_id: Some(id),
            target_name: "a".into(),
            event: "test".into(),
            cluster: None,
            alert_count: 0,
            payload_json: "{}".into(),
        };
        store.record_delivery(&record, &sent, 1_000).unwrap();
        let doctor_history::AlertUpsert::Created(a) =
            store.upsert_alert("t", "k1", &facts("high"), 10).unwrap()
        else {
            panic!()
        };
        enqueue_alert_events(&store, "t", &[a], &[], &[], &[], 1_500).unwrap();
        process_due(&store, &key, 1_500).unwrap();
        let row = &store.deliveries(&DeliveryFilter::default(), 1).unwrap()[0];
        assert_eq!(
            (row.state.as_str(), row.error_class.as_deref()),
            ("skipped", Some("rate_limited"))
        );
    }

    #[test]
    fn n01_target_secrets_resolve_merge_and_never_store_plaintext() {
        let store = HistoryStore::open_in_memory().unwrap();
        let key = SecretKey::generate().unwrap();
        let input = NotifyTargetInput {
            target_id: None,
            kind: NotifyChannelKind::Feishu,
            name: "DBA 值班群".into(),
            enabled: true,
            min_severity: NotifySeverity::Medium,
            address: None,
            webhook: Some("https://open.feishu.cn/open-apis/bot/v2/hook/abcd-9f2c".into()),
            signing_secret: Some("sign-me".into()),
            clear_signing_secret: false,
        };
        let resolved = resolve_target(&store, &key, &input).unwrap();
        let id = store
            .insert_notify_target(&resolved.write, 20, 1)
            .unwrap()
            .unwrap();
        let stored = store.notify_target(id).unwrap().unwrap();
        assert_eq!(stored.address_hint, "open.feishu.cn/…/****9f2c");
        let blob = stored.secret.clone().unwrap();
        assert!(!String::from_utf8_lossy(&blob).contains("abcd"));
        // An edit without secrets keeps them; a cleared signing secret goes.
        let edit = NotifyTargetInput {
            target_id: Some(id.to_string()),
            webhook: None,
            signing_secret: None,
            clear_signing_secret: true,
            ..input.clone()
        };
        let resolved = resolve_target(&store, &key, &edit).unwrap();
        assert_eq!(
            resolved.secret.webhook.as_deref(),
            Some("https://open.feishu.cn/open-apis/bot/v2/hook/abcd-9f2c")
        );
        assert_eq!(resolved.secret.signing_secret, None);
        // Under another key the saved secret is unreadable, never guessed.
        let other = SecretKey::generate().unwrap();
        assert_eq!(
            resolve_target(&store, &other, &edit).err(),
            Some("notify_secret_unreadable")
        );
        let bad = NotifyTargetInput {
            webhook: Some("https://example.com/hook".into()),
            ..input
        };
        assert_eq!(
            resolve_target(&store, &key, &bad).err(),
            Some("invalid_notify_webhook")
        );
    }
}
