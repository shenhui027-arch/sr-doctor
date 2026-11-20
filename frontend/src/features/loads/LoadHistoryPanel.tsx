// Design board 07-loads: 30-day load history, Routine Load job snapshots, and
// saved error logs from the local SQLite history store.
import { useId, useState } from "react";
import type { JSX } from "react";
import type {
  HistoryLoadCoverage,
  LoadErrorCategory,
  LoadHistoryDay,
  LoadHistoryErrorLogsResponse,
  LoadHistoryResponse,
  LoadHistoryRoutineResponse,
  LoadKindGroup,
  RoutineHistoryState,
  StoredLoadErrorLog,
  StoredLoadErrorLogState,
} from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Button, Card, InfoTip, PageGrid, SegTabs, StateStamp } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { clockText, collectionLabel, useHistoryStatus } from "../history/history-status";
import { useMonitorRead } from "../history/monitor-read";
import { MonitorReadError, RefreshBar } from "../history/MonitorStates";
import type { ErrorLogTarget } from "./LoadErrorLogDialog";
import { KpiCards, type OverviewKpiSpec } from "./LoadOverviewPanel";
import {
  bytes,
  categoryText,
  compact,
  count,
  groupText,
  loadFailureText,
  shareText,
  type Tone,
} from "./load-text";
import "./load-history.css";

type Mode = "stats" | "errors";
const KIND_CLASS: Record<LoadKindGroup, string> = {
  stream_load: "stream",
  insert: "insert",
  routine_load: "routine",
  broker: "broker",
  other: "other",
};
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

function DateTicks({ starts }: { starts: number[] }): JSX.Element {
  const tickCount = Math.min([5, 4, 3].find((n) => (starts.length - 1) % (n - 1) === 0) ?? 5, starts.length);
  const picks = tickCount <= 1
    ? starts.slice(0, 1)
    : Array.from({ length: tickCount }, (_, index) => starts[Math.round((index * (starts.length - 1)) / (tickCount - 1))]);
  return (
    <div aria-hidden="true" className="load-hour-labels">
      {picks.map((start, index) => (
        <span key={`${start}:${index}`}>{utcDay(start)}</span>
      ))}
    </div>
  );
}

const COVERAGE_LABELS: Record<HistoryLoadCoverage, readonly [string, string]> = {
  complete: ["complete", "完整"],
  partial: ["partial", "部分"],
  not_collected: ["not collected", "未采集"],
};

export function coverageText(coverage: HistoryLoadCoverage, text: LocalizedText): string {
  return lookupEnumLabel(COVERAGE_LABELS, coverage, text);
}

const PARTIAL_REASON_LABELS: Record<string, readonly [string, string]> = {
  row_limit: ["an hour hit the 2000-row read limit", "有时段读到 2000 条上限"],
  stream_records_cleared: [
    "StarRocks cleared finished Stream Loads / Routine Load tasks before they were read",
    "读取前 StarRocks 已清掉已结束的 Stream Load / Routine Load 任务",
  ],
  load_records_cleared: [
    "StarRocks cleared INSERT / Broker Load records before they were read",
    "读取前 StarRocks 已清掉 INSERT / Broker Load 记录",
  ],
  first_look_unproven: [
    "first collection: StarRocks held nothing older, so completeness cannot be shown",
    "首次采集时 StarRocks 中没有更早的记录，无法确认这段是否完整",
  ],
};

function partialReasonText(reason: string, text: LocalizedText): string {
  return lookupEnumLabel(PARTIAL_REASON_LABELS, reason, text);
}

export function logStateText(state: StoredLoadErrorLogState, log: StoredLoadErrorLog, text: LocalizedText): string {
  switch (state) {
    case "saved":
      return log.truncated ? text("Saved · cut at the limit", "已保存 · 已截断") : text("Saved", "已保存");
    case "pending":
      return log.attempts > 0
        ? text(`Read failed · retrying (attempt ${log.attempts + 1})`, `读取失败 · 下轮重试（第 ${log.attempts + 1} 次）`)
        : text("Waiting to be read", "等待读取");
    case "purged":
      return text("Cleared by StarRocks before it was read", "读取前已被清理");
    case "expired":
      return text("Not read within StarRocks' 48 hours", "未能在 StarRocks 保留期内读取");
    case "failed":
      return text("Reads kept failing", "多次读取失败");
    case "no_log":
      return text("StarRocks has no error log", "StarRocks 没有错误日志");
    case "trimmed":
      return text("Text dropped under the 50 MB limit", "原文已按 50 MB 上限删除");
    default:
      return lookupEnumLabel({}, state, text);
  }
}

export function logTone(state: StoredLoadErrorLogState): Tone {
  switch (state) {
    case "saved":
      return "ok";
    case "pending":
      return "warn";
    case "trimmed":
      return "info";
    case "purged":
    case "expired":
    case "failed":
    case "no_log":
    default:
      return "off";
  }
}

const ROUTINE_HISTORY_LABELS: Record<RoutineHistoryState, readonly [string, string]> = {
  running: ["Running", "运行中"],
  scheduling: ["Waiting to schedule", "等待调度"],
  paused: ["Paused", "已暂停"],
  stopped: ["Stopped", "已停止"],
  cancelled: ["Cancelled", "已取消"],
  absent: ["Not present", "不存在"],
  not_collected: ["Not collected", "未采集"],
  unknown: ["Unknown", "未知"],
};

export function routineStateText(state: RoutineHistoryState, text: LocalizedText): string {
  return lookupEnumLabel(ROUTINE_HISTORY_LABELS, state, text);
}

function reasonDetail(column: string | null, targetType: string | null): string {
  return [column, targetType].filter(Boolean).join(" · ");
}

