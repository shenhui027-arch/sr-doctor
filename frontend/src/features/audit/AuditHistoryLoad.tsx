// Design board O2 (查询 · 审计历史 · 概览 · 30 天): daily totals, the weekly
// rhythm heatmap and this week against last week, all from the local history
// store's hourly aggregates. Every number, level and coordinate comes from
// the server; days that were not collected are drawn as gaps, never as 0.
import { useEffect, useState, type JSX } from "react";
import type {
  AuditPerformanceValueTick,
  HistoryLoadCoverage,
  HistoryLoadDay,
  HistoryLoadResponse,
} from "../../../../contracts/generated/doctor-contracts";
import { Card, InfoTip, KpiCard, SegTabs, StateStamp } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import type { DoctorApi } from "../../lib/api";
import { countText } from "../../lib/duration-display";
import { LimitationNotes } from "../shell/LimitationNotes";
import { auditClockText, metricText } from "./audit-display";
import "./audit-workload.css";

type Metric = "executions" | "errors" | "query_time";

const X0 = 56;
const PW = 888;
const YB = 212;
const PH = 196;
const WEEKDAYS: Array<[string, string]> = [
  ["Mon", "周一"],
  ["Tue", "周二"],
  ["Wed", "周三"],
  ["Thu", "周四"],
  ["Fri", "周五"],
  ["Sat", "周六"],
  ["Sun", "周日"],
];

const COVERAGE_LABELS: Record<HistoryLoadCoverage, readonly [string, string]> = {
  complete: ["Complete", "完整"],
  partial: ["Partial", "部分"],
  not_collected: ["Not collected", "未采集"],
};

const METRIC_LABELS: Record<Metric, readonly [string, string]> = {
  executions: ["Executions", "执行次数"],
  errors: ["Failures", "失败"],
  query_time: ["Cumulative time", "累计耗时"],
};

export type AuditHistoryLoadProps = {
  api: Pick<DoctorApi, "readHistoryLoad">;
  connectionId: string;
  describeFailure(cause: unknown): string;
  text: LocalizedText;
};

export function AuditHistoryLoad({ api, connectionId, describeFailure, text }: AuditHistoryLoadProps): JSX.Element {
  const [state, setState] = useState<{ key: string; response: HistoryLoadResponse | null; error: string | null } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    api.readHistoryLoad({ connection_id: connectionId, days: 30 }, controller.signal)
      .then((response) => {
        if (!controller.signal.aborted) setState({ key: connectionId, response, error: null });
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setState({ key: connectionId, response: null, error: describeFailure(cause) });
      });
    return () => controller.abort();
    // The read depends only on the connection; the callbacks are stable in effect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectionId]);
  const current = state?.key === connectionId ? state : null;
  return (
    <section aria-busy={!current} aria-label={text("30-day load", "30 天负载")} className="audit-history-load">
      {current?.error ? (
        <div className="notice-banner notice-banner--danger" role="alert">{current.error}</div>
      ) : !current?.response ? (
        <div className="state-box" role="status">{text("Reading the local history…", "正在读取本地历史库…")}</div>
      ) : (
        <AuditHistoryLoadView response={current.response} text={text} />
      )}
    </section>
  );
}

function count(value: string | null | undefined): string {
  return value === null || value === undefined ? "—" : countText(value) ?? value;
}

function day(seconds: number): string {
  return auditClockText(String(seconds * 1000)).slice(5, 10);
}

function tickText(tick: AuditPerformanceValueTick): string {
  return tick.value.kind === "count" ? count(tick.value.count) : metricText(tick.value.metric);
}

function coverageText(coverage: HistoryLoadCoverage, text: LocalizedText): string {
  return lookupEnumLabel(COVERAGE_LABELS, coverage, text);
}

function metricLabel(metric: Metric, text: LocalizedText): string {
  return lookupEnumLabel(METRIC_LABELS, metric, text);
}

