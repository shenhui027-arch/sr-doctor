// Design boards I12-I13 (task 59): load alerts. The server evaluates every
// rule from the stored load history and counts every number; this lays out
// the list and the detail and sends the acknowledge / mute / reopen choices.
// The Routine Load reason is shown here only; notifications never carry it.
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type { AlertState, LoadAlertDetail, LoadAlertView, LoadAlertWindow, LoadAlertsMore, LoadAlertsResponse, LoadKindGroup } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { alertStateText } from "./AlertsPanel";
import { clockText } from "./history-status";
import "./load-alerts.css";

export type AlertTab = AlertState | "all";
const TABS: AlertTab[] = ["open", "acknowledged", "resolved", "muted", "all"];

/** Where an alert's subject lives in the load workspace. */
export type LoadHistoryTarget = { target: string | null; table: string | null; kind: LoadKindGroup | null; jobId: string | null; errors: boolean };

const level = (value: string, text: LocalizedText) => (value === "high" ? text("high", "高") : value === "medium" ? text("medium", "中") : text("low", "低"));

/** Parts per million as a percent, with enough digits for small rates. */
export function ratePercent(ppm: number): string {
  if (ppm === 0) return "0%";
  const percent = ppm / 10_000;
  return `${percent >= 1 ? percent.toFixed(1) : percent >= 0.01 ? percent.toFixed(2) : percent.toFixed(3)}%`;
}

/** Thousands-grouped decimal text, never through a float: counts can exceed 2^53. */
const grouped = (value: string) => (/^\d+$/.test(value) ? value.replace(/\B(?=(\d{3})+(?!\d))/g, ",") : value);
const kindText = (kind: LoadKindGroup) => ({ stream_load: "Stream Load", insert: "INSERT", routine_load: "Routine Load", broker: "Broker Load", other: "Other" }[kind]);

export function loadAlertTitle(detail: LoadAlertDetail, text: LocalizedText): string {
  switch (detail.rule) {
    case "routine_paused": return text("Routine Load paused", "Routine Load 暂停");
    case "routine_stopped": return text(`Routine Load ended (${detail.state})`, `Routine Load 已结束（${detail.state}）`);
    case "lag_growing": return text("Lag keeps growing", "积压持续增长");
    case "failure_rate": return text("Failure rate jump", "失败率突增");
    case "filter_rate": return text("Filter rate jump", "过滤率突增");
  }
}

function subject(detail: LoadAlertDetail): string {
  return detail.rule === "failure_rate" || detail.rule === "filter_rate"
    ? `${detail.database}.${detail.table} · ${kindText(detail.kind)}`
    : `${detail.name} · ${detail.database}.${detail.table}`;
}

function summary(detail: LoadAlertDetail, text: LocalizedText): string {
  switch (detail.rule) {
    case "routine_paused": return detail.max_lag ? text(`partition lag ${grouped(detail.max_lag)} messages`, `分区积压 ${grouped(detail.max_lag)} 条消息`) : text("paused", "已暂停");
    case "routine_stopped": return detail.state;
    case "lag_growing": {
      const hours = detail.hours;
      return text(`${grouped(hours[hours.length - 1]?.max_lag ?? "0")} messages (from ${grouped(hours[0]?.max_lag ?? "0")})`, `${grouped(hours[hours.length - 1]?.max_lag ?? "0")} 条消息（3 小时前 ${grouped(hours[0]?.max_lag ?? "0")}）`);
    }
    case "failure_rate":
    case "filter_rate": return text(`${ratePercent(detail.current.rate_ppm)} (baseline ${ratePercent(detail.baseline.rate_ppm)})`, `${ratePercent(detail.current.rate_ppm)}（基线 ${ratePercent(detail.baseline.rate_ppm)}）`);
  }
}

function utcRange(start: number, end: number): string {
  return `${clockText(start).slice(5, 16)}–${clockText(end).slice(11, 16)} UTC`;
}

