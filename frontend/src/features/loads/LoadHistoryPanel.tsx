// Design boards I9-I11 (task 58): the load history tab. Thirty days of loads,
// Routine Load jobs and saved error logs from the local history store, as the
// one bounded collector recorded them. Days are UTC days; every count, height,
// rate and coverage state arrives computed, and this only lays them out.
import { useState } from "react";
import type { JSX, KeyboardEvent } from "react";
import type {
  HistoryLoadCoverage, LoadErrorCategory, LoadHistoryDay, LoadHistoryErrorLogsResponse, LoadHistoryResponse,
  LoadHistoryRoutineResponse, LoadKindGroup, RoutineHistoryState, StoredLoadErrorLog, StoredLoadErrorLogState,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { clockText, collectionLabel, useHistoryStatus } from "../history/history-status";
import { useMonitorRead } from "../history/monitor-read";
import { MonitorReadError, RefreshBar } from "../history/MonitorStates";
import { KpiCards, type KpiCard } from "./LoadOverviewPanel";
import type { ErrorLogTarget } from "./LoadErrorLogDialog";
import { bytes, categoryText, compact, count, groupText, loadFailureText, shareText } from "./load-text";
import "./load-history.css";

type Mode = "stats" | "errors";
const KIND_CLASS: Record<LoadKindGroup, string> = { stream_load: "stream", insert: "insert", routine_load: "routine", broker: "broker", other: "other" };
/** Bottom to top in a stacked day. */
const STACK: LoadKindGroup[] = ["other", "broker", "routine_load", "insert", "stream_load"];

/** `MM-DD` of a UTC day start. */
export function utcDay(seconds: number): string {
  return new Date(seconds * 1000).toISOString().slice(5, 10);
}

/** Parts per million as a percent, with enough digits for small rates. */
export function ppmText(ppm: number | string | null | undefined): string {
  if (ppm === null || ppm === undefined) return "—";
  const value = Number(ppm);
  if (!Number.isFinite(value)) return "—";
  if (value === 0) return "0%";
  const percent = value / 10_000;
  return `${percent >= 1 ? percent.toFixed(1) : percent >= 0.01 ? percent.toFixed(2) : percent.toFixed(3)}%`;
}

/** A few evenly spaced UTC day labels for a time axis. */
function DateTicks({ starts }: { starts: number[] }): JSX.Element {
  // Evenly spaced labels sit where their buckets are; prefer a count that
  // divides the span exactly.
  const count = Math.min([5, 4, 3].find((n) => (starts.length - 1) % (n - 1) === 0) ?? 5, starts.length);
  const picks = count <= 1 ? starts.slice(0, 1) : Array.from({ length: count }, (_, index) => starts[Math.round((index * (starts.length - 1)) / (count - 1))]);
  return <div aria-hidden="true" className="load-hour-labels">{picks.map((start, index) => <span key={`${start}:${index}`}>{utcDay(start)}</span>)}</div>;
}

function coverageText(coverage: HistoryLoadCoverage, text: LocalizedText): string {
  return coverage === "complete" ? text("complete", "完整") : coverage === "partial" ? text("partial", "部分") : text("not collected", "未采集");
}

function partialReasonText(reason: string, text: LocalizedText): string {
  return {
    row_limit: text("an hour hit the 2000-row read limit", "有时段读到 2000 条上限"),
    stream_records_cleared: text("StarRocks cleared finished Stream Loads / Routine Load tasks before they were read", "读取前 StarRocks 已清掉已结束的 Stream Load / Routine Load 任务"),
    load_records_cleared: text("StarRocks cleared INSERT / Broker Load records before they were read", "读取前 StarRocks 已清掉 INSERT / Broker Load 记录"),
    first_look_unproven: text("first collection: StarRocks held nothing older, so completeness cannot be shown", "首次采集时 StarRocks 中没有更早的记录，无法确认这段是否完整"),
  }[reason] ?? reason;
}

function logStateText(state: StoredLoadErrorLogState, log: StoredLoadErrorLog, text: LocalizedText): string {
  switch (state) {
    case "saved": return log.truncated ? text("Saved · cut at the limit", "已保存 · 已截断") : text("Saved", "已保存");
    case "pending": return log.attempts > 0 ? text(`Read failed · retrying (attempt ${log.attempts + 1})`, `读取失败 · 下轮重试（第 ${log.attempts + 1} 次）`) : text("Waiting to be read", "等待读取");
    case "purged": return text("Cleared by StarRocks before it was read", "读取前已被清理");
    case "expired": return text("Not read within StarRocks' 48 hours", "未能在 StarRocks 保留期内读取");
    case "failed": return text("Reads kept failing", "多次读取失败");
    case "no_log": return text("StarRocks has no error log", "StarRocks 没有错误日志");
    case "trimmed": return text("Text dropped under the 50 MB limit", "原文已按 50 MB 上限删除");
  }
}

function logTone(state: StoredLoadErrorLogState): string {
  return state === "saved" ? "ok" : state === "pending" ? "warn" : state === "trimmed" ? "info" : "off";
}

function routineStateText(state: RoutineHistoryState, text: LocalizedText): string {
  return { running: text("Running", "运行"), scheduling: text("Scheduling", "等待调度"), paused: text("Paused", "暂停"), stopped: text("Stopped", "已停止"), cancelled: text("Cancelled", "已取消"), absent: text("Not present", "不存在"), not_collected: text("Not collected", "未采集"), unknown: text("Unknown", "未知") }[state];
}

function reasonDetail(column: string | null, targetType: string | null): string {
  return [column, targetType].filter(Boolean).join(" · ");
}

/** The pause control and what the shared collector has read for loads. */
function CollectionStrip({ connectionId, history, text }: { connectionId: string; history: LoadHistoryResponse | null; text: LocalizedText }): JSX.Element {
  const { status, refresh } = useHistoryStatus(connectionId);
  const [busy, setBusy] = useState(false);
  const state = status?.collection.state;
  const toggle = (action: "pause" | "resume") => {
    setBusy(true);
    doctorApi.setHistoryCollection({ connection_id: connectionId, action }).finally(() => { setBusy(false); refresh(); });
  };
  const collection = history?.collection;
  if (status?.target && history?.target && status.target !== history.target) {
    return <p className="history-banner neutral" role="status">{text(`Showing the saved history of ${history.target}; the collection status belongs to the connected cluster ${status.target}.`, `显示本机保存的 ${history.target} 的历史；当前连接的是 ${status.target}，采集状态与暂停开关不适用于这里。`)}</p>;
  }
  return <div className="history-card load-history-strip">
    <div className="load-history-strip-main">
      {status && <span className={`load-history-state ${state === "running" ? "ok" : state === "paused" ? "warn" : "off"}`}><i aria-hidden="true" />{collectionLabel(status, text)}</span>}
      <span>{text("Read up to", "已采到")} <span className="mono">{clockText(collection?.watermark_seconds)}</span> UTC</span>
      {collection?.last_rows_read !== null && collection?.last_rows_read !== undefined && <span>{text(`last read ${collection.last_rows_read} loads`, `最近一轮读取 ${collection.last_rows_read} 条导入`)}</span>}
      <span className="load-history-strip-end">{text("Local history store · kept 30 days", "本机历史库 · 保留 30 天")}</span>
      {(state === "running" || state === "backoff") && <button className="button secondary" disabled={busy} onClick={() => toggle("pause")} type="button">{text("Pause collection", "暂停采集")}</button>}
      {state === "paused" && <button className="button secondary" disabled={busy} onClick={() => toggle("resume")} type="button">{text("Resume collection", "继续采集")}</button>}
    </div>
    <span className="load-history-strip-note">{text("Shares the audit history's collector, every 5 minutes; finished loads go into the hour they finished (UTC days)", "和审计历史共用同一个采集任务，每 5 分钟读一次；已结束的导入按完成时间归入小时（按 UTC 日）")}</span>
  </div>;
}

/** A segmented single choice: one Tab stop, arrow keys move and select. */
function Segment<T extends string | number>({ label, options, value, onChange }: { label: string; options: Array<{ value: T; label: JSX.Element | string }>; value: T; onChange(value: T): void }): JSX.Element {
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = options.findIndex((option) => option.value === value);
    const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const next = (at + step + options.length) % options.length;
    onChange(options[next].value);
    event.currentTarget.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
  };
  return <div aria-label={label} className="history-segment" onKeyDown={onKeyDown} role="radiogroup">
    {options.map((option) => <button aria-checked={option.value === value} key={String(option.value)} onClick={() => onChange(option.value)} role="radio" tabIndex={option.value === value ? 0 : -1} type="button">{option.label}</button>)}
  </div>;
}