export function AuditHistoryLoadView({ response, text }: { response: HistoryLoadResponse; text: LocalizedText }): JSX.Element {
  if (response.target === null) {
    return (
      <Card title={text("30-day history", "30 天历史")} variant="detail">
        <div className="state-box">
          <p>{text("There is no history for this cluster yet, so the 30-day view is unavailable; the time-window view still works.", "这个集群还没有历史数据，30 天视图不可用；所选时间窗视图照常可用。")}</p>
          <p className="muted">{text("Remote clusters are connected without collection by default; enabling it in production requires the read-only pilot (G1, pending).", "远端集群默认只连接、不采集；生产启用采集前需先通过生产只读试点（G1，待验证）。")}</p>
        </div>
      </Card>
    );
  }
  const partial = response.collected_hours < response.expected_hours;
  const lastSegment = response.segments.length > 1 ? response.segments[response.segments.length - 1] : null;
  const segmentFoot = response.segments.length === 0
    ? text("no comparable segment recorded", "尚无可比分段")
    : lastSegment
      ? text(`latest from ${day(lastSegment.first_day_seconds)}`, `最近一段始于 ${day(lastSegment.first_day_seconds)}`)
      : text("one version and rule set", "同一版本与规则");

  return (
    <div className="audit-history-stack">
      <div className="audit-history-read" role="status">
        <span>{text("Source: local history, hourly aggregates · the audit table is not read", "来源：本地历史库小时聚合 · 不读取审计表")}</span>
        <span className="mono">{response.target} · {day(response.start_seconds)} – {day(response.end_seconds - 86400)} UTC</span>
        <span className={partial ? "state-stamp state-stamp--warn" : "state-stamp"}>
          {partial
            ? text(`Partial · ${response.collected_hours} / ${response.expected_hours} hours collected`, `部分 · ${response.collected_hours} / ${response.expected_hours} 小时已采集`)
            : text(`${response.collected_hours} / ${response.expected_hours} hours collected`, `${response.collected_hours} / ${response.expected_hours} 小时已采集`)}
        </span>
      </div>

      <section aria-label={text("30-day summary", "30 天汇总")} className="kpi-grid kpi-grid--5">
        <KpiCard
          foot={text("collected hours only", "只含已采集的小时")}
          label={text("Executions in 30 days", "30 天执行次数")}
          value={response.executions === null ? <StateStamp state="missing" /> : count(response.executions)}
        />
        <KpiCard
          foot={text("state = ERR, collected hours only", "state = ERR，只含已采集的小时")}
          label={text("Failed", "失败")}
          tone={response.errors !== null && response.errors !== "0" ? "critical" : "default"}
          value={response.errors === null ? <StateStamp state="missing" /> : count(response.errors)}
        />
        <KpiCard
          foot={text("hours read, of the hours elapsed", "已读取的小时 / 已过去的小时")}
          label={text("Collection coverage", "采集覆盖")}
          value={`${response.collected_hours} / ${response.expected_hours}`}
        />
        <KpiCard
          foot={text(`of ${count(response.rows_read)} rows read`, `读取 ${count(response.rows_read)} 行中`)}
          label={text("Rows attributed", "归入模板的行")}
          value={count(response.rows_attributed)}
        />
        <KpiCard
          foot={segmentFoot}
          label={text("Comparable segments", "可比分段")}
          value={response.segments.length === 0 ? <StateStamp state="missing" /> : String(response.segments.length)}
        />
      </section>

      <DailyChart response={response} text={text} />

      <div className="audit-history-grid">
        <Heatmap response={response} text={text} />
        <WeekTable response={response} text={text} />
      </div>

      {response.limitations.length > 0 && (
        <details className="disclosure">
          <summary>{text("Limitations", "限制")}</summary>
          <div className="disclosure-body">
            <LimitationNotes codes={response.limitations} text={text} />
          </div>
        </details>
      )}
    </div>
  );
}

function projection(item: HistoryLoadDay, metric: Metric): number | null {
  return metric === "executions" ? item.executions_permyriad : metric === "errors" ? item.errors_permyriad : item.query_time_permyriad;
}