function RateEvidence({ detail, text }: { detail: Extract<LoadAlertDetail, { rule: "failure_rate" | "filter_rate" }>; text: LocalizedText }): JSX.Element {
  const failure = detail.rule === "failure_rate";
  const row = (label: string, window: LoadAlertWindow, note: string, now: boolean) => <tr>
    <th scope="row">{label}<span className="load-alert-window history-muted mono">{note}</span></th>
    <td className="num">{grouped(window.total)}</td>
    <td className={`num${now ? " bad" : ""}`}>{grouped(window.part)}</td>
    <td className={`num${now ? " bad" : ""}`}>{ratePercent(window.rate_ppm)}</td>
  </tr>;
  // The server computes both from the raw counts; this only formats them.
  const delta = (detail.delta_ppm / 10_000).toFixed(detail.delta_ppm >= 10_000 || detail.delta_ppm <= -10_000 ? 1 : 2);
  const ratio = detail.ratio_permille === null ? null : (detail.ratio_permille / 1000).toFixed(1);
  return <>
    <table className="history-table load-alert-detail-table">
      <thead><tr>
        <th scope="col"><span className="load-alert-hidden">{text("Window", "窗口")}</span></th>
        <th className="num" scope="col">{failure ? text("Loads", "导入次数") : text("Input rows", "输入行")}</th>
        <th className="num" scope="col">{failure ? text("Failed", "失败") : text("Filtered", "过滤行")}</th>
        <th className="num" scope="col">{failure ? text("Failure rate", "失败率") : text("Filter rate", "过滤率")}</th>
      </tr></thead>
      <tbody>
        {row(text("Last 3 hours", "最近 3 小时"), detail.current, utcRange(detail.current.start_seconds, detail.current.end_seconds), true)}
        {row(text("Baseline · previous 7 days", "基线 · 前 7 天"), detail.baseline, text(`${utcRange(detail.baseline.start_seconds, detail.baseline.end_seconds)} · ${detail.baseline.collected_hours} hours collected`, `${utcRange(detail.baseline.start_seconds, detail.baseline.end_seconds)} · ${detail.baseline.collected_hours} 小时已采集`), false)}
      </tbody>
    </table>
    <p className="history-muted load-alert-note">
      {ratio === null
        ? detail.baseline.part === "0"
          ? text(`${delta} points above a baseline with none (no ratio).`, `比基线高 ${delta} 个百分点（基线里没有，不计倍数）。`)
          : text(`${delta} points above the baseline; the ratio is not available (counts too large to divide exactly).`, `比基线高 ${delta} 个百分点；倍数不可用（计数过大，无法精确计算）。`)
        : text(`${delta} points above the baseline, ${ratio}×.`, `比基线高 ${delta} 个百分点，是基线的 ${ratio} 倍。`)}
      {" "}{failure ? text("The history store keeps counts, not failure reasons: see the table's saved error logs or the load records StarRocks still holds.", "历史库只存次数，不存失败原因；具体原因看这张表保存的错误日志，或在“导入记录”里看 StarRocks 仍保留的记录。") : text("Reasons come from the table's saved error logs (samples).", "原因见这张表保存的错误日志（样本）。")}
    </p>
  </>;
}

