// Design board 06 (告警): in-app alerts raised from stage C regressions. The
// server decides every alert, its state and its evidence; this only renders
// and sends the user's acknowledge / mute / reopen / add-to-queue choices,
// and the manual confirmation dialog for resolving older-rule alerts.
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  AlertState,
  AlertView,
  HistoryAlertsResponse,
  NotifyChannelKind,
} from "../../../../contracts/generated/doctor-contracts";
import {
  BadgeStamp,
  Button,
  Card,
  Dialog,
  EvidenceMeter,
  InfoTip,
  SegTabs,
  StateStamp,
  type EvidenceLevel,
} from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { AuditMetricValue } from "../audit/AuditMetricValue";
import { metricText } from "../audit/audit-display";
import { clockText } from "./CollectionPanel";
import { NOTIFY_KINDS, kindName } from "./notify-text";
import "./alerts.css";

type AlertTab = AlertState | "all";
const TABS: AlertTab[] = ["open", "acknowledged", "resolved", "muted", "all"];

const ALERT_STATE_LABELS: Record<AlertState, readonly [string, string]> = {
  open: ["New", "新告警"],
  acknowledged: ["Acknowledged", "已确认"],
  resolved: ["Resolved", "已解决"],
  muted: ["Muted", "已静默"],
};

const LEVEL_LABELS: Record<"high" | "medium" | "low", readonly [string, string]> = {
  high: ["high", "高"],
  medium: ["medium", "中"],
  low: ["low", "低"],
};

const EVIDENCE_LABELS: Record<AlertView["evidence"], readonly [string, string]> = {
  complete: ["complete", "完整"],
  partial: ["partial", "部分"],
  unavailable: ["unavailable", "不可用"],
};

const KIND_LABELS: Record<AlertView["kind"], readonly [string, string]> = {
  official_digest: ["official digest", "官方 digest"],
  unofficial_fingerprint: ["unofficial fingerprint", "非官方指纹"],
};

export function alertStateText(state: AlertState, text: LocalizedText): string {
  return lookupEnumLabel(ALERT_STATE_LABELS, state, text);
}

export function alertTitle(alert: AlertView, text: LocalizedText): string {
  const change = alert.change;
  if (!change) return text("Median query time regression", "查询耗时中位数回归");
  // The absolute change leads; the percentage is secondary and undefined for a 0 baseline.
  const absolute = `${metricText(alert.baseline_median)} → ${metricText(alert.current_median)}`;
  const relative =
    change.relative_percent === null
      ? text(" (baseline 0, no percentage)", "（基线为 0，百分比不定义）")
      : text(` (+${change.relative_percent}%)`, `（+${change.relative_percent}%）`);
  return text(`Median query time ${absolute}${relative}`, `查询耗时中位数 ${absolute}${relative}`);
}

const level = (value: "high" | "medium" | "low", text: LocalizedText) =>
  lookupEnumLabel(LEVEL_LABELS, value, text);

const evidenceText = (value: AlertView["evidence"], text: LocalizedText) =>
  lookupEnumLabel(EVIDENCE_LABELS, value, text);

function toMeterLevel(evidence: AlertView["evidence"]): EvidenceLevel {
  if (evidence === "complete") return "complete";
  if (evidence === "partial") return "partial";
  return "missing";
}

function severityDotClass(severity: string): string {
  if (severity === "high") return "status-dot status-dot--critical";
  if (severity === "medium") return "status-dot status-dot--warn";
  return "status-dot status-dot--hollow";
}

function stateDotClass(state: AlertState): string {
  if (state === "open") return "status-dot status-dot--critical";
  if (state === "acknowledged") return "status-dot status-dot--warn";
  if (state === "resolved") return "status-dot status-dot--ok";
  return "status-dot status-dot--hollow";
}