function DailyChart({ response, text }: { response: HistoryLoadResponse; text: LocalizedText }): JSX.Element {
  const [metric, setMetric] = useState<Metric>("executions");
  const days = response.days;
  const ticks = metric === "executions" ? response.axes.executions : metric === "errors" ? response.axes.errors : response.axes.query_time;
  const boundaries = days.flatMap((item, index) => (index > 0 && item.segment !== days[index - 1].segment ? [index] : []));
  const lastBoundary = boundaries.length ? boundaries[boundaries.length - 1] : null;
  const y = (permyriad: number): number => YB - (PH * permyriad) / 10000;

  return (
    <Card
      actions={
        <SegTabs<Metric>
          aria-label={text("Metric", "指标")}
          items={[
            { value: "executions", label: metricLabel("executions", text) },
            { value: "errors", label: metricLabel("errors", text) },
            { value: "query_time", label: metricLabel("query_time", text) },
          ]}
          onValueChange={setMetric}
          size="sm"
          value={metric}
        />
      }
      footer={
        <>
          <div className="audit-history-legend">
            <span><i className={`swatch ${metric === "errors" ? "failed" : "bar"}`} />{text("Collected", "已采集")}</span>
            <span><i className="swatch gap" />{text("Partly collected", "部分采集")}</span>
            <span><i className="swatch recent" />{text("Not collected", "未采集")}</span>
            {boundaries.length > 0 ? (
              <span><i className="swatch audit-history-before-swatch" />{text("Before the version change · not compared", "版本变化前 · 不与之后比较")}</span>
            ) : null}
          </div>
          <InfoTip label={text("About daily totals", "关于按日总量")} side="top">
            {text("Additive totals only; percentiles cannot be combined from hourly aggregates.", "只画可相加的量；分位数不能由小时聚合合成，30 天视图不提供。")}
          </InfoTip>
        </>
      }
      title={text(`Daily ${metricLabel(metric, text).toLowerCase()} · ${days.length} days`, `每日${metricLabel(metric, text)} · ${days.length} 天`)}
      variant="chart"
    >
      {days.length === 0 ? (
        <div className="state-box" role="status">
          <StateStamp state="missing" />
          <span>{text("No daily aggregates recorded for this window.", "该时间窗尚无按日聚合数据。")}</span>
        </div>
      ) : (() => {
        const width = PW / days.length;
        const barWidth = width > 4 ? width - 4 : width * 0.75;
        return (
          <svg
            aria-label={text(`${metricLabel(metric, text)} per day; not-collected days are gaps`, `每日${metricLabel(metric, text)}；未采集的日期留空`)}
            className={`audit-history-plot metric-${metric === "errors" ? "count" : "scan"}`}
            role="img"
            viewBox="0 0 960 250"
          >
            <defs>
              <pattern height="6" id="audit-history-hatch" patternTransform="rotate(45)" patternUnits="userSpaceOnUse" width="6">
                <rect fill="var(--warning-bg)" height="6" width="6" />
                <line stroke="var(--warning)" strokeOpacity="0.45" strokeWidth="2" x1="0" x2="0" y1="0" y2="6" />
              </pattern>
            </defs>
            {lastBoundary !== null ? <rect className="audit-history-before" height={PH + 4} width={lastBoundary * width} x={X0} y={YB - PH - 4} /> : null}
            {ticks.map((tick) => (
              <g className="tick" key={tick.y_permyriad}>
                <line x1={X0} x2={X0 + PW} y1={y(tick.y_permyriad)} y2={y(tick.y_permyriad)} />
                <text textAnchor="end" x={X0 - 6} y={y(tick.y_permyriad) + 4}>{tickText(tick)}</text>
              </g>
            ))}
            {days.map((item, index) => {
              const left = X0 + index * width + 2;
              const value = projection(item, metric);
              if (item.coverage === "not_collected" || value === null) {
                return (
                  <rect className="recent" height={PH - 8} key={index} width={barWidth} x={left} y={YB - PH + 8}>
                    <title>{`${day(item.start_seconds)} · ${coverageText(item.coverage, text)}`}</title>
                  </rect>
                );
              }
              return (
                <rect
                  className={item.coverage === "partial" ? "gap" : metric === "errors" ? "failed" : "bar"}
                  height={(PH * value) / 10000}
                  key={index}
                  width={barWidth}
                  x={left}
                  y={y(value)}
                >
                  <title>{`${day(item.start_seconds)} · ${coverageText(item.coverage, text)}`}</title>
                </rect>
              );
            })}
            {boundaries.map((index) => (
              <g key={`boundary-${index}`}>
                <line className="audit-history-boundary" x1={X0 + index * width} x2={X0 + index * width} y1={YB - PH - 4} y2={YB} />
                <text className="time" x={X0 + index * width + 6} y={YB - PH + 10}>
                  {`${response.segments[days[index].segment]?.engine_versions.join(", ") ?? ""} · ${text("not comparable", "不可比")}`}
                </text>
              </g>
            ))}
            <line className="axis" x1={X0} x2={X0 + PW} y1={YB} y2={YB} />
            {days.map((item, index) =>
              index % 5 === 0 || index === days.length - 1 ? (
                <text className="time" key={`t-${index}`} textAnchor="middle" x={X0 + index * width + width / 2} y={YB + 20}>
                  {day(item.start_seconds)}
                </text>
              ) : null,
            )}
          </svg>
        );
      })()}
    </Card>
  );
}

