//! Outbound alert notifications (v1.0.1): channels say how to send (an SMTP
//! server, a cloud SMS account), targets say to whom (mailboxes, group bots,
//! phone numbers). Secrets are write-only: a view only says whether one is
//! saved. Messages carry alert facts, never SQL literals or Profile text.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const NOTIFY_MAX_TARGETS_PER_CHANNEL: u16 = 20;
pub const NOTIFY_TARGET_NAME_MAX_CHARS: usize = 40;

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifyChannelKind {
    Email,
    Feishu,
    Dingtalk,
    AliyunSms,
    TencentSms,
}

impl NotifyChannelKind {
    pub const ALL: [Self; 5] = [
        Self::Email,
        Self::Feishu,
        Self::Dingtalk,
        Self::AliyunSms,
        Self::TencentSms,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Feishu => "feishu",
            Self::Dingtalk => "dingtalk",
            Self::AliyunSms => "aliyun_sms",
            Self::TencentSms => "tencent_sms",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    pub fn is_sms(self) -> bool {
        matches!(self, Self::AliyunSms | Self::TencentSms)
    }

    pub fn is_bot(self) -> bool {
        matches!(self, Self::Feishu | Self::Dingtalk)
    }
}

/// The lowest alert severity a target receives.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifySeverity {
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifyLanguage {
    Zh,
    En,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifySettings {
    /// Also send when an alert is back inside its baseline band.
    pub notify_resolved: bool,
    /// Add the literal-free SQL template to email and bot messages (never SMS).
    pub include_template: bool,
    pub language: NotifyLanguage,
    /// Base address for the "view alerts" link, e.g. `http://10.0.0.5:8080`.
    pub public_url: Option<String>,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            notify_resolved: false,
            include_template: false,
            language: NotifyLanguage::Zh,
            public_url: None,
        }
    }
}