export function AlertsPanel({
  alerts,
  busy,
  selectedId,
  onSelect,
  onAction,
  onResolveRuleChanged,
  onOpenTrends,
  onAddToQueue,
  onOpenItem,
  toolbarLeading,
  toolbar,
  initialConfirmResolveOpen = false,
  text,
}: {
  alerts: HistoryAlertsResponse;
  /** View and category switches at the start of the unified toolbar. */
  toolbarLeading?: ReactNode;
  /** Target and refresh controls, shown at the end of the unified toolbar. */
  toolbar?: ReactNode;
  busy: boolean;
  selectedId: string | null;
  onSelect(id: string): void;
  onAction(id: string, action: "acknowledge" | "mute" | "reopen"): void;
  onResolveRuleChanged(): void;
  onOpenTrends(templateKey: string): void;
  onAddToQueue(alert: AlertView): void;
  onOpenItem(itemId: string): void;
  initialConfirmResolveOpen?: boolean;
  text: LocalizedText;
}): JSX.Element {
  // With nothing new, open on everything so acknowledged alerts stay in view.
  const [tab, setTab] = useState<AlertTab>(alerts.counts.open === "0" ? "all" : "open");
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [confirmResolveOpen, setConfirmResolveOpen] = useState(initialConfirmResolveOpen);
  const { rule_changed: ruleChanged, ...stateCounts } = alerts.counts;
  const counts: Record<AlertTab, string> = { ...stateCounts, all: String(alerts.alerts.length) };
  const visible = alerts.alerts.filter((alert) => tab === "all" || alert.state === tab);
  const selected = visible.find((alert) => alert.alert_id === selectedId) ?? visible[0] ?? null;

  return (
    <div className="alert-stage-view">
      <div className="workspace-bar">
        {toolbarLeading}
        <div className="workspace-bar-right">
          <SegTabs<AlertTab>
            aria-label={text("Alert states", "告警状态")}
            items={TABS.map((item) => ({
              value: item,
              label: item === "all" ? text("All", "全部") : alertStateText(item, text),
              count: counts[item],
            }))}
            onValueChange={(next) => {
              setTab(next);
              setDrawerOpen(false);
            }}
            size="sm"
            value={tab}
          />
          <InfoTip label={text("Which changes raise an alert", "哪些变化会告警")}>
            {text(
              "Only changes that can be judged raise an alert: enough samples, the same StarRocks version and rule set on both sides.",
              "仅显示可判断的性能变化：样本足够，且前后 StarRocks 版本与规则集一致。",
            )}
          </InfoTip>
          {toolbar}
        </div>
      </div>

      {ruleChanged !== "0" && (
        <>
          <div className="notice-banner notice-banner--warn rule-changed-banner" role="status">
            <span className="rule-changed-banner-left">
              <span>
                {text(
                  ruleChanged === "1"
                    ? "1 alert from an older rule requires manual resolution."
                    : `${ruleChanged} alerts from older rules require manual resolution.`,
                  `有 ${ruleChanged} 条旧规则告警需手动确认关闭。`,
                )}
              </span>
              <InfoTip label={text("Why older-rule alerts stay open", "为什么旧规则告警需手动关闭")}>
                {text(
                  "Raised under the rules before an upgrade. The current rules cannot compare them, so they will not resolve automatically.",
                  "来自升级前的规则版本；当前规则无法与之自动比较，因此不会随趋势恢复而自动解决。",
                )}
              </InfoTip>
            </span>
            <Button disabled={busy} onClick={() => setConfirmResolveOpen(true)} size="sm" variant="secondary">
              {text("Mark them resolved", "全部标为已解决")}
            </Button>
          </div>

          <Dialog
            description={text(
              `This marks ${ruleChanged} older-rule alert(s) on ${alerts.target ?? "this cluster"} as resolved.`,
              `将把集群 ${alerts.target ?? "当前集群"} 上的 ${ruleChanged} 条旧规则告警标为“已解决”。`,
            )}
            footer={
              <>
                <Button onClick={() => setConfirmResolveOpen(false)} size="sm" variant="secondary">
                  {text("Cancel", "取消")}
                </Button>
                <Button
                  disabled={busy}
                  onClick={() => {
                    setConfirmResolveOpen(false);
                    onResolveRuleChanged();
                  }}
                  size="sm"
                  variant="primary"
                >
                  {text("Confirm resolve", "确认标为已解决")}
                </Button>
              </>
            }
            onOpenChange={setConfirmResolveOpen}
            open={confirmResolveOpen}
            title={text("Mark older-rule alerts resolved", "确认关闭旧规则告警")}
            width="md"
          >
            <p>
              {text(
                "Only open or acknowledged alerts flagged with rule_changed (raised before the rule upgrade) will be marked resolved. Alerts raised under the current rule set are not affected.",
                "仅将升级前旧规则产生（标记为“规则已变更”）的未解决告警标为“已解决”；当前规则集产生的新告警不受影响。",
              )}
            </p>
          </Dialog>
        </>
      )}

      <Card
        actions={
          !alerts.rule.calibrated ? (
            <span className="state-stamp state-stamp--warn">
              {text("Trial thresholds · uncalibrated", "试运行 · 阈值未经真实业务校准")}
            </span>
          ) : undefined
        }
        aria-labelledby="alert-list"
        footer={
          <details className="disclosure alert-rules-channels">
            <summary>{text("Alert rules and notification channels", "告警规则与通知渠道")}</summary>
            <div className="disclosure-body">
              <h3 className="detail-sec-title">{text("Outbound notification status", "外发通知渠道状态")}</h3>
              <dl className="alert-channels-grid">
                {alerts.channels.map((channel) => (
                  <FragmentChannel channel={channel} key={channel.name} text={text} />
                ))}
              </dl>

              <h3 className="detail-sec-title">{text("Alert rules", "告警判定规则")}</h3>
              <ul className="alert-rules-list">
                <li>
                  {text(
                    `A template's verdict is a regression of at least ${level(alerts.rule.min_severity, text)} severity (last ${alerts.rule.current_hours} hours against the ${alerts.rule.baseline_hours / 24} days before)`,
                    `模板判定为“回归”，且严重度不低于“${level(alerts.rule.min_severity, text)}”（最近 ${alerts.rule.current_hours} 小时对比之前 ${alerts.rule.baseline_hours / 24} 天）`,
                  )}
                </li>
                <li>{text("Too few samples or a version change raises no alert", "样本不足或版本变化时不产生告警")}</li>
                <li>{text("One alert per template until it is back inside the baseline band", "同一模板在恢复到基线带前只产生一条告警")}</li>
                <li>{text("Back inside the band it is resolved automatically and can be reopened by hand", "恢复到基线带后自动标为已解决，可手动重新打开")}</li>
                <li>
                  {alerts.rule.calibrated
                    ? text("Latency alone never decides.", "不以耗时单独判定。")
                    : text(
                        "Latency alone never decides; the thresholds are on trial until calibrated on real workload.",
                        "不以耗时单独判定；阈值尚未用真实业务分布校准，处于试运行。",
                      )}
                </li>
              </ul>
            </div>
          </details>
        }
        title={<span id="alert-list">{text("Alerts", "告警")}</span>}
        variant="list"
      >
        {visible.length === 0 ? (
          <div className="state-box">{text("No alert in this state.", "这个状态下没有告警。")}</div>
        ) : (
          <ul className="item-list">
            {visible.map((alert) => {
              const isSelected = alert.alert_id === selected?.alert_id;
              return (
                <li key={alert.alert_id}>
                  <button
                    aria-haspopup="dialog"
                    className={isSelected ? "item-btn is-selected" : "item-btn"}
                    onClick={() => {
                      onSelect(alert.alert_id);
                      setDrawerOpen(true);
                    }}
                    type="button"
                  >
                    <div className="item-row-top">
                      <span className="status-inline">
                        <span className={severityDotClass(alert.severity)} />
                        <strong className="item-row-title">{alertTitle(alert, text)}</strong>
                      </span>
                      <span className="status-inline">
                        <span className={stateDotClass(alert.state)} />
                        {alertStateText(alert.state, text)}
                      </span>
                    </div>
                    <div className="item-row-meta">
                      <code className="mono">{alert.template ?? alert.template_key}</code>
                    </div>
                    <div className="item-row-meta">
                      <span className="mono" title={text("Last seen", "最近触发")}>
                        {text("Last", "最近")} {clockText(alert.last_seen_seconds).slice(5, 16)}
                      </span>
                      <span>{text("Severity", "严重度")} {level(alert.severity, text)}</span>
                      <span>{text("Confidence", "置信度")} {level(alert.confidence, text)}</span>
                      {alert.rule_changed && (
                        <span className="state-stamp state-stamp--warn rule-changed">
                          {text("Rules changed", "规则已变更")}
                        </span>
                      )}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      {selected && (
        <AlertDetailDrawer
          alert={selected}
          busy={busy}
          muteHours={alerts.rule.mute_hours}
          onAction={onAction}
          onAddToQueue={onAddToQueue}
          onClose={() => setDrawerOpen(false)}
          onOpenItem={(id) => {
            setDrawerOpen(false);
            onOpenItem(id);
          }}
          onOpenTrends={(key) => {
            setDrawerOpen(false);
            onOpenTrends(key);
          }}
          open={drawerOpen}
          text={text}
        />
      )}
    </div>
  );
}

export function AlertDetailDrawer({
  alert,
  busy,
  muteHours,
  onAction,
  onAddToQueue,
  onClose,
  onOpenItem,
  onOpenTrends,
  open,
  text,
}: {
  alert: AlertView;
  busy: boolean;
  muteHours: HistoryAlertsResponse["rule"]["mute_hours"];
  onAction(id: string, action: "acknowledge" | "mute" | "reopen"): void;
  onAddToQueue(alert: AlertView): void;
  onClose(): void;
  onOpenItem(itemId: string): void;
  onOpenTrends(templateKey: string): void;
  open: boolean;
  text: LocalizedText;
}): JSX.Element {
  return (
    <Dialog
      footer={
        <div className="alert-footer-actions">
          <div className="alert-footer-group">
            {alert.state === "open" && (
              <Button disabled={busy} onClick={() => onAction(alert.alert_id, "acknowledge")} size="sm" variant="primary">
                {text("Acknowledge", "确认")}
              </Button>
            )}
            {alert.governance_item_id ? (
              <Button onClick={() => onOpenItem(alert.governance_item_id!)} size="sm" variant="secondary">
                {text(`Open governance item #${alert.governance_item_id}`, `查看治理项 #${alert.governance_item_id}`)}
              </Button>
            ) : (
              <Button disabled={busy} onClick={() => onAddToQueue(alert)} size="sm" variant="secondary">
                {text("Add to governance queue", "加入治理队列")}
              </Button>
            )}
            {(alert.state === "open" || alert.state === "acknowledged") && (
              <Button disabled={busy} onClick={() => onAction(alert.alert_id, "mute")} size="sm" variant="secondary">
                {text(`Mute ${muteHours} hours`, `静默 ${muteHours} 小时`)}
              </Button>
            )}
            {alert.state === "resolved" && (
              <Button disabled={busy} onClick={() => onAction(alert.alert_id, "reopen")} size="sm" variant="secondary">
                {text("Reopen", "重新打开")}
              </Button>
            )}
          </div>
          <Button onClick={() => onOpenTrends(alert.template_key)} size="sm" variant="ghost">
            {text("View trend and representative executions", "查看趋势与代表执行")}
          </Button>
        </div>
      }
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      open={open}
      placement="right"
      title={alertTitle(alert, text)}
      width="lg"
    >
      <div className="detail-stack">
        <p className="mono muted">
          {text("First seen", "首次触发")} {clockText(alert.first_seen_seconds).slice(5, 16)} · {text("last seen", "最近触发")}{" "}
          {clockText(alert.last_seen_seconds).slice(5, 16)}
        </p>

        <div className="fact-pills">
          <span className="status-inline">
            <span className={severityDotClass(alert.severity)} />
            {text("Severity", "严重度")} {level(alert.severity, text)}
          </span>
          <span>{text("Confidence", "置信度")} {level(alert.confidence, text)}</span>
          <EvidenceMeter
            label={`${text("Evidence", "证据")} ${evidenceText(alert.evidence, text)}`}
            level={toMeterLevel(alert.evidence)}
          />
          {alert.workload === "stable" && (
            <span>{text("Workload unchanged: possibly contention", "工作量未变：可能是资源争用")}</span>
          )}
        </div>

        <h3 className="detail-sec-title">{text("Evidence chain", "证据链")}</h3>
        <dl className="kv-grid">
          <dt>{text("Template", "模板")}</dt>
          <dd>
            <div className="sql-well">{alert.template ?? "—"}</div>
            <BadgeStamp kind={alert.kind === "official_digest" ? "official_digest" : "unofficial_fingerprint"} />{" "}
            <span className="muted">({lookupEnumLabel(KIND_LABELS, alert.kind, text)})</span>
          </dd>
          <dt>{text("Compared hour (UTC)", "比较时间桶（UTC）")}</dt>
          <dd className="mono">
            {clockText(alert.bucket_start_seconds).slice(5, 16)}–{clockText(alert.bucket_start_seconds + 3600).slice(11, 16)} ·{" "}
            <AuditMetricValue text={text} value={alert.baseline_median} /> →{" "}
            <AuditMetricValue text={text} value={alert.current_median} />
          </dd>
          <dt>{text("Representative", "代表执行")}</dt>
          <dd className="mono">{alert.exemplar_query_id ?? <StateStamp state="missing" />}</dd>
          <dt>{text("Comparability", "可比性")}</dt>
          <dd className="mono">
            StarRocks {alert.comparability.engine_version} · {alert.comparability.rule_version}
            {alert.rule_changed && (
              <>
                {" · "}
                <span className="state-stamp state-stamp--warn" data-honest-state="not_comparable">
                  ≠ {text("rules changed since; not comparable", "规则已变更，无法比较")}
                </span>
              </>
            )}
          </dd>
          {alert.muted_until_seconds !== null && (
            <>
              <dt>{text("Muted until", "静默至")}</dt>
              <dd className="mono">{clockText(alert.muted_until_seconds)}</dd>
            </>
          )}
        </dl>
      </div>
    </Dialog>
  );
}

function FragmentChannel({
  channel,
  text,
}: {
  channel: HistoryAlertsResponse["channels"][number];
  text: LocalizedText;
}): JSX.Element {
  const name =
    channel.name === "in_app"
      ? text("In app · nav badge and this list", "应用内 · 导航角标与本列表")
      : NOTIFY_KINDS.includes(channel.name as NotifyChannelKind)
        ? kindName(channel.name as NotifyChannelKind, text)
        : channel.name;
  return (
    <>
      <dt>{name}</dt>
      <dd>
        {channel.available ? (
          <span className="status-inline">
            <span className="status-dot status-dot--ok" />
            {text("Enabled", "已启用")}
          </span>
        ) : (
          <span className="status-inline muted">
            <span className="status-dot status-dot--hollow" />
            {text("Off · set up under Settings › Notifications", "未启用 · 在“设置 › 通知”里设置")}
          </span>
        )}
      </dd>
    </>
  );
}
