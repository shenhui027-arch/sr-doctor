// Design board D (治理队列): a single-user record of what was changed outside
// this tool and whether the history shows it helped. The server owns the
// state machine: the user moves only along `allowed_transitions`, records a
// change to reach "pending verification", and only the comparator closes.
import type { JSX, ReactNode } from "react";
import type { GovernanceItem, GovernanceListResponse, GovernanceState, VerificationOutcome } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { metricText } from "../audit/audit-display";
import { clockText } from "./CollectionPanel";
import { trendChangeText } from "./TrendsPanel";

const STEPS: GovernanceState[] = ["new", "triaged", "in_progress", "pending_verification", "closed"];

export function governanceStateText(state: GovernanceState, text: LocalizedText): string {
  return {
    new: text("New", "新建"),
    triaged: text("Triaged", "已分诊"),
    in_progress: text("In progress", "处理中"),
    pending_verification: text("Pending verification", "待验证"),
    closed: text("Closed · verified improved", "已关闭 · 验证改善"),
    wont_fix: text("Won't fix", "不处理"),
  }[state];
}

export function outcomeText(outcome: VerificationOutcome, text: LocalizedText): string {
  switch (outcome.state) {
    case "improved": return text("Verified · improved", "已验证 · 改善");
    case "not_improved": return text("Not improved", "未改善");
    case "insufficient_samples": return text("Collecting samples · no verdict yet", "样本积累中 · 暂不下结论");
    case "workload_signal_missing": return text("No scan or CPU signal · no verdict", "无扫描量或 CPU 信号 · 不下结论");
    case "not_comparable": return outcome.reason === "engine_version_changed" ? text("StarRocks version changed · not comparable", "版本变化，暂不可比") : text("Rule set changed · not comparable", "规则集变化，暂不可比");
  }
}

const originText = (item: GovernanceItem, text: LocalizedText) => ({ alert: text(`Alert #${item.alert_id ?? ""}`, `告警 #${item.alert_id ?? ""}`), trends: text("Regression list", "回归列表"), manual: text("Manual", "手动") }[item.origin]);
const metricName = (name: string, text: LocalizedText) => ({ query_time: text("Median query time", "查询耗时中位数"), scan_bytes: text("Median scan bytes", "扫描数据量中位数"), cpu_cost: text("Median CPU", "CPU 中位数") }[name] ?? name);

export type GovernanceCommand =
  | { kind: "transition"; to: GovernanceState }
  | { kind: "pin" }
  | { kind: "change"; note: string }
  | { kind: "verify" };

/** Change-note drafts, one per item: a draft never moves to another item. */
export type NoteDrafts = Readonly<Record<string, string>>;