type KindOption = { kind: LoadKindGroup | null; label: string; count: string };

function KindSwitch({ options, kind, onKind, text }: { options: KindOption[]; kind: LoadKindGroup | null; onKind(kind: LoadKindGroup | null): void; text: LocalizedText }): JSX.Element {
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = options.findIndex((option) => option.kind === kind);
    const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const next = options[(at + step + options.length) % options.length];
    onKind(next.kind);
    event.currentTarget.querySelector<HTMLButtonElement>(`button[data-kind="${next.kind ?? "all"}"]`)?.focus();
  };
  return <div aria-label={text("Load type", "导入类型")} className="load-type-switch" onKeyDown={onKeyDown} role="radiogroup">
    {options.map((option) => <button aria-checked={option.kind === kind} aria-label={text(`${option.label} (${option.count})`, `${option.label}（${option.count}）`)} data-kind={option.kind ?? "all"} key={option.kind ?? "all"} onClick={() => onKind(option.kind)} role="radio" tabIndex={option.kind === kind ? 0 : -1} type="button">
      <span>{option.label}</span><span className="mono">{option.count}</span>
    </button>)}
  </div>;
}

function HistoryKpis({ history, kind, text }: { history: LoadHistoryResponse; kind: LoadKindGroup | null; text: LocalizedText }): JSX.Element {
  const totals = history.totals;
  // The per-type breakdown only when every type is shown.
  const failedByKind = kind === null ? history.kinds.filter((item) => item.failed !== "0").map((item) => `${groupText(item.kind, text)} ${count(item.failed)}`).join(" · ") : "";
  const storage = history.error_logs;
  const cards: KpiCard[] = [
    { label: text("Finished loads", "已结束的导入"), value: count(totals.loads), unit: text("loads", "次"),
      note: history.segments.length > 1 ? text(`${history.segments.length} version segments, not compared`, `跨 ${history.segments.length} 个版本段，不跨段比较`) : text("by finish time", "按完成时间"),
      caveat: totals.not_collected_hours > 0 ? text(`${totals.not_collected_hours} hours not collected, not counted`, `${totals.not_collected_hours} 小时未采集，未计入`) : null },
    { label: text("Failed / cancelled", "失败 / 取消"), value: count(totals.failed), unit: ppmText(totals.failed_ppm), note: failedByKind || (totals.failed === "0" ? text("none", "没有失败") : text("of finished loads", "占已结束导入")), bad: totals.failed !== "0" },
    { label: text("Rows written", "写入行"), value: compact(totals.loaded_rows), unit: text("rows", "行"), note: totals.partial_hours > 0 ? text(`${totals.partial_hours} partly read hours may be low`, `${totals.partial_hours} 个部分采集的小时可能少计`) : text("from SINK_ROWS", "来自 SINK_ROWS"),
      caveat: totals.loaded_unknown > 0 ? text(`${totals.loaded_unknown} loads reported no row count, left out`, `${totals.loaded_unknown} 次没有报告行数，未计入`) : null },
    { label: text("Filtered rows", "过滤行"), value: count(totals.filtered_rows), unit: ppmText(totals.filtered_ppm), note: text(`of input rows · ${totals.tables_with_filtered} tables filtered`, `占输入行 · ${totals.tables_with_filtered} 张表有过滤`),
      caveat: totals.filtered_unknown > 0 ? text(`${totals.filtered_unknown} loads reported no filtered count, left out`, `${totals.filtered_unknown} 次没有报告过滤行，未计入`) : null },
    { label: text("Saved error logs", "已保存的错误日志"), value: count(String(storage.saved)), unit: text("logs", "份"), note: text(`${bytes(storage.raw_bytes)} / 50 MB · ${storage.missed} cleared before reading`, `${bytes(storage.raw_bytes)} / 50 MB · ${storage.missed} 份读取前已被清理`) },
  ];
  const gaps = totals.not_collected_hours > 0 || totals.partial_hours > 0;
  return <>
    {gaps && <p className="load-history-scope" role="note">{text(`Collected time only: ${totals.not_collected_hours} hours not collected and ${totals.partial_hours} partly read in this range; nothing is filled in.`, `只统计已采集的时段：这段时间有 ${totals.not_collected_hours} 小时未采集、${totals.partial_hours} 小时为部分采集，不补数据。`)}</p>}
    <KpiCards cards={cards} label={text("Load history totals", "导入历史汇总")} />
  </>;
}

