// Design board B (采集与历史): collector state, the read budget, 24 hours of
// coverage, recent reads and what the local history store keeps. Every value
// is a server DTO field; nothing here reads StarRocks or recomputes coverage.
import type { JSX, ReactNode } from "react";
import type { CollectionRun, HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { countText } from "../../lib/duration-display";
import { clockText, collectionLabel } from "./history-status";
import "./history.css";

export { clockText, collectionLabel };

function bytesText(value: string | null | undefined): string {
  if (!value) return "—";
  const bytes = Number(value);
  if (!Number.isFinite(bytes)) return value;
  const units = ["B", "KB", "MB", "GB"];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit += 1; }
  return `${unit === 0 ? size : size.toFixed(1)} ${units[unit]}`;
}

function runLabel(run: CollectionRun, text: LocalizedText): string {
  const reason = run.reason ?? "";
  if (run.state === "complete") return text("Complete", "成功");
  if (run.state === "partial") return reason === "row_limit" ? text("Partial · row limit reached", "部分 · 达到行数上限") : text("Partial", "部分");
  if (run.state === "gap") return text("Not read · beyond catch-up", "未读取 · 超出补采范围");
  return text(`Failed · will retry (${reason})`, `失败 · 稍后重试（${reason}）`);
}

export function CollectionPanel({ status, busy, onToggle, onClear, clearConfirm, text }: {
  status: HistoryStatusResponse;
  busy: boolean;
  onToggle(action: "pause" | "resume"): void;
  onClear(): void;
  /** Shown in the store card, next to the button it confirms. */
  clearConfirm?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const state = status.collection;
  const store = status.store;
  const canToggle = state.state === "running" || state.state === "paused" || state.state === "backoff";
  const counts = status.coverage.reduce((total, hour) => ({ ...total, [hour.state]: (total[hour.state] ?? 0) + 1 }), {} as Record<string, number>);
  return <div className="history-page">
    {state.state === "stopped" && state.reason === "production_gate"
      ? <div className="history-banner" role="note">{text("This is a remote cluster; collection is not enabled yet.", "这是远端集群，采集尚未启用。")}
        <details className="history-details inline"><summary>{text("What enables it", "查看启用条件")}</summary>{text("A production read-only pilot must first confirm TLS verify-full, the account's read-only permissions and the read load; the operator then turns collection on.", "需要先完成生产只读试点，确认 TLS verify-full、账号只读权限与读取负载，再由运维开启采集。")}</details>
      </div>
      : state.state === "stopped" && state.reason === "not_connected"
      ? <p className="history-banner neutral" role="note">{text("No connected session. Reconnect to continue from the watermark; the time in between is shown as not collected.", "当前没有已连接的会话。重新连接后从水位继续；期间的时段显示为未采集。")}</p>
      : null}
    <div className="history-row">
      <section aria-labelledby="history-collector" className="history-card">
        <div className="history-card-head">
          <h2 id="history-collector">{text("Incremental collection", "增量采集")}</h2>
          <span className={`history-state ${state.state}`}>{collectionLabel(status, text)}</span>
          {canToggle && <button className="button dark" disabled={busy} onClick={() => onToggle(state.state === "paused" ? "resume" : "pause")} type="button">{state.state === "paused" ? text("Resume", "恢复采集") : text("Pause", "暂停采集")}</button>}
        </div>
        <dl className="history-facts">
          <dt>{text("Target", "目标")}</dt><dd>{status.target ?? "—"}</dd>
          <dt>{text("Watermark (UTC)", "采集水位（UTC）")}</dt><dd>{clockText(status.watermark_seconds)}</dd>
          <dt>{text("Next read (UTC)", "下次读取（UTC）")}</dt><dd>{state.state === "running" || state.state === "backoff" ? clockText(state.next_run_seconds) : "—"}</dd>
        </dl>
        <details className="history-details">
          <summary>{text("How collection runs", "采集方式")}</summary>
          <dl className="history-facts">
            <dt>{text("Runs while", "运行条件")}</dt><dd>{text("this session stays connected · one connection · deduplicated by queryId", "只在本会话保持连接时运行 · 单连接 · 按 queryId 去重")}</dd>
            <dt>{text("Paused", "暂停")}</dt><dd>{text("keeps collected data; resumes from the watermark", "保留已采数据，恢复时从水位继续")}</dd>
            <dt>{text("Over budget", "超出预算")}</dt><dd>{text("the slice is marked partial rather than read harder", "宁可漏采并标记部分，不加大负载")}</dd>
            <dt>{text("On failure", "失败时")}</dt><dd>{text("waits longer before each retry (exponential backoff)", "每次重试前等待更久（指数退避）")}</dd>
          </dl>
        </details>
      </section>
      <section aria-labelledby="history-budget" className="history-card">
        <h2 id="history-budget">{text("Read budget per slice", "每次读取预算")}</h2>
        <table className="history-table">
          <thead><tr><th scope="col">{text("Item", "项")}</th><th className="num" scope="col">{text("Local default", "本机开发默认")}</th><th className="num" scope="col">{text("Production", "生产预算")}</th></tr></thead>
          <tbody>
            <tr><td>{text("Read period", "读取周期")}</td><td className="num">{status.budget.period_seconds} s</td><td className="num muted">—</td></tr>
            <tr><td>{text("Slice length", "单次时间片")}</td><td className="num">{status.budget.slice_seconds} s</td><td className="num muted">—</td></tr>
            <tr><td>{text("Read lag", "读取滞后")}</td><td className="num">{status.budget.lag_seconds} s</td><td className="num muted">—</td></tr>
            <tr><td>{text("Rows per slice", "每次最多行数")}</td><td className="num">{countText(String(status.budget.slice_row_limit)) ?? status.budget.slice_row_limit}</td><td className="num muted">—</td></tr>
            <tr><td>{text("Statement timeout", "单条语句超时")}</td><td className="num">{status.budget.query_timeout_seconds} s</td><td className="num muted">—</td></tr>
          </tbody>
        </table>
        {!status.budget.production_budget_measured && <p className="history-pending"><span>{text("Not measured yet", "待实测")}</span>{text("The production budget is set after the production read-only pilot measures it.", "生产预算由生产只读试点实测后写入。")}</p>}
      </section>
    </div>
    <section aria-labelledby="history-coverage" className="history-card">
      <div className="history-card-head">
        <h2 id="history-coverage">{text("Coverage · last 24 hours", "最近 24 小时覆盖")}</h2>
        <span className="history-muted">{text(`complete ${counts.complete ?? 0} · partial ${counts.partial ?? 0} · not collected ${counts.not_collected ?? 0} · gaps are never interpolated`, `完整 ${counts.complete ?? 0} · 部分 ${counts.partial ?? 0} · 未采集 ${counts.not_collected ?? 0} · 缺口不插值`)}</span>
      </div>
      <ol aria-label={text("Hourly coverage", "每小时覆盖")} className="history-strip">{status.coverage.map((hour) => <li className={hour.state} key={hour.hour_start_seconds} title={`${clockText(hour.hour_start_seconds)} · ${hour.state}`} />)}</ol>
      <div className="history-legend">
        <span><i className="complete" />{text("Complete", "完整")}</span>
        <span><i className="partial" />{text("Partial: row limit reached or hour not fully read", "部分：达到行数上限或该小时未读完")}</span>
        <span><i className="not_collected" />{text("Not collected: not zero executions", "未采集：不是 0 次执行")}</span>
      </div>
    </section>
    <div className="history-row">
      <section aria-labelledby="history-runs" className="history-card wide">
        <h2 id="history-runs">{text("Recent reads", "最近读取")}</h2>
        {status.runs.length === 0 ? <p className="history-muted">{text("No reads yet.", "尚无读取记录。")}</p> : <div className="history-table-wrap"><table className="history-table">
          <thead><tr><th scope="col">{text("Started (UTC)", "时间（UTC）")}</th><th scope="col">{text("Slice", "时间片")}</th><th className="num" scope="col">{text("Rows read", "读取行数")}</th><th className="num" scope="col">{text("Duration", "耗时")}</th><th scope="col">{text("Result", "结果")}</th></tr></thead>
          <tbody>{status.runs.map((run) => <tr key={`${run.slice_start_seconds}-${run.started_seconds}-${run.state}`}>
            <td className="mono">{clockText(run.started_seconds)}</td>
            <td className="mono">{clockText(run.slice_start_seconds).slice(11, 16)}–{clockText(run.slice_end_seconds).slice(11, 16)}</td>
            <td className="num">{countText(run.rows_read) ?? run.rows_read}</td>
            <td className="num">{run.duration_ms} ms</td>
            <td><span className={`history-run ${run.state}`}>{runLabel(run, text)}</span></td>
          </tr>)}</tbody>
        </table></div>}
      </section>
      <section aria-labelledby="history-store" className="history-card">
        <h2 id="history-store">{text("Local history store", "本机历史库")}</h2>
        {store.state === "available" ? <>
          <dl className="history-facts">
            <dt>{text("Retention", "保留")}</dt><dd>{text(`${store.retention_days} days, expired data deleted automatically`, `${store.retention_days} 天，过期自动删除`)}</dd>
            <dt>{text("Size", "占用")}</dt><dd>{bytesText(store.file_bytes)}</dd>
            <dt>{text("Templates", "模板")}</dt><dd>{store.templates}</dd>
            <dt>{text("Hourly buckets", "小时桶")}</dt><dd>{store.buckets}</dd>
          </dl>
          <p className="history-muted">{text("Keeps: literal-free templates and fingerprints, hourly aggregates, collection watermarks.", "保存：脱敏模板与指纹、小时桶聚合、采集水位。")}</p>
          <p className="history-muted">{text("Never keeps: SQL literals, Profile text, StarRocks credentials.", "不保存：原始 SQL 字面量、原始 Profile 文本、StarRocks 凭据。")}</p>
          <button className="button danger" disabled={busy} onClick={onClear} type="button">{text("Clear history…", "清空历史…")}</button>
          {clearConfirm}
        </> : <p className="history-muted">{store.state === "disabled" ? text("History is off in this setup (DOCTOR_HISTORY=off); nothing is collected.", "此运行方式未开启历史（DOCTOR_HISTORY=off），不会采集。") : text("The history file could not be opened; nothing is collected.", "无法打开历史文件，不会采集。")}</p>}
      </section>
    </div>
  </div>;
}