export function GovernancePanel({ governance, busy, selectedId, onSelect, onCommand, onOpenTrends, notes = {}, onNote = () => undefined, toolbar, text }: {
  governance: GovernanceListResponse;
  busy: boolean;
  selectedId: string | null;
  onSelect(id: string): void;
  onCommand(item: GovernanceItem, command: GovernanceCommand): void;
  onOpenTrends(templateKey: string): void;
  notes?: NoteDrafts;
  onNote?(itemId: string, note: string): void;
  /** Target and refresh controls, shown in the page head. */
  toolbar?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const items = governance.items;
  const selected = items.find((item) => item.item_id === selectedId) ?? items[0] ?? null;
  const stage = selected ? STEPS.indexOf(selected.state) : -1;
  const note = selected ? notes[selected.item_id] ?? "" : "";
  return <div className="history-page">
    <div className="history-card-head">
      <h2 className="governance-title">{text("Governance queue", "治理队列")}</h2>
      <span className="history-muted">{text("After you optimize in the cluster, record the change here and verify its effect", "在集群完成优化后，在此记录改动并验证效果")}</span>
      {toolbar && <span className="trend-toolbar">{toolbar}</span>}
    </div>
    <div className="history-row">
      <section aria-labelledby="governance-list" className="history-card wide">
        <h2 id="governance-list">{text("Items", "治理项")}</h2>
        {items.length === 0 ? <p className="history-muted">{text("No item yet. Add one from an alert or the regression list.", "还没有治理项。可从告警或回归列表加入。")}</p> : <div className="history-table-wrap"><table className="history-table trend-table">
          <thead><tr><th scope="col">{text("Item", "治理项")}</th><th scope="col">{text("Origin", "来源")}</th><th scope="col">{text("Baseline snapshot", "基线快照")}</th><th scope="col">{text("State", "状态")}</th></tr></thead>
          <tbody>{items.map((item) => <tr aria-selected={item.item_id === selected?.item_id} className={item.item_id === selected?.item_id ? "selected" : undefined} key={item.item_id} onClick={() => onSelect(item.item_id)}>
            <td className="trend-template-cell"><button className="link-button title" onClick={() => onSelect(item.item_id)} type="button">{item.title}</button><code className="alert-template">{item.template ?? item.template_key}</code></td>
            <td>{originText(item, text)}</td>
            <td>{item.snapshot ? text(`Pinned · ${clockText(item.snapshot.pinned_seconds).slice(5, 16)}`, `已钉住 · ${clockText(item.snapshot.pinned_seconds).slice(5, 16)}`) : text("Not pinned", "未钉住")}</td>
            <td><span className={`governance-state ${item.state}`}>{governanceStateText(item.state, text)}</span></td>
          </tr>)}</tbody>
        </table></div>}
      </section>
      {selected && <section aria-labelledby="governance-detail" className="history-card governance-detail">
        <span className="history-muted">{text("Item", "治理项")} #{selected.item_id}</span>
        <h2 id="governance-detail">{selected.title}</h2>
        <ol aria-label={text("Progress", "进度")} className="governance-steps">
          {STEPS.map((step, index) => <li className={selected.state === "wont_fix" ? "skipped" : index < stage ? "done" : index === stage ? "current" : undefined} key={step}>{governanceStateText(step, text).split(" · ")[0]}</li>)}
        </ol>
        {selected.state === "wont_fix" && <p className="history-muted">{governanceStateText("wont_fix", text)}</p>}

        <h3 className="trend-subhead">{text("Pinned baseline snapshot", "钉住的基线快照")}</h3>
        {selected.snapshot ? <dl className="history-facts">
          <dt>{text("Pinned (UTC)", "钉住时间（UTC）")}</dt><dd>{clockText(selected.snapshot.pinned_seconds)}</dd>
          <dt>{text("Window", "窗口")}</dt><dd>{clockText(selected.snapshot.window_start_seconds).slice(5, 16)} – {clockText(selected.snapshot.window_end_seconds).slice(5, 16)}</dd>
          <dt>{text("Median", "中位数")}</dt><dd>{metricText(selected.snapshot.query_time_median)} · {text("scan", "扫描")} {metricText(selected.snapshot.scan_bytes_median)} · CPU {metricText(selected.snapshot.cpu_cost_median)}</dd>
          <dt>{text("Executions", "执行次数")}</dt><dd>{selected.snapshot.executions} · {text("valid", "有效")} {selected.snapshot.valid_samples}</dd>
          <dt>{text("Representative", "代表执行")}</dt><dd>{selected.snapshot.exemplar_query_id ?? "—"}</dd>
        </dl> : <p className="history-muted">{text("No snapshot pinned yet. Before recording a change, pin the template's recent executions as the baseline.", "尚未钉住基线快照。记录改动前，请先钉住该模板最近的执行作为基线。")}</p>}
        <div className="alert-actions">
          <button className="button secondary" onClick={() => onOpenTrends(selected.template_key)} type="button">{text("View trend", "查看趋势")}</button>
          {(selected.state === "new" || selected.state === "triaged" || selected.state === "in_progress") && <button className="button secondary" disabled={busy} onClick={() => onCommand(selected, { kind: "pin" })} type="button">{selected.snapshot ? text("Re-pin baseline", "更换基线") : text("Pin baseline", "钉住基线")}</button>}
        </div>

        <h3 className="trend-subhead">{text("Record", "处理记录")}</h3>
        <ol className="governance-events">
          {selected.events.map((event, index) => <li key={`${event.at_seconds}-${index}`}>
            <span className="history-muted mono">{clockText(event.at_seconds).slice(5, 16)}</span>
            <span>{event.kind === "created" ? text("Created", "创建") : event.kind === "pinned" ? text("Baseline pinned", "钉住基线") : event.kind === "change_recorded" ? text("Change recorded", "记录改动") : event.kind === "verified" ? text("Effect verified", "效果验证") : `${event.from ? governanceStateText(event.from, text) : ""} → ${governanceStateText(event.to, text)}`}</span>
            <span className="history-muted">{event.actor === "comparator" ? text("automatic verification", "自动验证") : text("you", "你")}</span>
            {event.note && <span className="governance-note">{event.note}</span>}
          </li>)}
        </ol>
        {selected.state === "in_progress" && <div className="governance-change">
          <label htmlFor="governance-note">{text("What did you change outside this tool? (no SQL text)", "你在本工具外做了什么改动？（不要粘贴 SQL）")}</label>
          <textarea id="governance-note" maxLength={500} onChange={(event) => onNote(selected.item_id, event.target.value)} rows={2} value={note} />
          <button className="button dark" disabled={busy || !note.trim() || !selected.snapshot} onClick={() => onCommand(selected, { kind: "change", note })} type="button">{text("Record the change and wait for verification", "记录改动并进入待验证")}</button>
          {!selected.snapshot && <p className="history-muted">{text("Pin a baseline first.", "请先钉住基线。")}</p>}
        </div>}

        <h3 className="trend-subhead">{text("Before / after verification", "前后对比验证")}</h3>
        {selected.verification ? <>
          <p className="history-muted">{text(`Before: pinned window · after: ${clockText(selected.verification.after_start_seconds).slice(5, 16)} – ${clockText(selected.verification.after_end_seconds).slice(5, 16)}`, `前：钉住的窗口 · 后：${clockText(selected.verification.after_start_seconds).slice(5, 16)} – ${clockText(selected.verification.after_end_seconds).slice(5, 16)}`)}</p>
          <table className="history-table">
            <thead><tr><th scope="col">{text("Metric", "指标")}</th><th className="num" scope="col">{text("Before", "治理前")}</th><th className="num" scope="col">{text("After", "治理后")}</th><th className="num" scope="col">{text("Change", "变化")}</th></tr></thead>
            <tbody>
              {selected.verification.metrics.map((metric) => <tr key={metric.metric}><td>{metricName(metric.metric, text)}</td><td className="num">{metricText(metric.before)}</td><td className="num">{metricText(metric.after)}</td><td className="num">{trendChangeText(metric.change)}</td></tr>)}
              <tr><td>{text("Executions", "执行次数")}</td><td className="num">{selected.verification.executions_before}</td><td className="num">{selected.verification.executions_after}</td><td className="num">—</td></tr>
            </tbody>
          </table>
          <p className="trend-chips">
            <span className={`trend-verdict ${selected.verification.outcome.state === "improved" ? "down" : selected.verification.outcome.state === "not_improved" ? "up" : "dash"}`}>{outcomeText(selected.verification.outcome, text)}</span>
            <span>{text("Confidence", "置信度")} {selected.verification.confidence ? ({ high: text("high", "高"), medium: text("medium", "中"), low: text("low", "低") }[selected.verification.confidence]) : "—"}</span>
            <span className={`evidence ${selected.verification.evidence}`}>{text("Evidence", "证据")} {({ complete: text("complete", "完整"), partial: text("partial", "部分"), unavailable: text("unavailable", "不可用") }[selected.verification.evidence])}</span>
          </p>
        </> : <p className="history-muted">{selected.change_seconds !== null ? text(`After window starts at the first whole hour after ${clockText(selected.change_seconds)}.`, `治理后窗口从 ${clockText(selected.change_seconds)} 之后的第一个整点开始累计。`) : text("No after window yet: record the change first.", "尚无治理后窗口：请先记录改动。")}</p>}
        <div className="alert-actions">
          {selected.state === "pending_verification" && <button className="button dark" disabled={busy} onClick={() => onCommand(selected, { kind: "verify" })} type="button">{text("Verify the effect", "验证优化效果")}</button>}
          {selected.allowed_transitions.map((to) => <button className={to === "wont_fix" ? "button secondary" : "button dark"} disabled={busy} key={to} onClick={() => onCommand(selected, { kind: "transition", to })} type="button">{{
            triaged: text("Mark triaged", "标为已分诊"),
            in_progress: selected.state === "new" || selected.state === "triaged" ? text("Start work", "开始处理") : text("Back to work", "退回处理中"),
            wont_fix: text("Won't fix", "不处理"),
            new: "", pending_verification: "", closed: "",
          }[to]}</button>)}
        </div>
        <p className="history-muted">{text("An item closes only when verification shows an improvement; it cannot be closed by hand.", "治理项只在验证确认改善后自动关闭，不能手动关闭。")}</p>
      </section>}
    </div>
  </div>;
}