function DayDetail({ day, history, text }: { day: LoadHistoryDay; history: LoadHistoryResponse; text: LocalizedText }): JSX.Element {
  const segment = history.segments[day.segment];
  return <div className="load-hour-detail" role="status">
    <strong>{utcDay(day.start_seconds)}（{coverageText(day.coverage, text)}）</strong>
    {day.loads === null ? <span>{text("Not collected: SR Doctor was not connected, or collection was paused. Nothing is filled in.", "未采集：当时没有连接或已暂停采集，不补数据。")}</span> : <>
      {day.by_kind.map((kind) => <span key={kind.kind}>{groupText(kind.kind, text)} <span className="mono">{count(kind.loads)}</span></span>)}
      <span className={day.failed !== "0" ? "bad" : undefined}>{text("failed", "失败")} <span className="mono">{count(day.failed)}</span></span>
      <span>{text("filtered rows", "过滤行")} <span className="mono">{count(day.filtered_rows)}</span></span>
      {day.unknown_counts > 0 && <span className="load-partial-note">{text(`${day.unknown_counts} counts not reported, left out`, `${day.unknown_counts} 个计数没有报告，未计入`)}</span>}
    </>}
    {day.partial_reasons.map((reason) => <span className="load-partial-note" key={reason}>{partialReasonText(reason, text)}</span>)}
    {day.not_collected_hours > 0 && day.loads !== null && <span className="load-partial-note">{text(`${day.not_collected_hours} hours not collected`, `${day.not_collected_hours} 小时未采集`)}</span>}
    {segment && <span className="history-muted">StarRocks {segment.engine_versions.join(", ")}</span>}
  </div>;
}

function DailyChart({ history, text }: { history: LoadHistoryResponse; text: LocalizedText }): JSX.Element {
  const days = history.days;
  const [picked, setPicked] = useState<number | null>(null);
  const latest = (() => { for (let index = days.length - 1; index >= 0; index -= 1) if (days[index].loads !== null) return index; return days.length - 1; })();
  const selected = days[picked ?? latest];
  const boundaries = days.map((day, index) => ({ day, index })).filter(({ day, index }) => index > 0 && day.segment !== days[index - 1].segment && day.loads !== null);
  const axis = [...history.count_axis].reverse();
  return <section aria-labelledby="load-history-daily" className="history-card load-history-section">
    <div className="history-card-head">
      <h2 id="load-history-daily">{text("Loads per day by type", "每天导入次数 · 按类型")} <span className="history-muted">{text("by finish time (UTC days) · collection coverage below", "按完成时间（UTC 日）· 下方为采集覆盖")}</span></h2>
      <span className="load-legend">
        {(["stream_load", "insert", "routine_load", "broker"] as LoadKindGroup[]).map((kind) => <span key={kind}><i className={`kind-${KIND_CLASS[kind]}`} />{groupText(kind, text)}</span>)}
        <span><i className="fail-line" />{text("days with failures", "有失败的天")}</span>
      </span>
    </div>
    <div className="load-history-chart">
      <div aria-hidden="true" className="load-axis tall">{axis.map((tick) => <span key={tick}>{compact(tick)}</span>)}</div>
      <div className="load-history-plot">
        {days.map((day, index) => <button aria-label={day.loads === null ? text(`${utcDay(day.start_seconds)} not collected`, `${utcDay(day.start_seconds)} 未采集`) : text(`${utcDay(day.start_seconds)}: ${count(day.loads)} loads, ${count(day.failed)} failed, ${coverageText(day.coverage, text)}`, `${utcDay(day.start_seconds)}：${count(day.loads)} 次导入，失败 ${count(day.failed)}，${coverageText(day.coverage, text)}`)} aria-pressed={(picked ?? latest) === index} className={`load-history-day${day.loads === null ? " gone" : ""}`} key={day.start_seconds} onClick={() => setPicked(index)} type="button">
          <span className={day.failed !== null && day.failed !== "0" ? "fail-mark on" : "fail-mark"} />
          {STACK.map((kind) => {
            const part = day.by_kind.find((item) => item.kind === kind);
            return part ? <span className={`stack kind-${KIND_CLASS[kind]}${day.coverage === "partial" ? " partial" : ""}`} key={kind} style={{ height: `${part.height_permyriad / 100}%` }} /> : null;
          })}
        </button>)}
        {boundaries.map(({ day, index }) => <span className="load-history-boundary" key={day.start_seconds} style={{ left: `${(index / days.length) * 100}%` }}>
          {text(`from ${utcDay(day.start_seconds)}: StarRocks ${history.segments[day.segment]?.engine_versions.join(", ")}; not compared across`, `${utcDay(day.start_seconds)} 起 StarRocks ${history.segments[day.segment]?.engine_versions.join(", ")} · 不跨段比较`)}
        </span>)}
      </div>
      <span className="history-muted load-history-cov-label">{text("Coverage", "覆盖")}</span>
      <div className="load-history-coverage">
        {days.map((day) => <span className={`cov-${day.coverage}`} key={day.start_seconds} title={`${utcDay(day.start_seconds)} ${coverageText(day.coverage, text)}`} />)}
      </div>
      <span />
      <DateTicks starts={days.map((day) => day.start_seconds)} />
    </div>
    <span className="load-legend">
      <span><i className="cov-complete" />{text("complete", "完整")}</span>
      <span><i className="cov-partial" />{text("partial (records cleared by StarRocks first, or the read limit hit)", "部分（有小时被 StarRocks 提前清理或读到上限）")}</span>
      <span><i className="cov-not_collected" />{text("not collected (not connected or paused); nothing is filled in", "未采集（未连接或已暂停），不补数据")}</span>
    </span>
    {selected && <DayDetail day={selected} history={history} text={text} />}
  </section>;
}

type Run = { points: string; dot: boolean };

/** Polyline runs between uncollected points (and across version segments),
 * never bridged; a run of one point is drawn as a dot. */
function runs(values: Array<number | null>, width: number, segments?: number[]): Run[] {
  const out: Run[] = [];
  let current: string[] = [];
  const flush = () => { if (current.length) out.push({ points: (current.length === 1 ? [current[0], current[0]] : current).join(" "), dot: current.length === 1 }); current = []; };
  values.forEach((value, index) => {
    if (segments && index > 0 && segments[index] !== segments[index - 1]) flush();
    if (value === null) { flush(); return; }
    current.push(`${(index + 0.5) * width},${100 - value / 100}`);
  });
  flush();
  return out;
}