function Detail({ alert, target, thresholds, busy, onAction, onOpenHistory, text }: {
  alert: LoadAlertView;
  /** The cluster the alerts belong to; the history opens on it. */
  target: string | null;
  thresholds: LoadAlertsResponse["thresholds"];
  busy: boolean;
  onAction(id: string, action: "acknowledge" | "mute" | "reopen"): void;
  onOpenHistory(target: LoadHistoryTarget): void;
  text: LocalizedText;
}): JSX.Element {
  const detail = alert.detail;
  const isRate = detail.rule === "failure_rate" || detail.rule === "filter_rate";
  return <section aria-labelledby="load-alert-detail" className="history-card">
    <span className="history-muted mono">{text("Load alert", "导入告警")} · {text("first found", "首次发现")} {clockText(alert.first_seen_seconds).slice(5, 16)} UTC · {text("last seen", "最近观测")} {clockText(alert.last_seen_seconds).slice(5, 16)} UTC</span>
    <h2 id="load-alert-detail">{loadAlertTitle(detail, text)} · {subject(detail)}</h2>
    <p className="trend-chips">
      <span className={`alert-sev ${alert.severity}`}>{text("Severity", "严重度")} {level(alert.severity, text)}</span>
      <span className={`alert-state ${alert.state}`}>{alertStateText(alert.state, text)}</span>
      {!thresholds.calibrated && <span className="planned load-alert-trial">{text("Rule on trial", "规则试运行")}</span>}
    </p>
    {detail.rule === "failure_rate" || detail.rule === "filter_rate" ? <RateEvidence detail={detail} text={text} />
      : detail.rule === "lag_growing" ? <>
        <h3 className="trend-subhead">{text("Largest partition lag per hour (messages)", "每小时最大分区积压（条消息）")}</h3>
        <div aria-label={text(`Largest partition lag over the last ${detail.hours.length} hours: ${detail.hours.map((hour) => grouped(hour.max_lag)).join(", ")}`, `最近 ${detail.hours.length} 个完整小时的最大分区积压：${detail.hours.map((hour) => grouped(hour.max_lag)).join("、")}`)} className="load-alert-lag" role="img">
          {detail.hours.map((hour, index) => <div key={hour.start_seconds}>
            <span className="mono">{grouped(hour.max_lag)}</span>
            <span className={index === detail.hours.length - 1 ? "bar latest" : "bar"} style={{ height: `${hour.height_permyriad / 100}%` }} />
            <span className="history-muted mono">{clockText(hour.start_seconds).slice(11, 16)}</span>
          </div>)}
        </div>
        <p className="history-muted load-alert-note">{text(`Rising every hour; latest is ${(detail.ratio_permille / 1000).toFixed(1)}× the first. Lag is read at snapshot times only; partition detail is live on the Routine Load tab.`, `连续上升，最新是 3 小时前的 ${(detail.ratio_permille / 1000).toFixed(1)} 倍。积压只在快照时刻取值；分区明细在“Routine Load”标签实时查看。`)}</p>
      </>
        : <dl className="history-facts">
          <dt>{text("Observed state", "观测状态")}</dt><dd>{detail.rule === "routine_stopped" ? detail.state : "PAUSED"}</dd>
          <dt>{text("Snapshot time", "快照时间")}</dt><dd>{clockText(detail.observed_seconds).slice(5, 16)} UTC · <span className="load-alert-key">{text("when a snapshot first saw it, not when it changed", "首次观测到的快照时刻，不代表真实切换时刻")}</span></dd>
          <dt>{text("Reason", "原因")}</dt><dd className="mono load-alert-reason">{detail.reason ?? "—"}</dd>
          {detail.rule === "routine_paused" && <><dt>{text("Largest partition lag", "最大分区积压")}</dt><dd>{detail.max_lag ? text(`${grouped(detail.max_lag)} messages (latest offset − consumed offset; the largest seen this hour)`, `${grouped(detail.max_lag)} 条消息（最新位置 − 已消费位置；本小时观测到的最大值）`) : text("unknown", "未知")}</dd></>}
        </dl>}
    {!isRate && detail.rule !== "lag_growing" && <p className="history-muted load-alert-note">{text("The reason is shown here only, never in notifications. SR Doctor is read-only and does not resume jobs; once the job runs again, the next collection resolves this alert.", "原因原文只在这里显示，不进通知。SR Doctor 只读，不提供恢复作业；作业恢复后下一轮采集自动解决这条告警。")}</p>}
    <dl className="history-facts">
      <dt>{text("Version and rules", "版本与规则")}</dt><dd>{alert.engine_version === "unknown" ? text("StarRocks version not recorded", "StarRocks 版本未记录") : `StarRocks ${alert.engine_version}`} · {alert.rule_version}</dd>
      {alert.muted_until_seconds !== null && <><dt>{text("Muted until", "静默至")}</dt><dd>{clockText(alert.muted_until_seconds)} UTC</dd></>}
    </dl>
    <div className="alert-actions">
      {alert.state === "open" && <button className="button dark" disabled={busy} onClick={() => onAction(alert.alert_id, "acknowledge")} type="button">{text("Mark acknowledged", "标记已确认")}</button>}
      {(alert.state === "open" || alert.state === "acknowledged") && <button className="button secondary" disabled={busy} onClick={() => onAction(alert.alert_id, "mute")} type="button">{text(`Mute this alert ${thresholds.mute_hours} hours`, `静默此告警 ${thresholds.mute_hours} 小时`)}</button>}
      {alert.state === "resolved" && <button className="button secondary" disabled={busy} onClick={() => onAction(alert.alert_id, "reopen")} type="button">{text("Reopen", "重新打开")}</button>}
      {isRate
        ? <>
          <button className="button secondary" onClick={() => onOpenHistory({ target, table: `${detail.database}.${detail.table}`, kind: detail.kind, jobId: null, errors: false })} type="button">{text(`View ${detail.database}.${detail.table} in load history`, `在导入历史中查看 ${detail.database}.${detail.table}`)}</button>
          <button className="button secondary" onClick={() => onOpenHistory({ target, table: `${detail.database}.${detail.table}`, kind: null, jobId: null, errors: true })} type="button">{text("Saved error logs", "查看错误日志")}</button>
        </>
        : <button className="button secondary" onClick={() => onOpenHistory({ target, table: null, kind: "routine_load", jobId: detail.job_id, errors: false })} type="button">{text("View the job's 30 days", "查看作业的 30 天历史")}</button>}
    </div>
  </section>;
}