function CollectionStrip({
  connectionId,
  history,
  text,
}: {
  connectionId: string;
  history: LoadHistoryResponse | null;
  text: LocalizedText;
}): JSX.Element {
  const { status, refresh } = useHistoryStatus(connectionId);
  const [busy, setBusy] = useState(false);
  const state = status?.collection.state;
  const toggle = (action: "pause" | "resume") => {
    setBusy(true);
    doctorApi.setHistoryCollection({ connection_id: connectionId, action }).finally(() => {
      setBusy(false);
      refresh();
    });
  };
  const collection = history?.collection;
  if (status?.target && history?.target && status.target !== history.target) {
    return (
      <div className="notice-banner" role="status">
        <span>
          {text(
            `Showing the saved history of ${history.target}; the collection status belongs to the connected cluster ${status.target}.`,
            `显示本机保存的 ${history.target} 的历史；当前连接的是 ${status.target}，采集状态与暂停开关不适用于这里。`,
          )}
        </span>
      </div>
    );
  }
  const dotClass = state === "running"
    ? "status-dot status-dot--ok"
    : state === "paused"
      ? "status-dot status-dot--warn"
      : "status-dot status-dot--hollow";
  return (
    <div className="load-history-strip">
      <div className="load-history-strip-main">
        {status ? (
          <span className="status-inline">
            <span aria-hidden="true" className={dotClass} />
            <span>{collectionLabel(status, text)}</span>
          </span>
        ) : null}
        <span>
          {text("Read up to", "已采到")} <span className="mono">{clockText(collection?.watermark_seconds)}</span> UTC
        </span>
        {collection?.last_rows_read !== null && collection?.last_rows_read !== undefined ? (
          <span>{text(`last read ${collection.last_rows_read} loads`, `最近一轮读取 ${collection.last_rows_read} 条导入`)}</span>
        ) : null}
        <InfoTip label={text("Collection scope", "采集说明")}>
          {text(
            "Local SQLite history store kept 30 days. Shares the audit collector (every 5 minutes); finished loads are bucketed by UTC finish hour.",
            "本机 SQLite 历史库保留 30 天；与审计历史共用同一有界采集任务（每 5 分钟），已结束导入按完成时间归入 UTC 日。",
          )}
        </InfoTip>
        <div className="load-history-strip-actions">
          {(state === "running" || state === "backoff") ? (
            <Button disabled={busy} onClick={() => toggle("pause")} size="sm" variant="secondary">
              {text("Pause collection", "暂停采集")}
            </Button>
          ) : null}
          {state === "paused" ? (
            <Button disabled={busy} onClick={() => toggle("resume")} size="sm" variant="secondary">
              {text("Resume collection", "继续采集")}
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function HistoryKpis({
  history,
  kind,
  text,
}: {
  history: LoadHistoryResponse;
  kind: LoadKindGroup | null;
  text: LocalizedText;
}): JSX.Element {
  const totals = history.totals;
  const failedByKind = kind === null
    ? history.kinds
        .filter((item) => item.failed !== "0")
        .map((item) => `${groupText(item.kind, text)} ${count(item.failed)}`)
        .join(" · ")
    : "";
  const storage = history.error_logs;
  const cards: OverviewKpiSpec[] = [
    {
      key: "finished",
      label: text("Finished loads", "已结束的导入"),
      value: count(totals.loads),
      unit: text("loads", "次"),
      note:
        history.segments.length > 1
          ? text(`${history.segments.length} version segments, not compared`, `跨 ${history.segments.length} 个版本段，不跨段比较`)
          : text("by finish time", "按完成时间"),
      caveat:
        totals.not_collected_hours > 0
          ? text(`${totals.not_collected_hours} hours not collected, not counted`, `${totals.not_collected_hours} 小时未采集，未计入`)
          : null,
    },
    {
      key: "failed",
      label: text("Failed / cancelled", "失败 / 取消"),
      value: count(totals.failed),
      unit: ppmText(totals.failed_ppm),
      note: failedByKind || (totals.failed === "0" ? text("none", "没有失败") : text("of finished loads", "占已结束导入")),
      bad: totals.failed !== "0",
    },
    {
      key: "written",
      label: text("Rows written", "写入行"),
      value: compact(totals.loaded_rows),
      unit: text("rows", "行"),
      note:
        totals.partial_hours > 0
          ? text(`${totals.partial_hours} partly read hours may be low`, `${totals.partial_hours} 个部分采集的小时可能少计`)
          : text("from SINK_ROWS", "来自 SINK_ROWS"),
      caveat:
        totals.loaded_unknown > 0
          ? text(`${totals.loaded_unknown} loads reported no row count, left out`, `${totals.loaded_unknown} 次没有报告行数，未计入`)
          : null,
    },
    {
      key: "filtered",
      label: text("Filtered rows", "过滤行"),
      value: count(totals.filtered_rows),
      unit: ppmText(totals.filtered_ppm),
      note: text(`of input rows · ${totals.tables_with_filtered} tables filtered`, `占输入行 · ${totals.tables_with_filtered} 张表有过滤`),
      caveat:
        totals.filtered_unknown > 0
          ? text(`${totals.filtered_unknown} loads reported no filtered count, left out`, `${totals.filtered_unknown} 次没有报告过滤行，未计入`)
          : null,
    },
    {
      key: "saved_logs",
      label: text("Saved error logs", "已保存的错误日志"),
      value: count(String(storage.saved)),
      unit: text("logs", "份"),
      note: text(
        `${bytes(storage.raw_bytes)} / 50 MB · ${storage.missed} cleared before reading`,
        `${bytes(storage.raw_bytes)} / 50 MB · ${storage.missed} 份读取前已被清理`,
      ),
    },
  ];
  const gaps = totals.not_collected_hours > 0 || totals.partial_hours > 0;
  return (
    <>
      {gaps ? (
        <div className="notice-banner load-history-scope" role="note">
          <span>
            {text(
              `Collected time only: ${totals.not_collected_hours} hours not collected and ${totals.partial_hours} partly read in this range; nothing is filled in.`,
              `只统计已采集的时段：这段时间有 ${totals.not_collected_hours} 小时未采集、${totals.partial_hours} 小时为部分采集，不补数据。`,
            )}
          </span>
        </div>
      ) : null}
      <KpiCards cards={cards} label={text("Load history totals", "导入历史汇总")} />
    </>
  );
}

function DayDetail({ day, history, text }: { day: LoadHistoryDay; history: LoadHistoryResponse; text: LocalizedText }): JSX.Element {
  const segment = history.segments[day.segment];
  return (
    <div className="load-hour-detail" role="status">
      <strong>
        {utcDay(day.start_seconds)}（{coverageText(day.coverage, text)}）
      </strong>
      {day.loads === null ? (
        <span>
          {text(
            "Not collected: SR Doctor was not connected, or collection was paused. Nothing is filled in.",
            "未采集：当时没有连接或已暂停采集，不补数据。",
          )}
        </span>
      ) : (
        <>
          {day.by_kind.map((kind) => (
            <span key={kind.kind}>
              {groupText(kind.kind, text)} <span className="mono">{count(kind.loads)}</span>
            </span>
          ))}
          <span className={day.failed !== "0" ? "load-num-critical" : undefined}>
            {text("failed", "失败")} <span className="mono">{count(day.failed)}</span>
          </span>
          <span>
            {text("filtered rows", "过滤行")} <span className="mono">{count(day.filtered_rows)}</span>
          </span>
          {day.unknown_counts > 0 ? (
            <span className="load-partial-note">
              {text(`${day.unknown_counts} counts not reported, left out`, `${day.unknown_counts} 个计数没有报告，未计入`)}
            </span>
          ) : null}
        </>
      )}
      {day.partial_reasons.map((reason) => (
        <span className="load-partial-note" key={reason}>
          {partialReasonText(reason, text)}
        </span>
      ))}
      {day.not_collected_hours > 0 && day.loads !== null ? (
        <span className="load-partial-note">
          {text(`${day.not_collected_hours} hours not collected`, `${day.not_collected_hours} 小时未采集`)}
        </span>
      ) : null}
      {segment ? <span className="load-muted">StarRocks {segment.engine_versions.join(", ")}</span> : null}
    </div>
  );
}

function DailyChart({ history, text }: { history: LoadHistoryResponse; text: LocalizedText }): JSX.Element {
  const days = history.days;
  const [picked, setPicked] = useState<number | null>(null);
  const latest = (() => {
    for (let index = days.length - 1; index >= 0; index -= 1) {
      if (days[index].loads !== null) return index;
    }
    return days.length - 1;
  })();
  const selected = days[picked ?? latest];
  const boundaries = days
    .map((day, index) => ({ day, index }))
    .filter(({ day, index }) => index > 0 && day.segment !== days[index - 1].segment && day.loads !== null);
  const axis = [...history.count_axis].reverse();

  return (
    <div aria-labelledby="load-history-daily" className="load-history-block">
      <div className="load-history-block-head">
        <strong id="load-history-daily">
          {text("Loads per day by type", "每天导入次数 · 按类型")}
        </strong>
        <span className="load-legend">
          {(["stream_load", "insert", "routine_load", "broker"] as LoadKindGroup[]).map((kind) => (
            <span key={kind}>
              <i className={`kind-${KIND_CLASS[kind]}`} />
              {groupText(kind, text)}
            </span>
          ))}
          <span>
            <i className="fail-line" />
            {text("days with failures", "有失败的天")}
          </span>
        </span>
      </div>
      <div className="load-history-chart">
        <div aria-hidden="true" className="load-axis tall">
          {axis.map((tick) => (
            <span key={tick}>{compact(tick)}</span>
          ))}
        </div>
        <div className="load-history-plot">
          {days.map((day, index) => (
            <button
              aria-label={
                day.loads === null
                  ? text(`${utcDay(day.start_seconds)} not collected`, `${utcDay(day.start_seconds)} 未采集`)
                  : text(
                      `${utcDay(day.start_seconds)}: ${count(day.loads)} loads, ${count(day.failed)} failed, ${coverageText(day.coverage, text)}`,
                      `${utcDay(day.start_seconds)}：${count(day.loads)} 次导入，失败 ${count(day.failed)}，${coverageText(day.coverage, text)}`,
                    )
              }
              aria-pressed={(picked ?? latest) === index}
              className={`load-history-day${day.loads === null ? " gone" : ""}`}
              key={day.start_seconds}
              onClick={() => setPicked(index)}
              type="button"
            >
              <span className={day.failed !== null && day.failed !== "0" ? "fail-mark on" : "fail-mark"} />
              {STACK.map((kind) => {
                const part = day.by_kind.find((item) => item.kind === kind);
                return part ? (
                  <span
                    className={`stack kind-${KIND_CLASS[kind]}${day.coverage === "partial" ? " partial" : ""}`}
                    key={kind}
                    style={{ height: `${part.height_permyriad / 100}%` }}
                  />
                ) : null;
              })}
            </button>
          ))}
          {boundaries.map(({ day, index }) => (
            <span
              className="load-history-boundary"
              key={day.start_seconds}
              style={{ left: `${(index / days.length) * 100}%` }}
            >
              {text(
                `from ${utcDay(day.start_seconds)}: StarRocks ${history.segments[day.segment]?.engine_versions.join(", ")}; not compared across`,
                `${utcDay(day.start_seconds)} 起 StarRocks ${history.segments[day.segment]?.engine_versions.join(", ")} · 不跨段比较`,
              )}
            </span>
          ))}
        </div>
        <span className="load-muted load-history-cov-label">{text("Coverage", "覆盖")}</span>
        <div className="load-history-coverage">
          {days.map((day) => (
            <span
              className={`cov-${day.coverage}`}
              key={day.start_seconds}
              title={`${utcDay(day.start_seconds)} ${coverageText(day.coverage, text)}`}
            />
          ))}
        </div>
        <span />
        <DateTicks starts={days.map((day) => day.start_seconds)} />
      </div>
      <span className="load-legend">
        <span><i className="cov-complete" />{text("complete", "完整")}</span>
        <span><i className="cov-partial" />{text("partial (records cleared first or read limit hit)", "部分（有小时被提前清理或读到上限）")}</span>
        <span><i className="cov-not_collected" />{text("not collected; nothing is filled in", "未采集（未连接或已暂停），不补数据")}</span>
      </span>
      {selected ? <DayDetail day={selected} history={history} text={text} /> : null}
    </div>
  );
}

type Run = { points: string; dot: boolean };

/** Polyline runs between uncollected points (and across version segments), never bridged. */
function runs(values: Array<number | null>, width: number, segments?: number[]): Run[] {
  const out: Run[] = [];
  let current: string[] = [];
  const flush = () => {
    if (current.length) {
      out.push({
        points: (current.length === 1 ? [current[0], current[0]] : current).join(" "),
        dot: current.length === 1,
      });
    }
    current = [];
  };
  values.forEach((value, index) => {
    if (segments && index > 0 && segments[index] !== segments[index - 1]) flush();
    if (value === null) {
      flush();
      return;
    }
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

  return (
    <div aria-labelledby="load-history-rates" className="load-history-block load-history-rates">
      <div className="load-history-block-head">
        <strong id="load-history-rates">{text("Failure and filter rates per day", "失败率与过滤率 · 按天")}</strong>
        <span className="load-legend">
          <span><i className="line-failed" />{text("failure rate", "失败率")}</span>
          <span><i className="line-filtered" />{text("filter rate", "过滤率")}</span>
        </span>
      </div>
      <div className="load-history-chart small">
        <div aria-hidden="true" className="load-axis short">
          {[...history.rate_axis].reverse().map((tick) => (
            <span key={tick}>{ppmText(tick)}</span>
          ))}
        </div>
        <svg
          aria-label={
            last
              ? text(
                  `Failure and filter rates per day; latest ${ppmText(last.failed_rate_ppm)} failed, ${ppmText(last.filtered_rate_ppm)} filtered`,
                  `每天的失败率与过滤率；最近一天失败 ${ppmText(last.failed_rate_ppm)}、过滤 ${ppmText(last.filtered_rate_ppm)}`,
                )
              : text("No collected days", "没有采集到的天")
          }
          className="load-history-lines"
          preserveAspectRatio="none"
          role="img"
          viewBox={`0 0 ${days.length * width} 100`}
        >
          {failed.map((run) => (
            <polyline className={run.dot ? "rate-failed dot" : "rate-failed"} key={`f${run.points}`} points={run.points} vectorEffect="non-scaling-stroke" />
          ))}
          {filtered.map((run) => (
            <polyline className={run.dot ? "rate-filtered dot" : "rate-filtered"} key={`r${run.points}`} points={run.points} vectorEffect="non-scaling-stroke" />
          ))}
        </svg>
        <span />
        <DateTicks starts={days.map((day) => day.start_seconds)} />
      </div>
      <span className="load-muted">
        {history.segments.length > 1
          ? text("Uncollected days and StarRocks version changes break the line", "未采集的天与 StarRocks 版本变化处断开，不连线")
          : text("Uncollected days break the line", "未采集的天断开，不连线")}
      </span>
    </div>
  );
}

function TableList({
  history,
  onOpenLogs,
  text,
}: {
  history: LoadHistoryResponse;
  onOpenLogs(table: string): void;
  text: LocalizedText;
}): JSX.Element {
  return (
    <Card
      footer={
        <span>
          {text(
            "Exact counts from StarRocks; reason shares come from saved error log samples (~50 rows/load).",
            "过滤行与失败次数为 StarRocks 精确计数；原因占比仅来自保存的错误日志样本（每次最多约 50 行）。",
          )}
        </span>
      }
      span={{ xl: 6, lg: 6, md: 8, sm: 4 }}
      title={text("By table", "按表")}
      titleExtra={
        <InfoTip label={text("About table filter reasons", "按表过滤原因说明")}>
          {text(
            "Sorted by filtered rows; top reason comes from saved error log samples, not every filtered row.",
            "按过滤行排序；主要过滤原因来自已保存的错误日志样本，不代表全部过滤行。",
          )}
        </InfoTip>
      }
      variant="table"
    >
      {history.tables.length === 0 ? (
        <div className="state-box">
          <p>{text("No finished loads collected in this range.", "这段时间没有采集到已结束的导入。")}</p>
        </div>
      ) : (
        <table className="data-table load-table">
          <thead>
            <tr>
              <th scope="col">{text("Table", "表")}</th>
              <th scope="col">{text("Type", "类型")}</th>
              <th className="num" scope="col">{text("Loads", "次数")}</th>
              <th className="num" scope="col">{text("Failed", "失败")}</th>
              <th className="num" scope="col">{text("Rows written", "写入行")}</th>
              <th className="num" scope="col">{text("Filtered", "过滤行")}</th>
              <th scope="col">{text("Top filter reason (sample)", "主要过滤原因（样本）")}</th>
              <th scope="col"><span className="sr-only">{text("Actions", "操作")}</span></th>
            </tr>
          </thead>
          <tbody>
            {history.tables.map((row) => {
              const name = `${row.database}.${row.table}`;
              const reason = row.top_reason;
              return (
                <tr key={name}>
                  <th className="mono" scope="row">{name}</th>
                  <td>{row.kinds.map((kind) => groupText(kind, text)).join(" · ")}</td>
                  <td className="num">{count(row.loads)}</td>
                  <td className={`num${row.failed !== "0" ? " load-num-critical" : ""}`}>{count(row.failed)}</td>
                  <td className="num">{compact(row.loaded_rows)}</td>
                  <td className="num">{count(row.filtered_rows)}</td>
                  <td>
                    {reason ? (
                      <>
                        <span>
                          {categoryText(reason.category, text)}
                          {reasonDetail(reason.column, reason.target_type) ? (
                            <span className="mono"> {reasonDetail(reason.column, reason.target_type)}</span>
                          ) : null}{" "}
                          · {text(`${shareText(reason.sample_share_permyriad)} of the sample`, `样本内占比 ${shareText(reason.sample_share_permyriad)}`)}
                        </span>
                        {reason.new_since_seconds !== null ? (
                          <span className="badge-stamp load-new-reason">
                            {text(`new · ${utcDay(reason.new_since_seconds)}`, `新出现 · ${utcDay(reason.new_since_seconds)}`)}
                          </span>
                        ) : null}
                      </>
                    ) : (
                      <span className="load-muted">
                        {row.filtered_rows === "0" ? text("no filtered rows", "没有过滤行") : text("no saved error log", "没有保存的错误日志")}
                      </span>
                    )}
                  </td>
                  <td className="num">
                    {row.saved_logs > 0 ? (
                      <Button onClick={() => onOpenLogs(name)} size="sm" variant="secondary">
                        {text(`Error logs ${row.saved_logs}`, `错误日志 ${row.saved_logs}`)}
                      </Button>
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </Card>
  );
}

export function StatsView({
  history,
  kind = null,
  onOpenLogs,
  text,
}: {
  history: LoadHistoryResponse;
  kind?: LoadKindGroup | null;
  onOpenLogs(table: string): void;
  text: LocalizedText;
}): JSX.Element {
  return (
    <div className="load-view-stack">
      <HistoryKpis history={history} kind={kind} text={text} />
      <PageGrid>
        <Card
          span={{ xl: 6, lg: 6, md: 8, sm: 4 }}
          title={text("Daily loads & rates (UTC)", "按天导入与失败/过滤率（UTC）")}
          titleExtra={
            <InfoTip label={text("How history is counted", "历史统计口径")}>
              {text(
                "Finished loads only, bucketed by UTC finish day. Uncollected hours and StarRocks version changes break the line and are never interpolated.",
                "仅统计已结束导入（按 UTC 完成日）；未采集时段与 StarRocks 版本变化单独成段，折线断开、绝不插值。",
              )}
            </InfoTip>
          }
          variant="chart"
        >
          <div className="load-history-charts-stack">
            <DailyChart history={history} key={`${history.days.length}:${history.days[0]?.start_seconds ?? 0}`} text={text} />
            <RatesChart history={history} text={text} />
          </div>
        </Card>
        <TableList history={history} onOpenLogs={onOpenLogs} text={text} />
      </PageGrid>
    </div>
  );
}

export function RoutineView({
  routine,
  onJob,
  text,
}: {
  routine: LoadHistoryRoutineResponse;
  onJob(jobId: string): void;
  text: LocalizedText;
}): JSX.Element {
  const detail = routine.selected;
  if (routine.jobs.length === 0) {
    return (
      <Card span={{ xl: 12, lg: 12, md: 8, sm: 4 }} title="Routine Load" variant="detail">
        <div className="state-box">
          <p>{text("No Routine Load job snapshots in this range. Jobs are snapshotted on every collection cycle from StarRocks 3.2.", "这段时间没有 Routine Load 作业快照。StarRocks 3.2 起，每轮采集都会给作业拍一次快照。")}</p>
        </div>
      </Card>
    );
  }
  const pausedRuns: Array<[number, number]> = [];
  const gapRuns: Array<[number, number]> = [];
  detail?.lag.forEach((point, index) => {
    const push = (list: Array<[number, number]>) => {
      const last = list[list.length - 1];
      if (last && last[1] === index) last[1] = index + 1;
      else list.push([index, index + 1]);
    };
    if (point.paused) push(pausedRuns);
    if (!point.collected) push(gapRuns);
  });
  const lagLines = detail ? runs(detail.lag.map((point) => point.y_permyriad), 1) : [];

  return (
    <div className="load-view-stack">
      <PageGrid>
        <Card
          actions={
            <span className="load-legend">
              <span><i className="state-running" />{text("running", "运行中")}</span>
              <span><i className="state-paused" />{text("paused", "已暂停")}</span>
              <span><i className="state-cancelled" />{text("cancelled / stopped", "已取消 / 已停止")}</span>
              <span><i className="state-not_collected" />{text("not collected", "未采集")}</span>
            </span>
          }
          footer={
            routine.jobs.some((job) => job.unknown_intervals > 0) ? (
              <span>
                {text(
                  "Intervals where a counter went down (job recreated) or snapshots were far apart are unknown and left out of the sums.",
                  "累计值变小（作业重建）或两次快照相隔太久的区间记为未知，不计入增量。",
                )}
              </span>
            ) : undefined
          }
          span={{ xl: detail ? 7 : 12, lg: detail ? 7 : 12, md: 8, sm: 4 }}
          title={text("Routine Load job history", "Routine Load 作业快照")}
          titleExtra={
            <InfoTip label={text("About Routine Load snapshots", "作业快照口径")}>
              {routine.cell_seconds >= 3600 * 6
                ? text("Each cell is 6 hours; increase = difference of adjacent snapshots' lifetime counters.", "状态条每格 6 小时；增量 = 相邻两次快照的累计值之差。")
                : text("Each cell is 1 hour; increase = difference of adjacent snapshots' lifetime counters.", "状态条每格 1 小时；增量 = 相邻两次快照的累计值之差。")}
            </InfoTip>
          }
          variant="table"
        >
          <table className="data-table load-table">
            <thead>
              <tr>
                <th scope="col">{text("Job", "作业")}</th>
                <th className="load-history-cells-col" scope="col">{text("State", "状态")}</th>
                <th className="num" scope="col">{text("Pauses seen", "观察到的暂停")}</th>
                <th className="num" scope="col">{text("Rows loaded (seen)", "导入行（已观测）")}</th>
                <th className="num" scope="col">{text("Error rows (seen)", "错误行（已观测）")}</th>
                <th className="num" scope="col">{text("Committed / aborted (seen)", "提交 / 中止任务（已观测）")}</th>
                <th className="num" scope="col">{text("Max lag seen", "观察到的最大积压")}</th>
              </tr>
            </thead>
            <tbody>
              {routine.jobs.map((job) => {
                const selected = detail?.job_id === job.job_id;
                return (
                  <tr className={selected ? "is-selected" : undefined} key={job.job_id}>
                    <th scope="row">
                      <button aria-pressed={selected} className="row-pick-btn load-link" onClick={() => onJob(job.job_id)} type="button">
                        <strong className="mono">{job.name}</strong>
                        <span className="load-sub load-muted mono">{job.database}.{job.table}</span>
                      </button>
                    </th>
                    <td>
                      <span
                        aria-label={text(`${job.name}: now ${job.current_state}, paused ${job.pauses} times`, `${job.name}：当前 ${job.current_state}，暂停 ${job.pauses} 次`)}
                        className="load-history-cells"
                        role="img"
                      >
                        {job.cells.map((cell) => (
                          <span
                            className={`state-${cell.state}`}
                            key={cell.start_seconds}
                            title={`${clockText(cell.start_seconds).slice(5, 16)} UTC · ${routineStateText(cell.state, text)}`}
                          />
                        ))}
                      </span>
                      <DateTicks starts={job.cells.map((cell) => cell.start_seconds)} />
                    </td>
                    <td className={`num${job.pauses > 0 ? " load-num-critical" : ""}`}>{job.pauses}</td>
                    <td className="num">{job.loaded_rows === null ? <StateStamp detail={text("unknown", "未知")} state="missing" /> : compact(job.loaded_rows)}</td>
                    <td className="num">{job.error_rows === null ? text("unknown", "未知") : count(job.error_rows)}</td>
                    <td className="num">{job.committed_tasks === null ? text("unknown", "未知") : `${count(job.committed_tasks)} / ${count(job.aborted_tasks)}`}</td>
                    <td className="num">{job.max_lag ? count(job.max_lag) : text("unknown", "未知")}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </Card>

        {detail ? (
          <Card
            footer={
              <span>
                {text(
                  "Times are the snapshot that saw the state (UTC), not when it changed; reasons from REASONS_OF_STATE_CHANGED.",
                  "时间是观察到该状态的快照时刻（UTC），不代表真实切换时刻；原因取自 REASONS_OF_STATE_CHANGED。",
                )}
              </span>
            }
            span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
            title={`${detail.name} · ${text("Lag & state history", "分区积压与状态变化")}`}
            variant="detail"
          >
            <div className="load-history-charts-stack">
              <div className="load-history-block">
                <strong>{text("Largest partition lag per hour (OFFSET_LAG)", "每小时最大分区积压（OFFSET_LAG）")}</strong>
                {detail.lag_known ? (
                  <div className="load-history-chart">
                    <div aria-hidden="true" className="load-axis">
                      {[...detail.lag_axis].reverse().map((tick) => (
                        <span key={tick}>{compact(tick)}</span>
                      ))}
                    </div>
                    <svg
                      aria-label={text(`${detail.name}: largest partition lag per hour`, `${detail.name} 每小时最大分区积压`)}
                      className="load-history-lines"
                      preserveAspectRatio="none"
                      role="img"
                      viewBox={`0 0 ${detail.lag.length} 100`}
                    >
                      <defs>
                        <pattern height="6" id="lag-gap-hatch" patternTransform="rotate(45)" patternUnits="userSpaceOnUse" width="6">
                          <rect fill="#eceff2" height="6" width="6" />
                          <rect fill="#c9cfd7" height="6" width="2" />
                        </pattern>
                      </defs>
                      {gapRuns.map(([from, to]) => (
                        <rect className="lag-gap" fill="url(#lag-gap-hatch)" height="100" key={`g${from}`} width={to - from} x={from} y="0" />
                      ))}
                      {pausedRuns.map(([from, to]) => (
                        <rect className="lag-paused" height="100" key={`p${from}`} width={to - from} x={from} y="0" />
                      ))}
                      {lagLines.map((run) => (
                        <polyline className={run.dot ? "lag-line dot" : "lag-line"} key={run.points} points={run.points} vectorEffect="non-scaling-stroke" />
                      ))}
                    </svg>
                    <span />
                    <DateTicks starts={detail.lag.map((point) => point.start_seconds)} />
                  </div>
                ) : (
                  <p className="load-muted">{text("StarRocks before 3.3 reports no OFFSET_LAG; lag is unknown.", "3.3 之前的 StarRocks 没有 OFFSET_LAG，积压未知。")}</p>
                )}
              </div>

              <div className="load-history-block">
                <strong>{text("State changes", "状态变化")}</strong>
                {detail.events.length === 0 ? (
                  <p className="load-muted">{text("No state change in this range.", "这段时间没有状态变化。")}</p>
                ) : (
                  <ol className="load-history-events">
                    {detail.events.map((event) => (
                      <li key={event.at_seconds}>
                        <span className="mono load-muted">{clockText(event.at_seconds).slice(5, 16)}</span>
                        <span className="badge-stamp">{event.from_state ? event.to_state : text(`first seen · ${event.to_state}`, `首次观察 · ${event.to_state}`)}</span>
                        {event.reason ? <span className="load-history-event-reason">{event.reason}</span> : null}
                      </li>
                    ))}
                  </ol>
                )}
              </div>

              <div className="load-history-block">
                <strong>{text("Rows and error rows per day", "每天导入行与错误行")}</strong>
                <div className="load-history-chart">
                  <div aria-hidden="true" className="load-axis short">
                    {[...detail.rows_axis].reverse().map((tick) => (
                      <span key={tick}>{compact(tick)}</span>
                    ))}
                  </div>
                  <div className="load-history-plot short">
                    {detail.days.map((day) => (
                      <span
                        aria-label={
                          !day.collected
                            ? text(`${utcDay(day.start_seconds)} not collected`, `${utcDay(day.start_seconds)} 未采集`)
                            : day.loaded_rows === null
                              ? text(`${utcDay(day.start_seconds)} unknown`, `${utcDay(day.start_seconds)} 未知`)
                              : text(
                                  `${utcDay(day.start_seconds)}: ${count(day.loaded_rows)} rows, ${count(day.error_rows)} error rows`,
                                  `${utcDay(day.start_seconds)}：导入行 ${count(day.loaded_rows)}，错误行 ${count(day.error_rows)}`,
                                )
                        }
                        className={`load-history-day static${!day.collected ? " gone" : day.loaded_rows === null ? " unknown" : ""}`}
                        key={day.start_seconds}
                        role="img"
                      >
                        <span className="stack kind-error" style={{ height: `${day.error_height_permyriad / 100}%` }} />
                        <span className="stack kind-routine" style={{ height: `${day.loaded_height_permyriad / 100}%` }} />
                      </span>
                    ))}
                  </div>
                  <span />
                  <DateTicks starts={detail.days.map((day) => day.start_seconds)} />
                </div>
              </div>
            </div>
          </Card>
        ) : null}
      </PageGrid>
    </div>
  );
}

export function ErrorLogsView({
  logs,
  table,
  category,
  onTable,
  onCategory,
  offset,
  onOffset,
  onOpen,
  text,
}: {
  logs: LoadHistoryErrorLogsResponse;
  table: string | null;
  category: LoadErrorCategory | null;
  offset: number;
  onTable(table: string | null): void;
  onCategory(category: LoadErrorCategory | null): void;
  onOffset(offset: number): void;
  onOpen(log: StoredLoadErrorLog): void;
  text: LocalizedText;
}): JSX.Element {
  const tblSelectId = useId();
  const catSelectId = useId();
  const storage = logs.storage;
  const categories: LoadErrorCategory[] = [
    "out_of_range",
    "too_long",
    "column_count",
    "null_in_non_nullable",
    "out_of_partition",
    "json_cast",
    "json_structure",
    "other",
  ];
  const page = 50;

  return (
    <div className="load-view-stack">
      <div className="notice-banner notice-banner--warn" role="note">
        <span>
          <strong>{text("Rows are business data. ", "原始行含业务数据。")}</strong>
          {text(
            "Error log text is stored in plain text in the local history store: anyone who can sign in to SR Doctor sees every database's rows (StarRocks 3.5.14 shows them to unprivileged accounts too), and copies or backups of the file carry them. Kept 30 days / 50 MB cap, deleted when clearing history, and never sent to logs, alert notifications or exports.",
            "错误原文明文保存在本机历史库：能登录 SR Doctor 的人都能看到所有库的原文（StarRocks 3.5.14 对未授权账号同样开放这些内容）；复制或备份历史库文件时原文随之带走。保留 30 天 / 50 MB 上限，清空历史时一并删除，原文不进日志、告警通知和导出。",
          )}
        </span>
      </div>

      <PageGrid>
        <Card
          actions={
            <div className="load-inline-filters">
              <label className="filter-chip-input" htmlFor={tblSelectId}>
                <span>{text("Table", "表")}</span>
                <select id={tblSelectId} onChange={(event) => onTable(event.target.value || null)} value={table ?? ""}>
                  <option value="">{text(`All (${logs.table_options.length})`, `全部（${logs.table_options.length}）`)}</option>
                  {logs.table_options.map((name) => (
                    <option key={name} value={name}>{name}</option>
                  ))}
                </select>
              </label>
              <label className="filter-chip-input" htmlFor={catSelectId}>
                <span>{text("Reason", "原因")}</span>
                <select id={catSelectId} onChange={(event) => onCategory((event.target.value || null) as LoadErrorCategory | null)} value={category ?? ""}>
                  <option value="">{text("All", "全部")}</option>
                  {categories.map((item) => (
                    <option key={item} value={item}>{categoryText(item, text)}</option>
                  ))}
                </select>
              </label>
            </div>
          }
          footer={
            <span>
              <strong className="mono">{bytes(storage.raw_bytes)}</strong>{" "}
              {text(`/ 50 MB text limit · ${storage.saved} logs`, `/ 50 MB 原文上限 · ${storage.saved} 份`)}
              {storage.earliest_saved_seconds !== null
                ? text(` · earliest ${clockText(storage.earliest_saved_seconds).slice(5, 16)} UTC`, ` · 最早 ${clockText(storage.earliest_saved_seconds).slice(5, 16)} UTC`)
                : ""}
            </span>
          }
          span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
          title={text("Sampled reasons by table", "原因汇总（样本）")}
          titleExtra={
            <InfoTip label={text("Saved error log policy", "错误日志采集与保存说明")}>
              {text(
                "Read once in the next cycle after finish (up to 5 per cycle, 50 rows / 64 KB each, 4 KB/row cap). StarRocks keeps tracking logs 48h; older unread logs are marked cleared.",
                "有过滤行的导入结束后下一轮采集读取一次（每轮最多 5 份、每份最多 50 行 / 64 KB、单行 4 KB 截断）。StarRocks 仅保留 48 小时，超期未读标为读取前已被清理。",
              )}
            </InfoTip>
          }
          variant="table"
        >
          {logs.reasons.length === 0 ? (
            <div className="state-box">
              <p>{text("No saved error log in this range.", "这段时间没有保存的错误日志。")}</p>
            </div>
          ) : (
            <table className="data-table load-table">
              <thead>
                <tr>
                  <th scope="col">{text("Table", "表")}</th>
                  <th scope="col">{text("Reason", "原因")}</th>
                  <th scope="col">{text("Column · type", "列 · 类型")}</th>
                  <th className="num" scope="col">{text("Sampled rows", "样本行")}</th>
                  <th className="num" scope="col">{text("Loads", "导入次数")}</th>
                  <th scope="col">{text("First · last", "首次 · 最近")}</th>
                </tr>
              </thead>
              <tbody>
                {logs.reasons.map((reason) => (
                  <tr key={`${reason.database}.${reason.table}:${reason.category}:${reason.column ?? ""}:${reason.target_type ?? ""}`}>
                    <td className="mono">{reason.database}.{reason.table}</td>
                    <td>
                      {categoryText(reason.category, text)}
                      {reason.new ? <span className="badge-stamp load-new-reason">{text("new", "新出现")}</span> : null}
                    </td>
                    <td className="mono">{reasonDetail(reason.column, reason.target_type) || "—"}</td>
                    <td className="num">{count(String(reason.sampled_rows))}</td>
                    <td className="num">{reason.loads}</td>
                    <td className="mono">{utcDay(reason.first_seconds)} · {utcDay(reason.last_seconds)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Card>

        <Card
          footer={
            logs.total > page ? (
              <div className="load-history-pager">
                <span>
                  {text(
                    `${offset + 1}–${Math.min(offset + page, logs.total)} of ${logs.total}`,
                    `第 ${offset + 1}–${Math.min(offset + page, logs.total)} 份，共 ${logs.total} 份`,
                  )}
                </span>
                <div className="load-inline-filters">
                  <Button disabled={offset === 0} onClick={() => onOffset(Math.max(0, offset - page))} size="sm" variant="secondary">
                    {text("Previous", "上一页")}
                  </Button>
                  <Button disabled={offset + page >= logs.total} onClick={() => onOffset(offset + page)} size="sm" variant="secondary">
                    {text("Next", "下一页")}
                  </Button>
                </div>
              </div>
            ) : undefined
          }
          span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
          title={text("Saved error logs", "已保存的错误日志")}
          variant="table"
        >
          {logs.logs.length === 0 ? (
            <div className="state-box">
              <p>{text("Nothing matches.", "没有符合条件的错误日志。")}</p>
            </div>
          ) : (
            <table className="data-table load-table">
              <thead>
                <tr>
                  <th scope="col">{text("Finished (UTC)", "完成时间（UTC）")}</th>
                  <th scope="col">Label</th>
                  <th scope="col">{text("Table", "表")}</th>
                  <th scope="col">{text("Type", "类型")}</th>
                  <th className="num" scope="col">{text("Filtered", "过滤行")}</th>
                  <th className="num" scope="col">{text("Sampled", "样本行")}</th>
                  <th scope="col">{text("Reasons", "原因")}</th>
                  <th scope="col">{text("State", "状态")}</th>
                  <th scope="col"><span className="sr-only">{text("Actions", "操作")}</span></th>
                </tr>
              </thead>
              <tbody>
                {logs.logs.map((log) => {
                  const open = log.state === "saved";
                  const tone = logTone(log.state);
                  const dot = tone === "ok"
                    ? "status-dot status-dot--ok"
                    : tone === "warn"
                      ? "status-dot status-dot--warn"
                      : "status-dot status-dot--hollow";
                  return (
                    <tr key={log.job_id}>
                      <td className="mono">{clockText(log.finish_seconds).slice(5)}</td>
                      <td className="mono"><span className="load-label" title={log.label}>{log.label}</span></td>
                      <td className="mono">{log.database}.{log.table}</td>
                      <td>{groupText(log.kind, text)}</td>
                      <td className="num">{count(log.filtered_rows)}</td>
                      <td className="num">{open ? log.sampled : "—"}</td>
                      <td>
                        {log.categories.length
                          ? log.categories.map((item) => `${categoryText(item.category, text)} ${item.count}`).join(" · ")
                          : "—"}
                      </td>
                      <td>
                        <span className="status-inline">
                          <span aria-hidden="true" className={dot} />
                          <span>{logStateText(log.state, log, text)}</span>
                        </span>
                      </td>
                      <td className="num">
                        {open ? (
                          <Button onClick={() => onOpen(log)} size="sm" variant="secondary">
                            {text("View", "查看")}
                          </Button>
                        ) : null}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </Card>
      </PageGrid>
    </div>
  );
}

export function LoadHistoryPanel({
  connectionId,
  includeSystem,
  generation,
  onOpenLog,
  onOpenAlerts,
  focus = null,
  kind: controlledKind,
  text,
}: {
  connectionId: string | null;
  includeSystem: boolean;
  generation: string;
  onOpenLog(target: ErrorLogTarget): void;
  focus?: { target: string | null; table: string | null; kind: LoadKindGroup | null; jobId: string | null; errors: boolean } | null;
  onOpenAlerts?(target: string | null): void;
  /** Optional controlled load-type filter lifted to `.workspace-bar` so nav stays <= 2 levels. */
  kind?: LoadKindGroup | null;
  text: LocalizedText;
}): JSX.Element {
  const tableSelectId = useId();
  const [mode, setMode] = useState<Mode>(focus?.errors ? "errors" : "stats");
  const [days, setDays] = useState<"7" | "30">("30");
  const [ownKind, setOwnKind] = useState<LoadKindGroup | null>(focus && !focus.errors ? focus.kind : null);
  const kind = controlledKind !== undefined ? controlledKind : ownKind;
  const [table, setTable] = useState<string | null>(focus && !focus.errors ? focus.table : null);
  const [jobId, setJobId] = useState<string | null>(focus?.jobId ?? null);
  const [logTable, setLogTable] = useState<string | null>(focus?.errors ? focus.table : null);
  const [category, setCategory] = useState<LoadErrorCategory | null>(null);
  const [offset, setOffset] = useState(0);

  const dayNumber = days === "7" ? 7 : 30;
  const shownTarget = focus?.target ?? null;
  const routineMode = mode === "stats" && kind === "routine_load";
  const statsKind = kind === "routine_load" ? null : kind;

  const history = useMonitorRead(
    (signal) =>
      doctorApi.readLoadHistory(
        { connection_id: connectionId, target: shownTarget, days: dayNumber, kind: statsKind, table, include_system: includeSystem },
        signal,
      ),
    `history:${connectionId}:${shownTarget ?? ""}:${dayNumber}:${statsKind ?? ""}:${table ?? ""}:${includeSystem}`,
    generation,
    "load_history_failed",
  );
  const routine = useMonitorRead(
    routineMode
      ? (signal) => doctorApi.readLoadHistoryRoutine({ connection_id: connectionId, target: shownTarget, days: dayNumber, job_id: jobId }, signal)
      : null,
    `routine-history:${connectionId}:${shownTarget ?? ""}:${dayNumber}:${jobId ?? ""}`,
    generation,
    "load_history_failed",
  );
  const errors = useMonitorRead(
    mode === "errors"
      ? (signal) =>
          doctorApi.readLoadHistoryErrorLogs(
            { connection_id: connectionId, target: shownTarget, days: dayNumber, table: logTable, category, offset },
            signal,
          )
      : null,
    `error-logs:${connectionId}:${shownTarget ?? ""}:${dayNumber}:${logTable ?? ""}:${category ?? ""}:${offset}`,
    generation,
    "load_history_failed",
  );
  const current = mode === "errors" ? errors : routineMode ? routine : history;
  const data = history.data;
  const alertTarget = data?.target ?? null;
  const loadAlerts = useMonitorRead(
    alertTarget ? (signal) => doctorApi.readLoadAlerts(alertTarget, signal) : null,
    `load-alerts:${alertTarget ?? ""}`,
    generation,
    "load_alerts_failed",
  );
  const activeAlerts = loadAlerts.data ? Number(loadAlerts.data.counts.open) + Number(loadAlerts.data.counts.acknowledged) : 0;

  const openLogs = (name: string) => {
    setLogTable(name);
    setCategory(null);
    setOffset(0);
    setMode("errors");
  };
  const openLog = (log: StoredLoadErrorLog) =>
    onOpenLog({
      connectionId,
      jobId: log.job_id,
      title: log.label,
      meta: text(
        `${groupText(log.kind, text)} · ${log.database}.${log.table} · job_id ${log.job_id} · finished ${clockText(log.finish_seconds).slice(5)} UTC · saved copy`,
        `${groupText(log.kind, text)} · ${log.database}.${log.table} · job_id ${log.job_id} · 完成于 ${clockText(log.finish_seconds).slice(5)} UTC · 本机保存的副本`,
      ),
      stored: true,
      storedState: log.state,
      target: shownTarget ?? data?.target ?? null,
    });

  const body = mode === "errors"
    ? errors.data
      ? (
          <ErrorLogsView
            category={category}
            logs={errors.data}
            offset={offset}
            onCategory={(value) => {
              setCategory(value);
              setOffset(0);
            }}
            onOffset={setOffset}
            onOpen={openLog}
            onTable={(value) => {
              setLogTable(value);
              setOffset(0);
            }}
            table={logTable}
            text={text}
          />
        )
      : null
    : routineMode
      ? routine.data
        ? <RoutineView onJob={setJobId} routine={routine.data} text={text} />
        : null
      : data
        ? <StatsView history={data} kind={statsKind} onOpenLogs={openLogs} text={text} />
        : null;

  return (
    <div className="load-history">
      <div className="load-history-toolbar">
        <div className="load-inline-filters">
          <SegTabs
            aria-label={text("History content", "历史内容")}
            items={[
              { value: "stats", label: text("Statistics", "统计") },
              {
                value: "errors",
                label: text("Error logs", "错误日志"),
                count: data ? data.error_logs.saved : undefined,
              },
            ]}
            onValueChange={setMode}
            size="sm"
            value={mode}
          />
          <SegTabs
            aria-label={text("Range", "时间范围")}
            items={[
              { value: "7", label: text("7 days", "7 天") },
              { value: "30", label: text("30 days", "30 天") },
            ]}
            onValueChange={setDays}
            size="sm"
            value={days}
          />
          {controlledKind === undefined && mode === "stats" ? (
            <label className="filter-chip-input">
              <span>{text("Type", "类型")}</span>
              <select
                onChange={(event) => {
                  setOwnKind((event.target.value || null) as LoadKindGroup | null);
                  setJobId(null);
                }}
                value={kind ?? ""}
              >
                <option value="">{text("All", "全部")}</option>
                <option value="stream_load">Stream Load</option>
                <option value="routine_load">Routine Load</option>
                <option value="insert">INSERT</option>
                <option value="broker">Broker Load</option>
              </select>
            </label>
          ) : null}
          {mode === "stats" && !routineMode && data ? (
            <label className="filter-chip-input" htmlFor={tableSelectId}>
              <span>{text("Table", "表")}</span>
              <select id={tableSelectId} onChange={(event) => setTable(event.target.value || null)} value={table ?? ""}>
                <option value="">{text(`All (${data.table_options.length})`, `全部（${data.table_options.length}）`)}</option>
                {data.table_options.map((name) => (
                  <option key={name} value={name}>{name}</option>
                ))}
              </select>
            </label>
          ) : null}
        </div>
        <div className="load-inline-filters">
          {connectionId ? (
            <CollectionStrip connectionId={connectionId} history={data} text={text} />
          ) : (
            <span className="load-muted">
              {text(
                `Not connected: saved history of ${data?.target ?? shownTarget ?? "most recent cluster"}`,
                `未连接：本机保存的 ${data?.target ?? shownTarget ?? "最近采集集群"} 历史`,
              )}
            </span>
          )}
          <RefreshBar error={current.error} loading={current.loading} onRefresh={current.refresh} text={text} updatedSeconds={current.updatedSeconds} />
        </div>
      </div>

      {activeAlerts > 0 ? (
        <div className="notice-banner notice-banner--warn load-history-alerts" role="status">
          <span>
            {text(`${activeAlerts} load alert${activeAlerts === 1 ? "" : "s"} to handle (new or acknowledged).`, `有 ${activeAlerts} 条待处理的导入告警（新告警或已确认）。`)}{" "}
            {loadAlerts.data && !loadAlerts.data.thresholds.calibrated ? (
              <BadgeStamp kind="planned" />
            ) : null}
          </span>
          {onOpenAlerts ? (
            <Button onClick={() => onOpenAlerts(alertTarget)} size="sm" variant="secondary">
              {text("View load alerts", "查看导入告警")}
            </Button>
          ) : null}
        </div>
      ) : null}

      {body ?? (
        current.error ? (
          <MonitorReadError error={current.error} onRetry={current.refresh} text={text} title={loadFailureText(current.error, text)} />
        ) : (
          <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>
        )
      )}
    </div>
  );
}