function RatesChart({ history, text }: { history: LoadHistoryResponse; text: LocalizedText }): JSX.Element {
  const days = history.days;
  const width = 10;
  const segments = days.map((day) => day.segment);
  const failed = runs(days.map((day) => day.failed_rate_y_permyriad), width, segments);
  const filtered = runs(days.map((day) => day.filtered_rate_y_permyriad), width, segments);
  const last = [...days].reverse().find((day) => day.failed_rate_ppm !== null);
  return <section aria-labelledby="load-history-rates" className="history-card load-history-section load-history-rates">
    <h2 id="load-history-rates">{text("Failure and filter rates per day", "失败率与过滤率 · 按天")} <span className="history-muted">{text("failed ÷ finished; filtered ÷ input rows", "失败 ÷ 已结束导入；过滤行 ÷ 输入行")}</span></h2>
    <div className="load-history-chart small">
      <div aria-hidden="true" className="load-axis short">{[...history.rate_axis].reverse().map((tick) => <span key={tick}>{ppmText(tick)}</span>)}</div>
      <svg aria-label={last ? text(`Failure and filter rates per day; latest ${ppmText(last.failed_rate_ppm)} failed, ${ppmText(last.filtered_rate_ppm)} filtered`, `每天的失败率与过滤率；最近一天失败 ${ppmText(last.failed_rate_ppm)}、过滤 ${ppmText(last.filtered_rate_ppm)}`) : text("No collected days", "没有采集到的天")} className="load-history-lines" preserveAspectRatio="none" role="img" viewBox={`0 0 ${days.length * width} 100`}>
        {failed.map((run) => <polyline className={run.dot ? "rate-failed dot" : "rate-failed"} key={`f${run.points}`} points={run.points} vectorEffect="non-scaling-stroke" />)}
        {filtered.map((run) => <polyline className={run.dot ? "rate-filtered dot" : "rate-filtered"} key={`r${run.points}`} points={run.points} vectorEffect="non-scaling-stroke" />)}
      </svg>
      <span />
      <DateTicks starts={days.map((day) => day.start_seconds)} />
    </div>
    <span className="load-legend">
      <span><i className="line-failed" />{text("failure rate", "失败率")}</span>
      <span><i className="line-filtered" />{text("filter rate (square points)", "过滤率（方点）")}</span>
      <span className="history-muted">{history.segments.length > 1 ? text("Uncollected days and StarRocks version changes break the line", "未采集的天与 StarRocks 版本变化处断开，不连线") : text("Uncollected days break the line", "未采集的天断开，不连线")}</span>
    </span>
  </section>;
}

function TableList({ history, onOpenLogs, text }: { history: LoadHistoryResponse; onOpenLogs(table: string): void; text: LocalizedText }): JSX.Element {
  return <section aria-labelledby="load-history-tables" className="history-card load-history-section load-history-tables">
    <h2 id="load-history-tables">{text("By table", "按表")} <span className="history-muted">{text("sorted by filtered rows · reasons from saved error log samples", "按过滤行排序 · 原因来自已保存的错误日志样本")}</span></h2>
    {history.tables.length === 0 ? <p className="notify-empty">{text("No finished loads collected in this range.", "这段时间没有采集到已结束的导入。")}</p> : <div className="history-table-wrap">
      <table className="history-table load-table">
        <thead><tr>
          <th scope="col">{text("Table", "表")}</th><th scope="col">{text("Type", "类型")}</th><th className="num" scope="col">{text("Loads", "次数")}</th><th className="num" scope="col">{text("Failed", "失败")}</th>
          <th className="num" scope="col">{text("Rows written", "写入行")}</th><th className="num" scope="col">{text("Filtered", "过滤行")}</th><th scope="col">{text("Top filter reason (sample)", "主要过滤原因（样本）")}</th><th scope="col"><span className="load-visually-hidden">{text("Actions", "操作")}</span></th>
        </tr></thead>
        <tbody>
          {history.tables.map((row) => {
            const name = `${row.database}.${row.table}`;
            const reason = row.top_reason;
            return <tr key={name}>
              <th className="mono" scope="row">{name}</th>
              <td>{row.kinds.map((kind) => groupText(kind, text)).join(" · ")}</td>
              <td className="num">{count(row.loads)}</td>
              <td className={`num${row.failed !== "0" ? " bad" : ""}`}>{count(row.failed)}</td>
              <td className="num">{compact(row.loaded_rows)}</td>
              <td className="num">{count(row.filtered_rows)}</td>
              <td>{reason ? <>
                <span>{categoryText(reason.category, text)}{reasonDetail(reason.column, reason.target_type) && <span className="mono"> {reasonDetail(reason.column, reason.target_type)}</span>} · {text(`${shareText(reason.sample_share_permyriad)} of the sample`, `样本内占比 ${shareText(reason.sample_share_permyriad)}`)}</span>
                {reason.new_since_seconds !== null && <span className="load-new-reason">{text(`new · ${utcDay(reason.new_since_seconds)}`, `新出现 · ${utcDay(reason.new_since_seconds)}`)}</span>}
              </> : <span className="history-muted">{row.filtered_rows === "0" ? text("no filtered rows", "没有过滤行") : text("no saved error log", "没有保存的错误日志")}</span>}</td>
              <td className="num">{row.saved_logs > 0 ? <button className="button secondary small" onClick={() => onOpenLogs(name)} type="button">{text(`Error logs ${row.saved_logs}`, `错误日志 ${row.saved_logs}`)}</button> : null}</td>
            </tr>;
          })}
        </tbody>
      </table>
    </div>}
    <p className="history-muted">{text("Filtered rows and failures are StarRocks' exact counts; reason shares come only from saved error logs (about 50 rows per load), not from every filtered row.", "过滤行与失败次数是 StarRocks 的准确计数；原因占比只来自保存下来的错误日志（每次导入最多约 50 行），不代表全部过滤行。")}</p>
  </section>;
}

export function StatsView({ history, kind = null, onOpenLogs, text }: { history: LoadHistoryResponse; kind?: LoadKindGroup | null; onOpenLogs(table: string): void; text: LocalizedText }): JSX.Element {
  return <>
    <HistoryKpis history={history} kind={kind} text={text} />
    <DailyChart history={history} key={`${history.days.length}:${history.days[0]?.start_seconds ?? 0}`} text={text} />
    <div className="history-row">
      <RatesChart history={history} text={text} />
      <TableList history={history} onOpenLogs={onOpenLogs} text={text} />
    </div>
  </>;
}

