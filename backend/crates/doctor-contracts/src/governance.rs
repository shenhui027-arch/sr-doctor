//! Stage D: in-app alerts (ADR-6) and the governance queue with pinned
//! baseline snapshots and before/after verification (ADR-7). Everything here
//! is local, single-user and literal-free; nothing changes StarRocks.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AuditMetricFact, EvidenceCompleteness, TrendChange, TrendComparability, TrendConfidence,
    TrendNotComparableReason, TrendSeverity, TrendVerdict, WorkloadSignal,
};

pub const GOVERNANCE_NOTE_MAX_CHARS: usize = 500;
pub const GOVERNANCE_TITLE_MAX_CHARS: usize = 120;

fn valid_key(value: &str) -> bool {
    (1..=256).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_id(value: &str) -> bool {
    (1..=19).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max
        && !value.chars().any(|c| c.is_control() && c != '\n')
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AlertState {
    Open,
    Acknowledged,
    /// Back inside the baseline band; can be reopened by hand.
    Resolved,
    Muted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AlertAction {
    Acknowledge,
    /// Silence for `ALERT_MUTE_HOURS`; no new alert for the template meanwhile.
    Mute,
    Reopen,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryAlertsRequest {
    pub target: Option<String>,
}

impl HistoryAlertsRequest {
    pub fn validate(&self) -> bool {
        self.target.as_deref().is_none_or(valid_key)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AlertActionRequest {
    pub alert_id: String,
    pub action: AlertAction,
}

impl AlertActionRequest {
    pub fn validate(&self) -> bool {
        valid_id(&self.alert_id)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AlertView {
    pub alert_id: String,
    pub target: String,
    pub template_key: String,
    pub template: Option<String>,
    pub kind: String,
    pub state: AlertState,
    pub muted_until_seconds: Option<u32>,
    pub first_seen_seconds: u32,
    pub last_seen_seconds: u32,
    pub resolved_seconds: Option<u32>,
    pub severity: TrendSeverity,
    pub confidence: TrendConfidence,
    pub evidence: EvidenceCompleteness,
    pub workload: WorkloadSignal,
    pub baseline_median: Option<AuditMetricFact>,
    pub current_median: Option<AuditMetricFact>,
    pub change: Option<TrendChange>,
    /// The newest current-window hour of the regression.
    pub bucket_start_seconds: u32,
    pub exemplar_query_id: Option<String>,
    pub comparability: TrendComparability,
    pub governance_item_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AlertCounts {
    pub open: String,
    pub acknowledged: String,
    pub resolved: String,
    pub muted: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AlertChannel {
    /// `in_app`, `webhook` or `email`.
    pub name: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AlertRule {
    /// Only regressions at or above this severity raise an alert.
    pub min_severity: TrendSeverity,
    pub mute_hours: u16,
    pub current_hours: u16,
    pub baseline_hours: u16,
    pub calibrated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryAlertsResponse {
    pub target: Option<String>,
    /// Newest first.
    pub alerts: Vec<AlertView>,
    pub counts: AlertCounts,
    pub channels: Vec<AlertChannel>,
    pub rule: AlertRule,
    pub limitations: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum GovernanceState {
    New,
    Triaged,
    InProgress,
    /// Entered only by recording a change; left only by a comparator result
    /// or by going back to work / not handling it.
    PendingVerification,
    /// Entered only by a comparator result of `improved`.
    Closed,
    WontFix,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum GovernanceOrigin {
    Alert,
    Trends,
    Manual,
}

/// The "before" side, pinned by the user: one template's current window at
/// pin time. Literal-free by construction; no Profile text.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceSnapshot {
    pub pinned_seconds: u32,
    pub template: Option<String>,
    pub window_start_seconds: u32,
    pub window_end_seconds: u32,
    pub comparability: TrendComparability,
    pub executions: String,
    pub valid_samples: String,
    pub query_time_median: Option<AuditMetricFact>,
    pub scan_bytes_median: Option<AuditMetricFact>,
    pub cpu_cost_median: Option<AuditMetricFact>,
    pub exemplar_query_id: Option<String>,
    pub verdict: Option<TrendVerdict>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[ts(tag = "state", rename_all = "snake_case")]
pub enum VerificationOutcome {
    /// Median query time fell by at least the rule's floor without more work.
    Improved,
    NotImproved,
    InsufficientSamples,
    WorkloadSignalMissing,
    NotComparable {
        reason: TrendNotComparableReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct VerificationMetric {
    /// `query_time`, `scan_bytes` or `cpu_cost`.
    pub metric: String,
    pub before: Option<AuditMetricFact>,
    pub after: Option<AuditMetricFact>,
    pub change: Option<TrendChange>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceVerification {
    pub verified_seconds: u32,
    pub after_start_seconds: u32,
    pub after_end_seconds: u32,
    pub outcome: VerificationOutcome,
    pub metrics: Vec<VerificationMetric>,
    pub executions_before: String,
    pub executions_after: String,
    pub confidence: Option<TrendConfidence>,
    pub evidence: EvidenceCompleteness,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceEvent {
    pub at_seconds: u32,
    /// `created`, `transition`, `pinned`, `change_recorded` or `verified`.
    pub kind: String,
    pub from: Option<GovernanceState>,
    pub to: GovernanceState,
    /// `local_user` or `comparator`.
    pub actor: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceItem {
    pub item_id: String,
    pub target: String,
    pub template_key: String,
    pub template: Option<String>,
    pub kind: String,
    pub title: String,
    pub origin: GovernanceOrigin,
    pub alert_id: Option<String>,
    pub state: GovernanceState,
    pub created_seconds: u32,
    pub updated_seconds: u32,
    /// When the user says the change outside this tool took effect.
    pub change_seconds: Option<u32>,
    pub snapshot: Option<GovernanceSnapshot>,
    pub verification: Option<GovernanceVerification>,
    /// Oldest first.
    pub events: Vec<GovernanceEvent>,
    /// States the user may move to by hand from here.
    pub allowed_transitions: Vec<GovernanceState>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceListRequest {
    pub target: Option<String>,
}

impl GovernanceListRequest {
    pub fn validate(&self) -> bool {
        self.target.as_deref().is_none_or(valid_key)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceListResponse {
    /// Open work first, newest first within a state.
    pub items: Vec<GovernanceItem>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceCreateRequest {
    pub target: String,
    pub template_key: String,
    pub origin: GovernanceOrigin,
    pub alert_id: Option<String>,
    pub title: Option<String>,
}

impl GovernanceCreateRequest {
    pub fn validate(&self) -> bool {
        valid_key(&self.target)
            && valid_key(&self.template_key)
            && (self.origin == GovernanceOrigin::Alert) == self.alert_id.is_some()
            && self.alert_id.as_deref().is_none_or(valid_id)
            && self
                .title
                .as_deref()
                .is_none_or(|title| valid_text(title, GOVERNANCE_TITLE_MAX_CHARS))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceTransitionRequest {
    pub item_id: String,
    pub to: GovernanceState,
    pub note: Option<String>,
}

impl GovernanceTransitionRequest {
    pub fn validate(&self) -> bool {
        valid_id(&self.item_id)
            && self
                .note
                .as_deref()
                .is_none_or(|note| valid_text(note, GOVERNANCE_NOTE_MAX_CHARS))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceItemRequest {
    pub item_id: String,
}

impl GovernanceItemRequest {
    pub fn validate(&self) -> bool {
        valid_id(&self.item_id)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(deny_unknown_fields)]
pub struct GovernanceChangeRequest {
    pub item_id: String,
    /// What was changed outside this tool; never SQL text.
    pub note: String,
    /// Defaults to now; never in the future.
    pub change_seconds: Option<u32>,
}

impl GovernanceChangeRequest {
    pub fn validate(&self) -> bool {
        valid_id(&self.item_id) && valid_text(&self.note, GOVERNANCE_NOTE_MAX_CHARS)
    }
}

/// The hand-driven transitions (ADR-7). `pending_verification` is entered
/// only by recording a change and `closed` only by a comparator result.
pub fn governance_manual_transitions(from: GovernanceState) -> &'static [GovernanceState] {
    use GovernanceState::*;
    match from {
        New => &[Triaged, WontFix],
        Triaged => &[InProgress, WontFix],
        InProgress => &[WontFix],
        PendingVerification => &[InProgress, WontFix],
        Closed => &[InProgress],
        WontFix => &[Triaged],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rd02_closed_and_pending_are_never_reachable_by_hand() {
        use GovernanceState::*;
        for from in [
            New,
            Triaged,
            InProgress,
            PendingVerification,
            Closed,
            WontFix,
        ] {
            let allowed = governance_manual_transitions(from);
            assert!(!allowed.contains(&Closed), "{from:?}");
            assert!(!allowed.contains(&PendingVerification), "{from:?}");
            assert!(!allowed.contains(&from), "{from:?}");
        }
    }

    #[test]
    fn requests_are_bounded() {
        let create =
            |origin, alert_id: Option<&str>, title: Option<&str>| GovernanceCreateRequest {
                target: "127.0.0.1:9030".into(),
                template_key: "history-template-v1:a".into(),
                origin,
                alert_id: alert_id.map(str::to_owned),
                title: title.map(str::to_owned),
            };
        assert!(create(GovernanceOrigin::Alert, Some("7"), None).validate());
        assert!(!create(GovernanceOrigin::Alert, None, None).validate());
        assert!(!create(GovernanceOrigin::Trends, Some("7"), None).validate());
        assert!(!create(GovernanceOrigin::Manual, None, Some(&"x".repeat(121))).validate());
        assert!(!create(GovernanceOrigin::Manual, None, Some("  ")).validate());
        let change = |note: &str| GovernanceChangeRequest {
            item_id: "1".into(),
            note: note.into(),
            change_seconds: None,
        };
        assert!(change("Added a partition on region").validate());
        assert!(!change(&"x".repeat(501)).validate());
        assert!(!change("bad\u{0007}").validate());
        assert!(
            !AlertActionRequest {
                alert_id: "-1".into(),
                action: AlertAction::Mute
            }
            .validate()
        );
    }
}
