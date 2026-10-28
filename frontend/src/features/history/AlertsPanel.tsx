// Design board D (告警): in-app alerts raised from stage C regressions. The
// server decides every alert, its state and its evidence; this only renders
// and sends the user's acknowledge / mute / reopen / add-to-queue choices,
// and the one-click resolve of alerts left over from an older rule version.
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type { AlertState, AlertView, HistoryAlertsResponse } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { metricText } from "../audit/audit-display";
import { clockText } from "./CollectionPanel";

type AlertTab = AlertState | "all";
const TABS: AlertTab[] = ["open", "acknowledged", "resolved", "muted", "all"];

export function alertStateText(state: AlertState, text: LocalizedText): string {
  return { open: text("New", "新告警"), acknowledged: text("Acknowledged", "已确认"), resolved: text("Resolved", "已解决"), muted: text("Muted", "已静默") }[state];
}

export function alertTitle(alert: AlertView, text: LocalizedText): string {
  const change = alert.change;
  if (!change) return text("Median query time regression", "查询耗时中位数回归");
  const relative = change.relative_percent === null ? text(" (baseline 0, no percentage)", "（基线为 0，百分比不定义）") : ` +${change.relative_percent}%`;
  return text(`Median query time regression${relative}`, `查询耗时中位数回归${relative}`);
}

const level = (value: "high" | "medium" | "low", text: LocalizedText) => ({ high: text("high", "高"), medium: text("medium", "中"), low: text("low", "低") }[value]);
const evidenceText = (value: AlertView["evidence"], text: LocalizedText) => ({ complete: text("complete", "完整"), partial: text("partial", "部分"), unavailable: text("unavailable", "不可用") }[value]);