export function RoutineView({ routine, onJob, text }: { routine: LoadHistoryRoutineResponse; onJob(jobId: string): void; text: LocalizedText }): JSX.Element {
  const detail = routine.selected;
  if (routine.jobs.length === 0) return <p className="notify-empty">{text("No Routine Load job snapshots in this range. Jobs are snapshotted on every collection cycle from StarRocks 3.2.", "这段时间没有 Routine Load 作业快照。StarRocks 3.2 起，每轮采集都会给作业拍一次快照。")}</p>;
  const pausedRuns: Array<[number, number]> = [];
  const gapRuns: Array<[number, number]> = [];
  detail?.lag.forEach((point, index) => {
    const push = (list: Array<[number, number]>) => { const last = list[list.length - 1]; if (last && last[1] === index) last[1] = index + 1; else list.push([index, index + 1]); };
    if (point.paused) push(pausedRuns);
    if (!point.collected) push(gapRuns);
  });
  const lagLines = detail ? runs(detail.lag.map((point) => point.y_permyriad), 1) : [];
  return <>
    <section aria-labelledby="load-history-jobs" className="history-card load-history-section">
      <div className="history-card-head">
        <h2 id="load-history-jobs">{text("Jobs", "作业")} <span className="history-muted">{routine.cell_seconds >= 3600 * 6 ? text("each cell 6 hours", "状态条每格 6 小时") : text("each cell 1 hour", "状态条每格 1 小时")} · {text("increase = difference of two close snapshots' lifetime counters", "增量 = 相邻两次快照的累计值之差")}</span></h2>
        <span className="load-legend">
          <span><i className="state-running" />{text("running", "运行")}</span>
          <span><i className="state-paused" />{text("paused", "暂停")}</span>
          <span><i className="state-cancelled" />{text("cancelled / stopped", "已取消 / 已停止")}</span>
          <span><i className="state-not_collected" />{text("not collected", "未采集")}</span>
        </span>
      </div>
      <div className="history-table-wrap">
        <table className="history-table load-table">
          <thead><tr>
            <th scope="col">{text("Job", "作业")}</th><th className="load-history-cells-col" scope="col">{text("State", "状态")}</th><th className="num" scope="col">{text("Pauses seen", "观察到的暂停")}</th>
            <th className="num" scope="col">{text("Rows loaded (seen)", "导入行（已观测）")}</th><th className="num" scope="col">{text("Error rows (seen)", "错误行（已观测）")}</th><th className="num" scope="col">{text("Committed / aborted (seen)", "提交 / 中止任务（已观测）")}</th><th className="num" scope="col">{text("Max lag seen", "观察到的最大积压")}</th>
          </tr></thead>
          <tbody>
            {routine.jobs.map((job) => <tr className={detail?.job_id === job.job_id ? "selected" : undefined} key={job.job_id}>
              <th scope="row"><button aria-pressed={detail?.job_id === job.job_id} className="load-link" onClick={() => onJob(job.job_id)} type="button"><strong className="mono">{job.name}</strong><span className="load-sub history-muted mono">{job.database}.{job.table}</span></button></th>
              <td><span aria-label={text(`${job.name}: now ${job.current_state}, paused ${job.pauses} times`, `${job.name}：当前 ${job.current_state}，暂停 ${job.pauses} 次`)} className="load-history-cells" role="img">
                {job.cells.map((cell) => <span className={`state-${cell.state}`} key={cell.start_seconds} title={`${clockText(cell.start_seconds).slice(5, 16)} UTC · ${routineStateText(cell.state, text)}`} />)}
              </span><DateTicks starts={job.cells.map((cell) => cell.start_seconds)} /></td>
              <td className={`num${job.pauses > 0 ? " bad" : ""}`}>{job.pauses}</td>
              <td className="num">{job.loaded_rows === null ? text("unknown", "未知") : compact(job.loaded_rows)}</td>
              <td className="num">{job.error_rows === null ? text("unknown", "未知") : count(job.error_rows)}</td>
              <td className="num">{job.committed_tasks === null ? text("unknown", "未知") : `${count(job.committed_tasks)} / ${count(job.aborted_tasks)}`}</td>
              <td className="num">{job.max_lag ? count(job.max_lag) : text("unknown", "未知")}</td>
            </tr>)}
          </tbody>
        </table>
      </div>
      {routine.jobs.some((job) => job.unknown_intervals > 0) && <p className="history-muted">{text("Intervals where a counter went down (job recreated) or snapshots were far apart are unknown and left out of the sums.", "累计值变小（作业重建）或两次快照相隔太久的区间记为未知，不计入增量。")}</p>}
    </section>
    {detail && <div className="history-row">
      <section aria-labelledby="load-history-lag" className="history-card load-history-section load-history-lag">
        <h2 id="load-history-lag">{detail.name} · {text("partition lag", "分区积压")} <span className="history-muted">{text("largest partition lag per hour (OFFSET_LAG, messages)", "每小时快照中的最大分区积压（OFFSET_LAG，条）")}</span></h2>
        {detail.lag_known ? <div className="load-history-chart">
          <div aria-hidden="true" className="load-axis">{[...detail.lag_axis].reverse().map((tick) => <span key={tick}>{compact(tick)}</span>)}</div>
          <svg aria-label={text(`${detail.name}: largest partition lag per hour`, `${detail.name} 每小时最大分区积压`)} className="load-history-lines" preserveAspectRatio="none" role="img" viewBox={`0 0 ${detail.lag.length} 100`}>
            <defs><pattern height="6" id="lag-gap-hatch" patternTransform="rotate(45)" patternUnits="userSpaceOnUse" width="6"><rect fill="#eceff2" height="6" width="6" /><rect fill="#c9cfd7" height="6" width="2" /></pattern></defs>
            {gapRuns.map(([from, to]) => <rect className="lag-gap" fill="url(#lag-gap-hatch)" height="100" key={`g${from}`} width={to - from} x={from} y="0" />)}
            {pausedRuns.map(([from, to]) => <rect className="lag-paused" height="100" key={`p${from}`} width={to - from} x={from} y="0" />)}
            {lagLines.map((run) => <polyline className={run.dot ? "lag-line dot" : "lag-line"} key={run.points} points={run.points} vectorEffect="non-scaling-stroke" />)}
          </svg>
          <span />
          <DateTicks starts={detail.lag.map((point) => point.start_seconds)} />
        </div> : <p className="notify-empty">{text("StarRocks before 3.3 reports no OFFSET_LAG; lag is unknown.", "3.3 之前的 StarRocks 没有 OFFSET_LAG，积压未知。")}</p>}
        <span className="load-legend"><span><i className="lag-line" />{text("largest partition lag", "最大分区积压")}</span><span><i className="lag-paused" />{text("paused", "暂停期间")}</span><span><i className="state-not_collected" />{text("not collected", "未采集")}</span></span>
        <p className="history-muted">{text("Lag is taken only at snapshot times; peaks between snapshots are not seen.", "积压只在快照时刻取值，两次快照之间的峰值看不到。")}</p>
      </section>
      <section aria-labelledby="load-history-events" className="history-card load-history-section load-side">
        <h2 id="load-history-events">{detail.name} · {text("state changes", "状态变化")} <span className="history-muted">{text("seen between snapshots", "快照之间观察到的")}</span></h2>
        {detail.events.length === 0 ? <p className="notify-empty">{text("No state change in this range.", "这段时间没有状态变化。")}</p> : <ol className="load-history-events">
          {detail.events.map((event) => <li key={event.at_seconds}>
            <span className="mono history-muted">{clockText(event.at_seconds).slice(5, 16)}</span>
            <span><span className={`notify-chip ${event.to_state === "RUNNING" ? "ok" : event.to_state === "PAUSED" ? "warn" : "off"}`}>{event.from_state ? event.to_state : text(`first seen · ${event.to_state}`, `首次观察 · ${event.to_state}`)}</span></span>
            {event.reason && <span className="load-history-event-reason">{event.reason}</span>}
          </li>)}
        </ol>}
        <p className="history-muted">{text("Times are the snapshot that saw the state (UTC), not when it changed; reasons from REASONS_OF_STATE_CHANGED.", "时间是观察到该状态的快照时刻（UTC），不代表真实切换时刻；原因取自 REASONS_OF_STATE_CHANGED。")}</p>
      </section>
    </div>}
    {detail && <section aria-labelledby="load-history-routine-days" className="history-card load-history-section">
      <h2 id="load-history-routine-days">{detail.name} · {text("rows and error rows per day", "每天导入行与错误行")} <span className="history-muted">{text("increase of lifetime counters; unknown spans stay out", "累计值的增量；作业重建或累计值变小的那一段记为未知")}</span></h2>
      <div className="load-history-chart">
        <div aria-hidden="true" className="load-axis short">{[...detail.rows_axis].reverse().map((tick) => <span key={tick}>{compact(tick)}</span>)}</div>
        <div className="load-history-plot short">
          {detail.days.map((day) => <span aria-label={!day.collected ? text(`${utcDay(day.start_seconds)} not collected`, `${utcDay(day.start_seconds)} 未采集`) : day.loaded_rows === null ? text(`${utcDay(day.start_seconds)} unknown`, `${utcDay(day.start_seconds)} 未知`) : text(`${utcDay(day.start_seconds)}: ${count(day.loaded_rows)} rows, ${count(day.error_rows)} error rows`, `${utcDay(day.start_seconds)}：导入行 ${count(day.loaded_rows)}，错误行 ${count(day.error_rows)}`)} className={`load-history-day static${!day.collected ? " gone" : day.loaded_rows === null ? " unknown" : ""}`} key={day.start_seconds} role="img">
            <span className="stack kind-error" style={{ height: `${day.error_height_permyriad / 100}%` }} />
            <span className="stack kind-routine" style={{ height: `${day.loaded_height_permyriad / 100}%` }} />
          </span>)}
        </div>
        <span />
        <DateTicks starts={detail.days.map((day) => day.start_seconds)} />
      </div>
      <span className="load-legend"><span><i className="kind-routine" />{text("rows loaded", "导入行")}</span><span><i className="kind-error" />{text("error rows", "错误行")}</span></span>
    </section>}
  </>;
}