function Heatmap({ response, text }: { response: HistoryLoadResponse; text: LocalizedText }): JSX.Element {
  const hours = Array.from({ length: 24 }, (_, hour) => hour);
  return (
    <Card
      footer={
        <div className="audit-history-legend audit-history-scale">
          <span>{text("Fewer", "少")}</span>
          {[0, 1, 2, 3, 4, 5].map((level) => <i className={`level-${level}`} key={level} />)}
          <span>{text("More", "多")}</span>
          <span><i className="missing" />{text("Not collected · not drawn, not interpolated", "未采集 · 不画、不插值")}</span>
        </div>
      }
      subtitle={text("average executions per completely collected hour", "每个完整采集小时的平均执行次数")}
      title={text("Weekly rhythm · weekday × hour (UTC)", "一周负载节律 · 星期 × 小时（UTC）")}
      variant="chart"
    >
      <div className="audit-history-heatmap-wrap">
        <table aria-label={text("Average executions by weekday and hour", "按星期与小时的平均执行次数")} className="audit-history-heatmap">
          <thead>
            <tr>
              <th scope="col"><span className="sr-only">{text("Weekday", "星期")}</span></th>
              {hours.map((hour) => <th key={hour} scope="col">{hour % 3 === 0 ? String(hour).padStart(2, "0") : ""}</th>)}
            </tr>
          </thead>
          <tbody>
            {WEEKDAYS.map(([en, zh], weekday) => (
              <tr key={en}>
                <th scope="row">{text(en, zh)}</th>
                {hours.map((hour) => {
                  const cell = response.heatmap.find((item) => item.weekday === weekday && item.hour === hour);
                  const label = cell?.average_executions === null || !cell
                    ? `${text(en, zh)} ${String(hour).padStart(2, "0")}:00 · ${text("not collected", "未采集")}`
                    : `${text(en, zh)} ${String(hour).padStart(2, "0")}:00 · ${text(`average ${count(cell.average_executions)} over ${cell.collected_hours} h`, `平均 ${count(cell.average_executions)} 次（${cell.collected_hours} 小时）`)}`;
                  return (
                    <td className={cell?.level === null || cell?.level === undefined ? "missing" : `level-${cell.level}`} key={hour} title={label}>
                      <span className="sr-only">{label}</span>
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Card>
  );
}

function WeekTable({ response, text }: { response: HistoryLoadResponse; text: LocalizedText }): JSX.Element {
  return (
    <Card
      actions={
        <InfoTip label={text("Comparison rules", "同比规则")} side="top">
          {text("A day partly collected on either side is not compared; with 0 last week only the absolute change is shown.", "任一侧部分采集时不比较；上周为 0 时只给绝对变化，不给百分比。")}
        </InfoTip>
      }
      subtitle={text("same weekday, executions", "同一星期几 · 执行次数")}
      title={text("This week vs last week", "本周与上周")}
      variant="table"
    >
      {response.week.length === 0 ? (
        <div className="state-box">{text("The window is too short for a week-over-week comparison.", "窗口不足两周，无法同比。")}</div>
      ) : (
        <table className="data-table">
          <thead>
            <tr>
              <th scope="col">{text("Day", "日")}</th>
              <th className="num" scope="col">{text("Last week", "上周")}</th>
              <th className="num" scope="col">{text("This week", "本周")}</th>
              <th className="num" scope="col">{text("Change", "变化")}</th>
            </tr>
          </thead>
          <tbody>
            {response.week.map((row) => (
              <tr key={row.current_day_seconds}>
                <td className="mono">{day(row.current_day_seconds)} / {day(row.previous_day_seconds)}</td>
                <td className="num">{row.previous_executions === null ? <StateStamp state="missing" /> : count(row.previous_executions)}</td>
                <td className="num">{row.current_executions === null ? <StateStamp state="missing" /> : count(row.current_executions)}</td>
                <td className="num">
                  {row.comparison.state === "exact" ? (
                    row.comparison.permille === null ? (
                      <span className="mono">{text(`${row.comparison.delta} · no %`, `${row.comparison.delta} · 无 %`)}</span>
                    ) : (
                      <span className="mono">{`${row.comparison.permille >= 0 ? "+" : "−"}${(Math.abs(row.comparison.permille) / 10).toFixed(1)}%`}</span>
                    )
                  ) : row.comparison.reason === "partial_coverage" ? (
                    <span className="state-stamp state-stamp--warn" data-honest-state="partial">
                      ◐ {text("partial · not compared", "部分 · 不比较")}
                    </span>
                  ) : (
                    <StateStamp state="not_comparable" />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}