export function LoadAlertsPanel({ alerts, busy, selectedId, onSelect, onAction, onOpenHistory, onMore, tab: shownTab, onTab, toolbar, text }: {
  alerts: LoadAlertsResponse;
  /** The state tab, kept by the parent so it survives a reload ("show more"). */
  tab?: AlertTab | null;
  onTab?(tab: AlertTab): void;
  /** Lists more alerts of one state than the default limit. */
  onMore?(more: LoadAlertsMore): void;
  busy: boolean;
  selectedId: string | null;
  onSelect(id: string): void;
  onAction(id: string, action: "acknowledge" | "mute" | "reopen"): void;
  onOpenHistory(target: LoadHistoryTarget): void;
  toolbar?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const [ownTab, setOwnTab] = useState<AlertTab>(alerts.counts.open === "0" ? "all" : "open");
  const tab = shownTab ?? ownTab;
  const setTab = (next: AlertTab) => { setOwnTab(next); onTab?.(next); };
  const total = [alerts.counts.open, alerts.counts.acknowledged, alerts.counts.resolved, alerts.counts.muted].reduce((sum, value) => sum + Number(value), 0);
  const counts: Record<AlertTab, string> = { ...alerts.counts, all: String(total) };
  const visible = alerts.alerts.filter((alert) => tab === "all" || alert.state === tab);
  const selected = visible.find((alert) => alert.alert_id === selectedId) ?? visible[0] ?? null;
  const t = alerts.thresholds;
  return <div className="history-page">
    <div className="history-card-head">
      <div aria-label={text("Alert states", "告警状态")} className="history-tabs" role="tablist">
        {TABS.map((item) => <button aria-selected={item === tab} key={item} onClick={() => setTab(item)} role="tab" type="button">{item === "all" ? text("All", "全部") : alertStateText(item, text)} <span className="tab-count">{counts[item]}</span></button>)}
      </div>
      {!t.calibrated && <span className="planned load-alert-trial">{text("On trial · thresholds not yet calibrated on real workload", "试运行 · 阈值未经真实业务校准")}</span>}
      <span className="history-muted">{text("From the local history store · evaluated after every collection", "来自本机历史库 · 每轮采集后评估")}</span>
      {toolbar && <span className="trend-toolbar">{toolbar}</span>}
    </div>
    <div className="history-row">
      <section aria-labelledby="load-alert-list" className="history-card wide">
        <h2 id="load-alert-list">{text("Load alerts", "导入告警")}</h2>
        {visible.length === 0 ? <p className="history-muted">{text("No load alert in this state.", "这个状态下没有导入告警。")}</p> : <ul className="alert-list">
          {visible.map((alert) => <li key={alert.alert_id}>
            <button aria-pressed={alert.alert_id === selected?.alert_id} className={alert.alert_id === selected?.alert_id ? "alert-row selected" : "alert-row"} onClick={() => onSelect(alert.alert_id)} type="button">
              <span className="history-muted mono">{clockText(alert.last_seen_seconds).slice(5, 16)}</span>
              <strong>{loadAlertTitle(alert.detail, text)}</strong>
              <span className={`alert-sev ${alert.severity}`}>{text("Severity", "严重度")} {level(alert.severity, text)}</span>
              <span className="history-muted">{summary(alert.detail, text)}</span>
              <code className="alert-template">{subject(alert.detail)}</code>
              <span className={`alert-state ${alert.state}`}>{alertStateText(alert.state, text)}</span>
            </button>
          </li>)}
        </ul>}
        {tab !== "all" && visible.length < Number(counts[tab]) && onMore && <p className="load-alert-more">
          <span className="history-muted">{text(`Showing the newest ${visible.length} of ${counts[tab]}.`, `显示最近 ${visible.length} 条，共 ${counts[tab]} 条。`)}</span>
          <button className="button secondary small" disabled={busy} onClick={() => onMore({ state: tab, limit: visible.length + alerts.list_limit })} type="button">{text(`Show ${alerts.list_limit} more`, `再显示 ${alerts.list_limit} 条`)}</button>
        </p>}
        {tab === "all" && alerts.alerts.length < total && <p className="history-muted load-alert-more">{text(`Each state lists its newest ${alerts.list_limit}; pick a state to show more.`, `每种状态各显示最近 ${alerts.list_limit} 条；切换到具体状态可以继续显示。`)}</p>}
      </section>
      <div className="alert-side">
        {selected && <Detail alert={selected} target={alerts.target} busy={busy} onAction={onAction} onOpenHistory={onOpenHistory} text={text} thresholds={t} />}
        <details className="history-card history-details" open={alerts.alerts.length === 0}>
          <summary>{text("Load alert rules (on trial)", "导入告警规则（试运行）")}</summary>
          <ul className="alert-rules">
            <li>{text("Routine Load paused: the latest snapshot says PAUSED (high); resolved when it runs again.", "Routine Load 暂停：最近一次快照为 PAUSED（高）；恢复运行后自动解决。")}</li>
            <li>{text("Ended: a job was seen changing to STOPPED or CANCELLED (medium).", "停止或取消：观察到作业变为 STOPPED / CANCELLED（中）。")}</li>
            <li>{text(`Lag keeps growing: running, the largest partition lag rose in each of the last ${t.lag_hours} complete hours, latest ≥ ${grouped(t.lag_min)} messages and ≥ 2× the first (medium; ≥ ${grouped(t.lag_high)} is high).`, `积压持续增长：运行中，最近 ${t.lag_hours} 个完整小时的最大分区积压逐小时上升，且最新 ≥ ${grouped(t.lag_min)} 条、≥ 3 小时前的 2 倍（中；≥ ${grouped(t.lag_high)} 条为高）。`)}</li>
            <li>{text(`Failure rate jump: same table and type, ≥ ${t.failure_min_loads} loads in the last ${t.current_hours} hours and ≥ ${t.failure_min_baseline_loads} in the previous ${t.baseline_hours / 24} days; now ≥ ${ratePercent(t.failure_min_ppm)}, ≥ 5 points and ≥ 3× above the baseline (medium; ≥ ${ratePercent(t.failure_high_ppm)} is high).`, `失败率突增：同表同类型，最近 ${t.current_hours} 小时 ≥ ${t.failure_min_loads} 次、前 ${t.baseline_hours / 24} 天 ≥ ${t.failure_min_baseline_loads} 次；当前 ≥ ${ratePercent(t.failure_min_ppm)}，高出基线 ≥ 5 个百分点且 ≥ 3 倍（中；≥ ${ratePercent(t.failure_high_ppm)} 为高）。`)}</li>
            <li>{text(`Filter rate jump: ≥ ${grouped(String(t.filter_min_rows))} input rows now and ≥ ${grouped(String(t.filter_min_baseline_rows))} in the baseline; now ≥ ${ratePercent(t.filter_min_ppm)}, ≥ 1 point and ≥ 3× above the baseline (medium; ≥ ${ratePercent(t.filter_high_ppm)} is high).`, `过滤率突增：最近输入 ≥ ${grouped(String(t.filter_min_rows))} 行、基线 ≥ ${grouped(String(t.filter_min_baseline_rows))} 行；当前 ≥ ${ratePercent(t.filter_min_ppm)}，高出基线 ≥ 1 个百分点且 ≥ 3 倍（中；≥ ${ratePercent(t.filter_high_ppm)} 为高）。`)}</li>
            <li>{text("Too few samples, an uncollected or partly collected hour in the window, or a version or rule change decides nothing. One alert per subject.", "样本不足、窗口内有未采集或部分采集的小时、版本或统计规则变化时不判定。同一对象只保留一条告警。")}</li>
            <li>{text("Notifications carry the rule, the subject, the numbers and the time, never error log text or the pause reason; SMS carries only the count and the highest severity.", "通知只带规则、对象、数值与时间，不带错误原文或暂停原因原文；短信只带条数与最高严重度。")}</li>
          </ul>
        </details>
      </div>
    </div>
  </div>;
}