export function ErrorLogsView({ logs, table, category, onTable, onCategory, offset, onOffset, onOpen, text }: {
  logs: LoadHistoryErrorLogsResponse;
  table: string | null; category: LoadErrorCategory | null; offset: number;
  onTable(table: string | null): void; onCategory(category: LoadErrorCategory | null): void; onOffset(offset: number): void;
  onOpen(log: StoredLoadErrorLog): void;
  text: LocalizedText;
}): JSX.Element {
  const storage = logs.storage;
  const categories: LoadErrorCategory[] = ["out_of_range", "too_long", "column_count", "null_in_non_nullable", "out_of_partition", "json_cast", "json_structure", "other"];
  const page = 50;
  return <>
    <div className="load-history-filters">
      <label className="notify-filter">{text("Table", "表")}
        <select onChange={(event) => onTable(event.target.value || null)} value={table ?? ""}>
          <option value="">{text(`All tables (${logs.table_options.length})`, `全部表（${logs.table_options.length}）`)}</option>
          {logs.table_options.map((name) => <option key={name} value={name}>{name}</option>)}
        </select>
      </label>
      <label className="notify-filter">{text("Reason", "原因")}
        <select onChange={(event) => onCategory((event.target.value || null) as LoadErrorCategory | null)} value={category ?? ""}>
          <option value="">{text("All", "全部")}</option>
          {categories.map((item) => <option key={item} value={item}>{categoryText(item, text)}</option>)}
        </select>
      </label>
    </div>
    <div className="history-row">
      <section aria-labelledby="load-history-store" className="history-card load-history-section load-side">
        <h2 id="load-history-store">{text("Storage", "保存情况")}</h2>
        <div className="load-history-store-line"><strong className="mono">{bytes(storage.raw_bytes)}</strong><span className="history-muted">{text(`/ 50 MB text limit · ${storage.saved} logs`, `/ 50 MB 原文上限 · ${storage.saved} 份`)}{storage.earliest_saved_seconds !== null && text(` · earliest ${clockText(storage.earliest_saved_seconds).slice(5, 16)} UTC`, ` · 最早 ${clockText(storage.earliest_saved_seconds).slice(5, 16)} UTC`)}</span></div>
        <div aria-label={text(`${bytes(storage.raw_bytes)} used, ${shareText(storage.used_permyriad)} of the limit`, `已用 ${bytes(storage.raw_bytes)}，占上限 ${shareText(storage.used_permyriad)}`)} className="load-history-meter" role="img"><span style={{ width: `${storage.used_permyriad / 100}%` }} /></div>
        <ul className="notify-notes">
          <li>{text("A load with filtered rows is read once in the next cycle after it finishes (at most 5 per cycle, 50 rows and 64 KB each, rows over 4 KB cut).", "有过滤行的导入结束后，下一轮采集读取一次（每轮最多 5 份，每份最多 50 行、64 KB，单行超过 4 KB 截断）。")}</li>
          <li>{text("Kept 30 days; over 50 MB the oldest go first; clearing the history store removes them too.", "保留 30 天；超过 50 MB 先删最旧的；清空历史库时一并删除。")}</li>
          <li>{text("StarRocks keeps error logs only 48 hours; later they cannot be read.", "StarRocks 只保留错误日志 48 小时，超过后读不到，标“读取前已被清理”。")}</li>
        </ul>
      </section>
      <section aria-labelledby="load-history-reasons" className="history-card load-history-section">
        <h2 id="load-history-reasons">{text("Reasons", "原因汇总")} <span className="history-muted">{text("by table and reason · sampled rows, not all filtered rows", "按表与原因 · 样本行数，不是全部过滤行")}</span></h2>
        {logs.reasons.length === 0 ? <p className="notify-empty">{text("No saved error log in this range.", "这段时间没有保存的错误日志。")}</p> : <div className="history-table-wrap"><table className="history-table load-table">
          <thead><tr><th scope="col">{text("Table", "表")}</th><th scope="col">{text("Reason", "原因")}</th><th scope="col">{text("Column · type", "列 · 类型")}</th><th className="num" scope="col">{text("Sampled rows", "样本行")}</th><th className="num" scope="col">{text("Loads", "导入次数")}</th><th scope="col">{text("First · last", "首次 · 最近")}</th></tr></thead>
          <tbody>{logs.reasons.map((reason) => <tr key={`${reason.database}.${reason.table}:${reason.category}:${reason.column ?? ""}:${reason.target_type ?? ""}`}>
            <td className="mono">{reason.database}.{reason.table}</td>
            <td>{categoryText(reason.category, text)}{reason.new && <span className="load-new-reason">{text("new", "新出现")}</span>}</td>
            <td className="mono">{reasonDetail(reason.column, reason.target_type) || "—"}</td>
            <td className="num">{count(String(reason.sampled_rows))}</td>
            <td className="num">{reason.loads}</td>
            <td className="mono">{utcDay(reason.first_seconds)} · {utcDay(reason.last_seconds)}</td>
          </tr>)}</tbody>
        </table></div>}
      </section>
    </div>
    <section aria-labelledby="load-history-logs" className="history-card load-history-section">
      <h2 id="load-history-logs">{text("Saved error logs", "已保存的错误日志")} <span className="history-muted">{text("newest first · the text shows only after “View”", "按完成时间倒序 · 点“查看”才显示原文")}</span></h2>
      {logs.logs.length === 0 ? <p className="notify-empty">{text("Nothing matches.", "没有符合条件的错误日志。")}</p> : <div className="history-table-wrap"><table className="history-table load-table">
        <thead><tr><th scope="col">{text("Finished (UTC)", "完成时间（UTC）")}</th><th scope="col">Label</th><th scope="col">{text("Table", "表")}</th><th scope="col">{text("Type", "类型")}</th><th className="num" scope="col">{text("Filtered", "过滤行")}</th><th className="num" scope="col">{text("Sampled", "样本行")}</th><th scope="col">{text("Reasons", "原因")}</th><th scope="col">{text("State", "状态")}</th><th scope="col"><span className="load-visually-hidden">{text("Actions", "操作")}</span></th></tr></thead>
        <tbody>{logs.logs.map((log) => {
          const open = log.state === "saved";
          return <tr key={log.job_id}>
            <td className="mono">{clockText(log.finish_seconds).slice(5)}</td>
            <td className="mono"><span className="load-label" title={log.label}>{log.label}</span></td>
            <td className="mono">{log.database}.{log.table}</td>
            <td>{groupText(log.kind, text)}</td>
            <td className="num">{count(log.filtered_rows)}</td>
            <td className="num">{open ? log.sampled : "—"}</td>
            <td>{log.categories.length ? log.categories.map((item) => `${categoryText(item.category, text)} ${item.count}`).join(" · ") : "—"}</td>
            <td><span className={`notify-chip ${logTone(log.state)}`}>{logStateText(log.state, log, text)}</span></td>
            <td className="num">{open && <button className="button secondary small" onClick={() => onOpen(log)} type="button">{text("View", "查看")}</button>}</td>
          </tr>;
        })}</tbody>
      </table></div>}
      {logs.total > page && <div className="load-history-pager">
        <span className="history-muted">{text(`${offset + 1}–${Math.min(offset + page, logs.total)} of ${logs.total}`, `第 ${offset + 1}–${Math.min(offset + page, logs.total)} 份，共 ${logs.total} 份`)}</span>
        <button className="button secondary" disabled={offset === 0} onClick={() => onOffset(Math.max(0, offset - page))} type="button">{text("Previous", "上一页")}</button>
        <button className="button secondary" disabled={offset + page >= logs.total} onClick={() => onOffset(offset + page)} type="button">{text("Next", "下一页")}</button>
      </div>}
    </section>
    <p className="load-history-warning" role="note"><strong>{text("Rows are business data. ", "原始行含业务数据。")}</strong>{text("Error log text is stored in plain text in the local history store: anyone who can sign in to SR Doctor sees every database's rows (StarRocks 3.5.14 shows them to unprivileged accounts too), and copies or backups of the file carry them. The text never goes to logs, alert notifications or exports.", "错误原文明文保存在本机历史库：能登录 SR Doctor 的人都能看到所有库的原文（StarRocks 3.5.14 对未授权账号同样开放这些内容）；复制或备份历史库文件时原文随之带走。原文不进日志、告警通知和导出。")}</p>
  </>;
}