/// TLS modes always verify the server certificate; there is no
/// encrypt-without-verify mode.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum EmailSecurity {
    /// Implicit TLS, usually port 465.
    Tls,
    /// Plain connection upgraded with STARTTLS, usually port 587.
    Starttls,
    /// No encryption: password and message cross the network in clear text.
    Plain,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AliyunSmsRegion {
    CnHangzhou,
    ApSoutheast1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TencentSmsRegion {
    ApGuangzhou,
    ApBeijing,
    ApNanjing,
}

/// Non-secret channel settings. The channel secret (SMTP password, SMS
/// AccessKey Secret / SecretKey) travels separately and is never returned.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "kind", rename_all = "snake_case")]
pub enum NotifyChannelConfig {
    Email {
        host: String,
        port: u16,
        security: EmailSecurity,
        /// Empty: no SMTP authentication.
        username: Option<String>,
        from_address: String,
        from_name: Option<String>,
    },
    /// Every Feishu target carries its own webhook and signing secret.
    Feishu {},
    /// Every DingTalk target carries its own webhook and signing secret.
    Dingtalk {},
    AliyunSms {
        access_key_id: String,
        sign_name: String,
        template_code: String,
        region: AliyunSmsRegion,
        hourly_limit: u16,
    },
    TencentSms {
        secret_id: String,
        sdk_app_id: String,
        sign_name: String,
        template_id: String,
        region: TencentSmsRegion,
        hourly_limit: u16,
    },
}

impl NotifyChannelConfig {
    pub fn kind(&self) -> NotifyChannelKind {
        match self {
            Self::Email { .. } => NotifyChannelKind::Email,
            Self::Feishu {} => NotifyChannelKind::Feishu,
            Self::Dingtalk {} => NotifyChannelKind::Dingtalk,
            Self::AliyunSms { .. } => NotifyChannelKind::AliyunSms,
            Self::TencentSms { .. } => NotifyChannelKind::TencentSms,
        }
    }

    pub fn hourly_limit(&self) -> Option<u16> {
        match self {
            Self::AliyunSms { hourly_limit, .. } | Self::TencentSms { hourly_limit, .. } => {
                Some(*hourly_limit)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyChannelSaveRequest {
    pub config: NotifyChannelConfig,
    pub enabled: bool,
    /// A new channel secret; `None` keeps the saved one.
    pub secret: Option<String>,
    pub clear_secret: bool,
}

/// A target to create (`target_id: None`), update, or test without saving.
/// Secret fields left `None` keep what is saved.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyTargetInput {
    pub target_id: Option<String>,
    pub kind: NotifyChannelKind,
    pub name: String,
    pub enabled: bool,
    pub min_severity: NotifySeverity,
    /// Email address (shown in full) or phone number (stored encrypted,
    /// shown masked). `None` keeps the saved phone number.
    pub address: Option<String>,
    /// Feishu / DingTalk webhook, stored encrypted.
    pub webhook: Option<String>,
    pub signing_secret: Option<String>,
    pub clear_signing_secret: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyTargetDeleteRequest {
    pub target_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyTestRequest {
    pub target: NotifyTargetInput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifyEvent {
    NewAlerts,
    Resolved,
    Test,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifyDeliveryState {
    /// Waiting for its first attempt or a retry.
    Pending,
    Sent,
    /// Every attempt failed; no further retry.
    Failed,
    /// Not sent: over the SMS hourly limit, or the channel or target was
    /// switched off before it went out.
    Skipped,
}

/// Error categories only; provider messages and response bodies are never kept.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifyErrorClass {
    Network,
    Timeout,
    Tls,
    AuthFailed,
    /// The provider answered and refused; `provider_code` says why.
    Rejected,
    InvalidResponse,
    RateLimited,
    NotConfigured,
    /// Saved under another master key: the secret must be entered again.
    SecretUnreadable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyDeliveryBrief {
    pub at_seconds: u32,
    pub state: NotifyDeliveryState,
    pub error: Option<NotifyErrorClass>,
    pub provider_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyChannelView {
    pub kind: NotifyChannelKind,
    pub enabled: bool,
    /// `None` until saved once (always present for bot channels).
    pub config: Option<NotifyChannelConfig>,
    pub secret_saved: bool,
    pub secret_unreadable: bool,
    pub targets: u16,
    pub enabled_targets: u16,
    /// Targets whose latest delivery failed.
    pub failing_targets: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyTargetView {
    pub target_id: String,
    pub kind: NotifyChannelKind,
    pub name: String,
    pub enabled: bool,
    pub min_severity: NotifySeverity,
    /// The address in full for email; masked for phones and webhooks.
    pub address_hint: String,
    pub signing_secret_saved: bool,
    pub secret_unreadable: bool,
    pub last: Option<NotifyDeliveryBrief>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum NotifyKeySource {
    /// `DOCTOR_SECRET_KEY`.
    Environment,
    /// `secret.key` beside the history file.
    File,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyOverviewResponse {
    pub settings: NotifySettings,
    pub channels: Vec<NotifyChannelView>,
    /// Grouped by channel, then by name.
    pub targets: Vec<NotifyTargetView>,
    /// Remote clusters raise no alert (so send nothing) until the G1 pilot
    /// passes and `DOCTOR_COLLECTION_ALLOW_REMOTE=after-g1` is set.
    pub remote_collection_allowed: bool,
    pub key_source: NotifyKeySource,
    pub max_targets_per_channel: u16,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyTestResponse {
    pub state: NotifyDeliveryState,
    pub error: Option<NotifyErrorClass>,
    pub provider_code: Option<String>,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyDeliveriesRequest {
    pub kind: Option<NotifyChannelKind>,
    pub target_id: Option<String>,
    pub state: Option<NotifyDeliveryState>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyDeliveryView {
    pub delivery_id: String,
    pub at_seconds: u32,
    pub kind: NotifyChannelKind,
    /// `None` after the target was deleted, or for a test of an unsaved one.
    pub target_id: Option<String>,
    pub target_name: String,
    pub event: NotifyEvent,
    /// The cluster ("host:port") the alerts came from; `None` for tests.
    pub cluster: Option<String>,
    pub alert_count: u16,
    pub state: NotifyDeliveryState,
    pub attempts: u8,
    pub next_attempt_seconds: Option<u32>,
    pub error: Option<NotifyErrorClass>,
    pub provider_code: Option<String>,
    pub duration_ms: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifySmsUsage {
    pub kind: NotifyChannelKind,
    /// Messages sent in the last 60 minutes.
    pub used: u16,
    pub hourly_limit: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct NotifyDeliveriesResponse {
    /// Newest first, at most 200.
    pub deliveries: Vec<NotifyDeliveryView>,
    pub sent_24h: u32,
    pub failed_24h: u32,
    pub pending: u32,
    pub sms_usage: Vec<NotifySmsUsage>,
    pub retention_days: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_config_is_tagged_by_kind() {
        let config: NotifyChannelConfig = serde_json::from_str(r#"{"kind":"feishu"}"#).unwrap();
        assert_eq!(config.kind(), NotifyChannelKind::Feishu);
        let email = serde_json::json!({
            "kind": "email", "host": "smtp.example.com", "port": 465, "security": "tls",
            "username": null, "from_address": "a@example.com", "from_name": null
        });
        let config: NotifyChannelConfig = serde_json::from_value(email).unwrap();
        assert_eq!(config.kind(), NotifyChannelKind::Email);
        assert!(serde_json::from_str::<NotifyChannelConfig>(r#"{"kind":"feishu","x":1}"#).is_err());
    }

    #[test]
    fn kinds_round_trip_their_storage_names() {
        for kind in NotifyChannelKind::ALL {
            assert_eq!(NotifyChannelKind::parse(kind.as_str()), Some(kind));
        }
    }
}