export function AlertsPanel({ alerts, busy, selectedId, onSelect, onAction, onResolveRuleChanged, onOpenTrends, onAddToQueue, onOpenItem, toolbar, text }: {
  alerts: HistoryAlertsResponse;
  /** Target and refresh controls, shown next to the state tabs. */
  toolbar?: ReactNode;
  busy: boolean;
  selectedId: string | null;
  onSelect(id: string): void;
  onAction(id: string, action: "acknowledge" | "mute" | "reopen"): void;
  onResolveRuleChanged(): void;
  onOpenTrends(templateKey: string): void;
  onAddToQueue(alert: AlertView): void;
  onOpenItem(itemId: string): void;
  text: LocalizedText;
}): JSX.Element {
  // With nothing new, open on everything so acknowledged alerts stay in view.
  const [tab, setTab] = useState<AlertTab>(alerts.counts.open === "0" ? "all" : "open");
  const { rule_changed: ruleChanged, ...stateCounts } = alerts.counts;
  const counts: Record<AlertTab, string> = { ...stateCounts, all: String(alerts.alerts.length) };
  const visible = alerts.alerts.filter((alert) => tab === "all" || alert.state === tab);
  const selected = visible.find((alert) => alert.alert_id === selectedId) ?? visible[0] ?? null;
  return <div className="history-page">
    <div className="history-card-head">
      <div aria-label={text("Alert states", "告警状态")} className="history-tabs" role="tablist">
        {TABS.map((item) => <button aria-selected={item === tab} key={item} onClick={() => setTab(item)} role="tab" type="button">{item === "all" ? text("All", "全部") : alertStateText(item, text)} <span className="tab-count">{counts[item]}</span></button>)}
      </div>
      <span className="history-muted">{text("Only changes that can be judged raise an alert", "仅显示可判断的性能变化")}</span>
      {toolbar && <span className="trend-toolbar">{toolbar}</span>}
    </div>
    {ruleChanged !== "0" && <p className="history-banner rule-changed-banner" role="status">
      {text(ruleChanged === "1"
        ? "1 alert was raised under the rules before an upgrade. The current rules cannot compare it, so it will not resolve by itself."
        : `${ruleChanged} alerts were raised under the rules before an upgrade. The current rules cannot compare them, so they will not resolve by themselves.`, `有 ${ruleChanged} 条告警来自升级前的规则。当前规则无法与之比较，它们不会自动解决。`)}
      <button className="button secondary" disabled={busy} onClick={onResolveRuleChanged} type="button">{text("Mark them resolved", "全部标为已解决")}</button>
    </p>}
    <div className="history-row">
      <section aria-labelledby="alert-list" className="history-card wide">
        <h2 id="alert-list">{text("Alerts", "告警")}</h2>
        {visible.length === 0 ? <p className="history-muted">{text("No alert in this state.", "这个状态下没有告警。")}</p> : <ul className="alert-list">
          {visible.map((alert) => <li key={alert.alert_id}>
            <button aria-pressed={alert.alert_id === selected?.alert_id} className={alert.alert_id === selected?.alert_id ? "alert-row selected" : "alert-row"} onClick={() => onSelect(alert.alert_id)} type="button">
              <span className="history-muted mono">{clockText(alert.last_seen_seconds).slice(5, 16)}</span>
              <strong>{alertTitle(alert, text)}{alert.rule_changed && <span className="rule-changed">{text("Rules changed", "规则已变更")}</span>}</strong>
              <span className={`alert-sev ${alert.severity}`}>{text("Severity", "严重度")} {level(alert.severity, text)}</span>
              <span className="history-muted">{text("Confidence", "置信度")} {level(alert.confidence, text)}</span>
              <code className="alert-template">{alert.template ?? alert.template_key}</code>
              <span className={`alert-state ${alert.state}`}>{alertStateText(alert.state, text)}</span>
            </button>
          </li>)}
        </ul>}
      </section>
      <div className="alert-side">
        {selected && <section aria-labelledby="alert-detail" className="history-card">
          <span className="history-muted mono">{text("Alert", "告警")} · {clockText(selected.first_seen_seconds).slice(5, 16)}</span>
          <h2 id="alert-detail">{alertTitle(selected, text)}</h2>
          <p className="trend-chips">
            <span className={`alert-sev ${selected.severity}`}>{text("Severity", "严重度")} {level(selected.severity, text)}</span>
            <span>{text("Confidence", "置信度")} {level(selected.confidence, text)}</span>
            <span className={`evidence ${selected.evidence}`}>{text("Evidence", "证据")} {evidenceText(selected.evidence, text)}</span>
            {selected.workload === "stable" && <span>{text("Workload unchanged: possibly contention", "工作量未变：可能是资源争用")}</span>}
          </p>
          <h3 className="trend-subhead">{text("Evidence chain", "证据链")}</h3>
          <dl className="history-facts">
            <dt>{text("Template", "模板")}</dt><dd>{selected.template ?? "—"} · {selected.kind === "official_digest" ? text("official digest", "官方 digest") : text("unofficial fingerprint", "非官方指纹")}</dd>
            <dt>{text("Hour (UTC)", "时间桶（UTC）")}</dt><dd>{clockText(selected.bucket_start_seconds).slice(5, 16)}–{clockText(selected.bucket_start_seconds + 3600).slice(11, 16)} · {metricText(selected.baseline_median)} → {metricText(selected.current_median)}</dd>
            <dt>{text("Representative", "代表执行")}</dt><dd>{selected.exemplar_query_id ?? "—"}</dd>
            <dt>{text("Comparability", "可比性")}</dt><dd>StarRocks {selected.comparability.engine_version} · {selected.comparability.rule_version}{selected.rule_changed && text(" · rules changed since; not comparable", " · 规则已变更，无法比较")}</dd>
            {selected.muted_until_seconds !== null && <><dt>{text("Muted until", "静默至")}</dt><dd>{clockText(selected.muted_until_seconds)}</dd></>}
          </dl>
          <div className="alert-actions">
            <button className="button secondary" onClick={() => onOpenTrends(selected.template_key)} type="button">{text("View trend and representative executions", "查看趋势与代表执行")}</button>
            {selected.state === "open" && <button className="button dark" disabled={busy} onClick={() => onAction(selected.alert_id, "acknowledge")} type="button">{text("Acknowledge", "确认")}</button>}
            {selected.governance_item_id ? <button className="button secondary" onClick={() => onOpenItem(selected.governance_item_id!)} type="button">{text(`Open governance item #${selected.governance_item_id}`, `查看治理项 #${selected.governance_item_id}`)}</button>
              : <button className="button secondary" disabled={busy} onClick={() => onAddToQueue(selected)} type="button">{text("Add to governance queue", "加入治理队列")}</button>}
            {(selected.state === "open" || selected.state === "acknowledged") && <button className="button secondary" disabled={busy} onClick={() => onAction(selected.alert_id, "mute")} type="button">{text(`Mute ${alerts.rule.mute_hours} hours`, `静默 ${alerts.rule.mute_hours} 小时`)}</button>}
            {selected.state === "resolved" && <button className="button secondary" disabled={busy} onClick={() => onAction(selected.alert_id, "reopen")} type="button">{text("Reopen", "重新打开")}</button>}
          </div>
        </section>}
        <details className="history-card history-details">
          <summary>{text("Alert rules and channels", "告警规则与通知渠道")}</summary>
          <ul className="alert-rules">
            <li>{text(`A template's verdict is a regression of at least ${level(alerts.rule.min_severity, text)} severity (last ${alerts.rule.current_hours} hours against the ${alerts.rule.baseline_hours / 24} days before)`, `模板判定为“回归”，且严重度不低于“${level(alerts.rule.min_severity, text)}”（最近 ${alerts.rule.current_hours} 小时对比之前 ${alerts.rule.baseline_hours / 24} 天）`)}</li>
            <li>{text("Too few samples or a version change raises no alert", "样本不足或版本变化时不产生告警")}</li>
            <li>{text("One alert per template until it is back inside the baseline band", "同一模板在恢复到基线带前只产生一条告警")}</li>
            <li>{text("Back inside the band it is resolved automatically and can be reopened by hand", "恢复到基线带后自动标为已解决，可手动重新打开")}</li>
            <li>{alerts.rule.calibrated ? text("Latency alone never decides.", "不以耗时单独判定。") : text("Latency alone never decides; the thresholds are on trial until calibrated on real workload.", "不以耗时单独判定；阈值尚未用真实业务分布校准，处于试运行。")}</li>
          </ul>
          <h3 className="trend-subhead">{text("Channels", "通知渠道")}</h3>
          <dl className="history-facts">
            {alerts.channels.map((channel) => <FragmentChannel channel={channel} key={channel.name} text={text} />)}
          </dl>
        </details>
      </div>
    </div>
  </div>;
}

function FragmentChannel({ channel, text }: { channel: HistoryAlertsResponse["channels"][number]; text: LocalizedText }): JSX.Element {
  const name = { in_app: text("In app · nav badge and this list", "应用内 · 导航角标与本列表"), webhook: "Webhook", email: text("Email", "邮件") }[channel.name] ?? channel.name;
  return <><dt>{name}</dt><dd>{channel.available ? text("Enabled", "已启用") : <span className="planned">{text("Not available in this version", "当前版本暂不支持")}</span>}</dd></>;
}