export function LoadHistoryPanel({ connectionId, includeSystem, generation, onOpenLog, onOpenAlerts, focus = null, text }: {
  /** Null: not connected; the saved history of `focus.target` is still shown. */
  connectionId: string | null;
  includeSystem: boolean;
  /** Changes on the workspace's refresh tick. */
  generation: string;
  onOpenLog(target: ErrorLogTarget): void;
  /** Task 59: where a load alert asked to land. */
  focus?: { target: string | null; table: string | null; kind: LoadKindGroup | null; jobId: string | null; errors: boolean } | null;
  /** Opens the load alerts of the cluster shown here. */
  onOpenAlerts?(target: string | null): void;
  text: LocalizedText;
}): JSX.Element {
  const [mode, setMode] = useState<Mode>(focus?.errors ? "errors" : "stats");
  const [days, setDays] = useState<7 | 30>(30);
  const [kind, setKind] = useState<LoadKindGroup | null>(focus && !focus.errors ? focus.kind : null);
  const [table, setTable] = useState<string | null>(focus && !focus.errors ? focus.table : null);
  const [jobId, setJobId] = useState<string | null>(focus?.jobId ?? null);
  const [logTable, setLogTable] = useState<string | null>(focus?.errors ? focus.table : null);
  const [category, setCategory] = useState<LoadErrorCategory | null>(null);
  const [offset, setOffset] = useState(0);
  // A load alert names its cluster; otherwise the connected one.
  const shownTarget = focus?.target ?? null;
  const routineMode = mode === "stats" && kind === "routine_load";
  const statsKind = kind === "routine_load" ? null : kind;
  const history = useMonitorRead((signal) => doctorApi.readLoadHistory({ connection_id: connectionId, target: shownTarget, days, kind: statsKind, table, include_system: includeSystem }, signal), `history:${connectionId}:${shownTarget ?? ""}:${days}:${statsKind ?? ""}:${table ?? ""}:${includeSystem}`, generation, "load_history_failed");
  const routine = useMonitorRead(routineMode ? (signal) => doctorApi.readLoadHistoryRoutine({ connection_id: connectionId, target: shownTarget, days, job_id: jobId }, signal) : null, `routine-history:${connectionId}:${shownTarget ?? ""}:${days}:${jobId ?? ""}`, generation, "load_history_failed");
  const errors = useMonitorRead(mode === "errors" ? (signal) => doctorApi.readLoadHistoryErrorLogs({ connection_id: connectionId, target: shownTarget, days, table: logTable, category, offset }, signal) : null, `error-logs:${connectionId}:${shownTarget ?? ""}:${days}:${logTable ?? ""}:${category ?? ""}:${offset}`, generation, "load_history_failed");
  const current = mode === "errors" ? errors : routineMode ? routine : history;
  const data = history.data;
  // Task 59: active load alerts of this target, with a way to them.
  const alertTarget = data?.target ?? null;
  const loadAlerts = useMonitorRead(alertTarget ? (signal) => doctorApi.readLoadAlerts(alertTarget, signal) : null, `load-alerts:${alertTarget ?? ""}`, generation, "load_alerts_failed");
  const activeAlerts = loadAlerts.data ? Number(loadAlerts.data.counts.open) + Number(loadAlerts.data.counts.acknowledged) : 0;
  const tasks = data?.kinds.find((item) => item.kind === "routine_load");
  const options: KindOption[] = data ? [
    { kind: null, label: text("All", "全部"), count: count(data.all_loads) },
    ...data.kinds.filter((item) => item.kind !== "other" && item.kind !== "routine_load").map((item) => ({ kind: item.kind, label: groupText(item.kind, text), count: count(item.loads) })),
    ...(tasks || data.routine_jobs > 0 ? [{ kind: "routine_load" as const, label: "Routine Load", count: text(`${data.routine_jobs} jobs`, `${data.routine_jobs} 作业`) }] : []),
  ] : [];
  const openLogs = (name: string) => { setLogTable(name); setCategory(null); setOffset(0); setMode("errors"); };
  const openLog = (log: StoredLoadErrorLog) => onOpenLog({ connectionId, jobId: log.job_id, title: log.label, meta: text(`${groupText(log.kind, text)} · ${log.database}.${log.table} · job_id ${log.job_id} · finished ${clockText(log.finish_seconds).slice(5)} UTC · saved copy`, `${groupText(log.kind, text)} · ${log.database}.${log.table} · job_id ${log.job_id} · 完成于 ${clockText(log.finish_seconds).slice(5)} UTC · 本机保存的副本`), stored: true, target: shownTarget ?? data?.target ?? null });
  const body = mode === "errors" ? (errors.data ? <ErrorLogsView category={category} logs={errors.data} offset={offset} onCategory={(value) => { setCategory(value); setOffset(0); }} onOffset={setOffset} onOpen={openLog} onTable={(value) => { setLogTable(value); setOffset(0); }} table={logTable} text={text} /> : null)
    : routineMode ? (routine.data ? <RoutineView onJob={setJobId} routine={routine.data} text={text} /> : null)
      : data ? <StatsView history={data} kind={statsKind} onOpenLogs={openLogs} text={text} /> : null;
  return <div className="load-history">
    <div className="load-history-filters">
      <Segment<Mode> label={text("History content", "历史内容")} onChange={setMode} options={[
        { value: "stats", label: text("Statistics", "统计") },
        { value: "errors", label: <>{text("Error logs", "错误日志")}{data && <span className="mono history-muted"> {data.error_logs.saved}</span>}</> },
      ]} value={mode} />
      <Segment<7 | 30> label={text("Range", "时间范围")} onChange={setDays} options={[7, 30].map((value) => ({ value: value as 7 | 30, label: text(`${value} days`, `${value} 天`) }))} value={days} />
      {mode === "stats" && !routineMode && data && <label className="notify-filter">{text("Table", "表")}
        <select onChange={(event) => setTable(event.target.value || null)} value={table ?? ""}>
          <option value="">{text(`All tables (${data.table_options.length})`, `全部表（${data.table_options.length}）`)}</option>
          {data.table_options.map((name) => <option key={name} value={name}>{name}</option>)}
        </select>
      </label>}
      <span className="load-history-refresh"><RefreshBar error={current.error} loading={current.loading} onRefresh={current.refresh} text={text} updatedSeconds={current.updatedSeconds} /></span>
    </div>
    {connectionId ? <CollectionStrip connectionId={connectionId} history={data} text={text} />
      : <p className="history-banner neutral" role="status">{text(`Not connected: showing the saved history of ${data?.target ?? shownTarget ?? "the most recent cluster"}.`, `未连接：显示本机保存的 ${data?.target ?? shownTarget ?? "最近采集的集群"} 的历史。`)}</p>}
    {activeAlerts > 0 && <p className="load-history-alerts" role="status">
      <span>{text(`${activeAlerts} load alert${activeAlerts === 1 ? "" : "s"} to handle (new or acknowledged).`, `有 ${activeAlerts} 条待处理的导入告警（新告警或已确认）。`)}</span>
      {loadAlerts.data && !loadAlerts.data.thresholds.calibrated && <span className="planned load-alert-trial">{text("Rules on trial", "规则试运行")}</span>}
      {onOpenAlerts && <button className="button secondary small" onClick={() => onOpenAlerts(alertTarget)} type="button">{text("View load alerts", "查看导入告警")}</button>}
    </p>}
    {mode === "stats" && (options.length > 1 || kind !== null) && <KindSwitch kind={kind} onKind={(value) => { setKind(value); setJobId(null); }} options={options} text={text} />}
    {body ?? (current.error ? <MonitorReadError error={current.error} onRetry={current.refresh} text={text} title={loadFailureText(current.error, text)} />
      : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>)}
    {mode === "stats" && <ul className="notify-notes boxed">
      <li>{text("Only finished loads (FINISHED / CANCELLED) are counted, by finish time to the second; running loads are on the Overview tab.", "只统计已结束的导入（FINISHED / CANCELLED），时间只到秒，按完成时间归入小时；进行中的导入在“概览”里看。")}</li>
      <li>{text("Counts are finished loads; a Routine Load job runs many tasks, and each finished task counts once. The Routine Load switch shows how many jobs had a snapshot.", "次数按已结束的导入计；一个 Routine Load 作业会运行很多个任务，每个结束的任务计一次。类型切换里的 Routine Load 显示有快照的作业数。")}</li>
      <li>{text("StarRocks keeps about 1000 records per kind for 3 days; when records were cleared before they were read, or a read hit its limit, the hour is marked partial. Nothing is interpolated.", "StarRocks 在 FE 内存里只保留最近 3 天、每类约 1000 条；采集时发现记录已被提前清理，或读到上限，就把这一小时标为“部分”，不插值。")}</li>
      <li>{text("A StarRocks version or history rule change starts a new segment that is not compared across. Only locally connected clusters are collected; remote clusters wait for the production read-only pilot (G1).", "StarRocks 版本或统计规则变化前后单独成段，不跨段比较。只采集本机连接的集群；远端集群要等生产只读试点（G1）通过。")}</li>
      <li>{text("Load alerts (Routine Load paused or ended, lag growing, failure or filter rate jumping against the baseline) are under Alerts › Loads.", "导入告警（Routine Load 暂停或结束、积压持续增长、失败率或过滤率相对基线突增）在“告警 › 导入”。")}</li>
    </ul>}
  </div>;
}
