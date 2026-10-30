// Design board O1 (查询 · 审计历史 · 概览): window cards, the bucketed trend
// chart with its selected-bucket aside, the latency histogram, failures by
// errorCode, workload composition and the top repeated SQL. Every number and
// every coordinate comes from the server; this file only lays them out.
import { Fragment, useEffect, useMemo, useState, type JSX, type ReactNode } from "react";
import type {
  AuditAggregateFact,
  AuditConcurrencyPresentation,
  AuditCoverage,
  AuditFamilyPresentation,
  AuditMetricFact,
  AuditPerformanceBucket,
  AuditPerformanceBucketChoice,
  AuditPerformanceDimensionId,
  AuditPerformanceQuantiles,
  AuditPerformanceRankKey,
  AuditPerformanceResponse,
  AuditPerformanceTickValue,
  AuditPerformanceValue,
  AuditPerformanceValueTick,
  AuditScope,
} from "../../../../contracts/generated/doctor-contracts";
import type { DoctorApi } from "../../lib/api";
import type { LocalizedText } from "../../i18n/locale";
import { countText } from "../../lib/duration-display";
import { LimitationNotes } from "../shell/LimitationNotes";
import { auditClockText, metricText, positiveCount } from "./audit-display";
import "./audit-performance.css";

type Metric = "count" | "latency" | "scan" | "cpu" | "memory" | "pending";
type PerformanceState = { key: string; response: AuditPerformanceResponse | null; error: string | null };

const X0 = 56;
const PW = 888;
const YB = 212;
const PH = 196;
const BUCKETS: Array<{ id: string; choice: AuditPerformanceBucketChoice }> = [
  { id: "auto", choice: { state: "auto" } },
  ...[60, 300, 900, 3600, 21600, 86400].map((seconds) => ({ id: String(seconds), choice: { state: "fixed", seconds } as AuditPerformanceBucketChoice })),
];
const DIMENSIONS: AuditPerformanceDimensionId[] = ["user", "database", "resource_group", "catalog"];
const RANK_KEYS: AuditPerformanceRankKey[] = ["query_time_sum", "execution_count", "error_count"];

export type AuditPerformanceOverviewProps = {
  api: Pick<DoctorApi, "readAuditPerformance">;
  connectionId: string;
  scope: AuditScope;
  concurrency: AuditConcurrencyPresentation | null;
  coverage: AuditCoverage | null;
  families: readonly AuditFamilyPresentation[];
  /** Label of the server ranking the families arrive in. */
  familySortLabel: string;
  /** Returns the readable failure and handles an ended session. */
  describeFailure(cause: unknown): string;
  onDrill(startMilliseconds: string, endMilliseconds: string, view: "occurrences" | "families"): void;
  onFilter(dimension: AuditPerformanceDimensionId, value: string): void;
  onOpenFamilies(): void;
  filterableUser: boolean;
  /** The load card (task 52), drawn after the failure row. */
  loadCard?: ReactNode;
  text: LocalizedText;
};

export function AuditPerformanceOverview(props: AuditPerformanceOverviewProps): JSX.Element {
  const { api, connectionId, scope, describeFailure, text } = props;
  const [bucket, setBucket] = useState("auto");
  const [state, setState] = useState<PerformanceState | null>(null);
  const key = JSON.stringify([connectionId, scope, bucket]);
  useEffect(() => {
    const controller = new AbortController();
    const choice = BUCKETS.find((item) => item.id === bucket)?.choice ?? { state: "auto" };
    api.readAuditPerformance({ connection_id: connectionId, scope, bucket: choice }, controller.signal)
      .then((response) => { if (!controller.signal.aborted) setState({ key, response, error: null }); })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setState({ key, response: null, error: describeFailure(cause) }); });
    return () => controller.abort();
    // `key` encodes every input of the read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const current = state?.key === key ? state : null;
  const response = current?.response && current.response.connection_id === connectionId ? current.response : null;
  return <section aria-label={text("Performance overview", "性能概览")} aria-busy={!current} className="audit-performance">
    {current?.error
      ? <p className="audit-error" role="alert">{current.error}</p>
      : !response
      ? <p className="audit-empty" role="status">{text("Reading the audit window aggregation…", "正在聚合读取审计窗口…")}</p>
      : <AuditPerformanceView {...props} bucket={bucket} onBucket={setBucket} response={response} />}
  </section>;
}

export function AuditPerformanceView({ bucket, onBucket, response, ...props }: AuditPerformanceOverviewProps & { bucket: string; onBucket(id: string): void; response: AuditPerformanceResponse }): JSX.Element {
  const { text } = props;
  const lastFilled = useMemo(() => {
    let index = response.buckets.length - 1;
    while (index > 0 && response.buckets[index].summary.execution_count === "0") index -= 1;
    return Math.max(index, 0);
  }, [response]);
  const [selected, setSelected] = useState(lastFilled);
  useEffect(() => setSelected(lastFilled), [lastFilled]);
  return <>
    <p className="audit-performance-read" role="status">
      <span className="mono">{auditClockText(String(response.scope.window.start_seconds * 1000))} – {auditClockText(String(response.scope.window.end_seconds * 1000))}</span>
      <span>{text("One bounded aggregation read of the audit table · computed by the server", "审计表一次有界聚合读取 · 服务端计算")}</span>
    </p>
    <PerformanceCards concurrency={props.concurrency} response={response} text={text} />
    <div className="audit-performance-row">
      <PerformanceChart bucket={bucket} onBucket={onBucket} onSelect={setSelected} response={response} selected={selected} text={text} />
      <BucketAside bucket={response.buckets[selected] ?? null} count={response.buckets.length} index={selected} onDrill={props.onDrill} onSelect={setSelected} text={text} />
    </div>
    <div className="audit-performance-row">
      <LatencyHistogram response={response} text={text} />
      <FailureBreakdown response={response} text={text} />
    </div>
    {props.loadCard}
    <div className="audit-performance-row">
      <WorkloadComposition filterableUser={props.filterableUser} onFilter={props.onFilter} response={response} text={text} />
      <TopFamilies families={props.families} label={props.familySortLabel} onOpen={props.onOpenFamilies} text={text} />
    </div>
    <CoveragePanel coverage={props.coverage} response={response} text={text} />
  </>;
}

function percentText(permyriad: number | null | undefined): string {
  return permyriad === null || permyriad === undefined ? "—" : `${(permyriad / 100).toFixed(permyriad < 1000 ? 2 : 1)}%`;
}

function count(value: string): string {
  return countText(value) ?? value;
}

function factText(fact: AuditMetricFact | null | undefined): string {
  return fact ? metricText(fact) : "—";
}

function quantileText(quantiles: AuditPerformanceQuantiles): string {
  return [quantiles.p50, quantiles.p95, quantiles.p99].map(factText).join(" / ");
}

function aggregateText(fact: AuditAggregateFact): string {
  return metricText(fact.metric);
}

function tickText(value: AuditPerformanceTickValue): string {
  return value.kind === "count" ? count(value.count) : metricText(value.metric);
}

function valueText(value: AuditPerformanceValue, text: LocalizedText): string {
  if (value.state === "sql_null") return text("(empty)", "（为空）");
  if (value.state === "conflicting") return text("(duplicate rows disagree)", "（重复行不一致）");
  return value.value === "" ? text("(empty string)", "（空字符串）") : value.value;
}

const y = (permyriad: number): number => YB - (PH * permyriad) / 10000;
const x = (permyriad: number): number => X0 + (PW * permyriad) / 10000;

function spark(buckets: readonly AuditPerformanceBucket[], pick: (bucket: AuditPerformanceBucket) => number | null | undefined): string {
  return buckets.flatMap((bucket) => {
    const value = pick(bucket);
    if (value === null || value === undefined) return [];
    const at = (96 * (bucket.x_permyriad + bucket.width_permyriad / 2)) / 10000;
    return [`${at.toFixed(1)},${(25 - (22 * value) / 10000).toFixed(1)}`];
  }).join(" ");
}

function PerformanceCards({ concurrency, response, text }: { concurrency: AuditConcurrencyPresentation | null; response: AuditPerformanceResponse; text: LocalizedText }): JSX.Element {
  const window = response.window;
  const nullTime = response.latency_histogram.exclusions.sql_null_count;
  const peak = concurrency?.series.state === "exact" ? concurrency.series.maximum_active_count : null;
  const cards: Array<{ label: string; value: string; unit?: string; note: string; line?: string; tone?: "hot"; compact?: boolean }> = [
    { label: text("Executions", "执行次数"), value: count(window.execution_count), note: text("unique retained executions · deduplicated", "保留的唯一执行 · 已去重"), line: spark(response.buckets, (bucket) => bucket.projection.execution_permyriad) },
    { label: text("Failed", "失败"), value: count(window.error_count), unit: percentText(window.error_share_permyriad), note: text("exact state = ERR count", "state = ERR 的精确计数"), line: spark(response.buckets, (bucket) => bucket.projection.error_permyriad), tone: "hot" },
    { label: text("Duration P50 / P95 / P99", "耗时 P50 / P95 / P99"), value: quantileText(window.query_time), compact: true, note: positiveCount(nullTime) ? text(`${nullTime} records with a NULL queryTime are left out`, `${nullTime} 条 queryTime 为空，未计入`) : text(`${count(window.query_time.eligible_count)} durations, whole window`, `全窗口 ${count(window.query_time.eligible_count)} 个耗时`), line: spark(response.buckets, (bucket) => bucket.projection.query_time_p95_permyriad) },
    { label: text("Cumulative CPU", "CPU 累计"), value: aggregateText(window.cpu_cost_sum.aggregate), note: positiveCount(window.cpu_cost_sum.excluded_count) ? text(`${window.cpu_cost_sum.excluded_count} records lack cpuCostNs`, `${window.cpu_cost_sum.excluded_count} 条缺少 cpuCostNs`) : text("summed cpuCostNs, not elapsed time", "cpuCostNs 求和，不是墙钟时间"), line: spark(response.buckets, (bucket) => bucket.projection.cpu_cost_permyriad) },
    { label: text("Cumulative scanned bytes", "扫描量累计"), value: aggregateText(window.scan_bytes_sum.aggregate), note: positiveCount(window.scan_bytes_sum.excluded_count) ? text(`${window.scan_bytes_sum.excluded_count} records lack scanBytes, not counted`, `${window.scan_bytes_sum.excluded_count} 条 scanBytes 为空，未计入`) : text("summed scanBytes", "scanBytes 求和"), line: spark(response.buckets, (bucket) => bucket.projection.scan_bytes_permyriad) },
    { label: text("Peak in flight", "执行中峰值"), value: peak ? count(peak) : "—", note: peak ? text("estimated from queryTime intervals", "按 queryTime 区间推算") : text("not available for this window", "此窗口不可用") },
  ];
  return <section aria-label={text("Window summary", "窗口汇总")} className="audit-performance-cards">
    {cards.map((card) => <div className={`audit-performance-card${card.tone === "hot" && window.error_count !== "0" ? " hot" : ""}${card.compact ? " compact" : ""}`} key={card.label}>
      <div className="audit-performance-card-label">{card.label}</div>
      <div className="audit-performance-card-value"><span title={card.value}>{card.value}</span>{card.unit ? <small>{card.unit}</small> : null}
        {card.line ? <svg aria-hidden="true" viewBox="0 0 96 28" preserveAspectRatio="none"><polyline points={card.line} /></svg> : null}</div>
      <div className="audit-performance-card-note" title={card.note}>{card.note}</div>
    </div>)}
  </section>;
}

function metricLabel(metric: Metric, text: LocalizedText): string {
  return {
    count: text("Volume and failures", "查询量与失败"),
    latency: text("Duration percentiles", "耗时分位"),
    scan: text("Scanned bytes", "扫描量"),
    cpu: text("CPU", "CPU 累计"),
    memory: text("Peak memory", "内存峰值"),
    pending: text("Queue time", "排队时间"),
  }[metric];
}

function axisFor(metric: Metric, response: AuditPerformanceResponse): AuditPerformanceValueTick[] {
  const axes = response.axes;
  return { count: axes.execution, latency: axes.query_time_log, scan: axes.scan_bytes, cpu: axes.cpu_cost, memory: axes.memory_cost, pending: axes.pending_time }[metric];
}

function PerformanceChart({ bucket, onBucket, onSelect, response, selected, text }: { bucket: string; onBucket(id: string): void; onSelect(index: number): void; response: AuditPerformanceResponse; selected: number; text: LocalizedText }): JSX.Element {
  const [metric, setMetric] = useState<Metric>("count");
  const pendingAvailable = response.window.pending_time.state === "available";
  const buckets = response.buckets;
  const selectedBucket = buckets[selected];
  const bars = (pick: (bucket: AuditPerformanceBucket) => number | null, excluded: (bucket: AuditPerformanceBucket) => string) =>
    buckets.map((item, index) => {
      const left = x(item.x_permyriad) + 1;
      const width = Math.max((PW * item.width_permyriad) / 10000 - 2, 1);
      const value = pick(item);
      if (value === null) {
        return item.summary.execution_count === "0" ? null : <rect className="gap" height={PH - 8} key={index} width={width} x={left} y={YB - PH + 8} />;
      }
      return <rect className={positiveCount(excluded(item)) ? "bar partial" : "bar"} height={(PH * value) / 10000} key={index} width={width} x={left} y={y(value)} />;
    });
  const line = (pick: (bucket: AuditPerformanceBucket) => number | null, className: string) => {
    const segments: string[][] = [[]];
    for (const item of buckets) {
      const value = pick(item);
      if (value === null) { if (segments[segments.length - 1].length) segments.push([]); continue; }
      segments[segments.length - 1].push(`${(x(item.x_permyriad) + (PW * item.width_permyriad) / 20000).toFixed(1)},${y(value).toFixed(1)}`);
    }
    return segments.filter((points) => points.length).map((points, index) => points.length === 1
      ? <circle className={className} cx={points[0].split(",")[0]} cy={points[0].split(",")[1]} key={`${className}-${index}`} r={2.5} />
      : <polyline className={className} key={`${className}-${index}`} points={points.join(" ")} />);
  };
  const marks = metric === "count"
    ? buckets.map((item, index) => {
      const left = x(item.x_permyriad) + 1;
      const width = Math.max((PW * item.width_permyriad) / 10000 - 2, 1);
      const total = (PH * item.projection.execution_permyriad) / 10000;
      const failed = (PH * item.projection.error_permyriad) / 10000;
      return <g key={index}><rect className="ok" height={Math.max(total - failed, 0)} width={width} x={left} y={YB - total + failed} /><rect className="failed" height={failed} width={width} x={left} y={YB - total} /></g>;
    })
    : metric === "latency"
    ? [...line((item) => item.projection.query_time_p99_permyriad, "p99"), ...line((item) => item.projection.query_time_p95_permyriad, "p95"), ...line((item) => item.projection.query_time_p50_permyriad, "p50")]
    : metric === "scan"
    ? bars((item) => item.projection.scan_bytes_permyriad, (item) => item.summary.scan_bytes_sum.excluded_count)
    : metric === "cpu"
    ? bars((item) => item.projection.cpu_cost_permyriad, (item) => item.summary.cpu_cost_sum.excluded_count)
    : metric === "memory"
    ? line((item) => item.projection.memory_cost_permyriad, "line")
    : line((item) => item.projection.pending_time_p95_permyriad, "line");
  const legend: Array<[string, string]> = metric === "count"
    ? [["ok", text("Succeeded", "成功执行")], ["failed", text("Failed (state = ERR)", "失败（state = ERR）")]]
    : metric === "latency"
    ? [["p99", "P99"], ["p95", "P95"], ["p50", "P50"]]
    : metric === "scan" || metric === "cpu"
    ? [["bar", metricLabel(metric, text)], ["partial", text("Some records lack the value", "部分记录缺少该值")], ["gap", text("No value · not drawn", "无可用值 · 不画")]]
    : [["line", metric === "memory" ? text("Largest single-execution memory", "段内单查询内存最大值") : "pendingTimeMs P95"]];
  const note = metric === "latency" ? text("Exact per bucket, log axis; buckets are never combined", "每段内精确分位，对数轴；段间不合并")
    : metric === "cpu" ? text("Summed CPU, not elapsed time", "CPU 求和，不是墙钟时间")
    : metric === "memory" ? text("Not concurrent total memory", "不是并发总内存")
    : metric === "pending" ? text("Only where the audit table has pendingTimeMs", "仅审计表有 pendingTimeMs 时可用")
    : text("Exact server counts", "服务端精确计数");
  return <section aria-labelledby="audit-performance-chart" className="audit-performance-panel audit-performance-chart">
    <div className="audit-performance-head">
      <h2 id="audit-performance-chart">{metricLabel(metric, text)} · {text(`every ${bucketText(response.bucket_seconds, text)}`, `每 ${bucketText(response.bucket_seconds, text)}`)}</h2>
      <div className="audit-performance-controls">
        <div aria-label={text("Metric", "指标")} className="audit-performance-segmented" role="group">
          {(["count", "latency", "scan", "cpu", "memory", "pending"] as Metric[]).map((item) => <button aria-pressed={metric === item} disabled={item === "pending" && !pendingAvailable} key={item} onClick={() => setMetric(item)} title={item === "pending" && !pendingAvailable ? text("The audit table has no pendingTimeMs", "审计表没有 pendingTimeMs") : undefined} type="button">{metricLabel(item, text)}</button>)}
        </div>
        <label className="audit-performance-bucket">{text("Bucket", "时间粒度")}<select onChange={(event) => onBucket(event.target.value)} value={bucket}>{BUCKETS.map((item) => <option key={item.id} value={item.id}>{item.id === "auto" ? text(`Auto · ${bucketText(response.bucket_seconds, text)}`, `自动 · ${bucketText(response.bucket_seconds, text)}`) : bucketText(Number(item.id), text)}</option>)}</select></label>
      </div>
    </div>
    <svg aria-label={`${metricLabel(metric, text)} · ${buckets.length} ${text("buckets", "段")}`} className={`audit-performance-plot metric-${metric}`} role="img" viewBox="0 0 960 250">
      <defs><pattern height="6" id="audit-performance-hatch" patternTransform="rotate(45)" patternUnits="userSpaceOnUse" width="6"><rect fill="var(--warning-tint)" height="6" width="6" /><line stroke="var(--hot-soft)" strokeWidth="2" x1="0" x2="0" y1="0" y2="6" /></pattern></defs>
      {selectedBucket ? <rect className="selected" height={PH + 4} width={Math.max((PW * selectedBucket.width_permyriad) / 10000, 2)} x={x(selectedBucket.x_permyriad)} y={YB - PH - 4} /> : null}
      {axisFor(metric, response).map((tick) => <g className="tick" key={tick.y_permyriad}><line x1={X0} x2={X0 + PW} y1={y(tick.y_permyriad)} y2={y(tick.y_permyriad)} /><text textAnchor="end" x={X0 - 6} y={y(tick.y_permyriad) + 4}>{tickText(tick.value)}</text></g>)}
      {buckets.map((item, index) => item.recent_ingestion ? <rect className="recent" height={PH} key={`recent-${index}`} width={Math.max((PW * item.width_permyriad) / 10000 - 1, 1)} x={x(item.x_permyriad) + 0.5} y={YB - PH} /> : null)}
      {marks}
      <line className="axis" x1={X0} x2={X0 + PW} y1={YB} y2={YB} />
      {response.axes.time_ticks.map((tick) => <text className="time" key={tick.at_milliseconds} textAnchor={tick.x_permyriad === 0 ? "start" : tick.x_permyriad === 10000 ? "end" : "middle"} x={x(tick.x_permyriad)} y={YB + 20}>{tick.label}</text>)}
      {buckets.map((item, index) => <rect className="hit" height={PH + 4} key={`hit-${index}`} onClick={() => onSelect(index)} width={Math.max((PW * item.width_permyriad) / 10000, 2)} x={x(item.x_permyriad)} y={YB - PH - 4}><title>{auditClockText(item.start_milliseconds)}</title></rect>)}
    </svg>
    <div className="audit-performance-foot">
      <div className="audit-performance-legend">
        {legend.map(([kind, label]) => <span key={kind}><i className={`swatch ${kind}`} />{label}</span>)}
        {buckets.some((item) => item.recent_ingestion) ? <span><i className="swatch recent" />{text("May still be arriving", "可能尚未全部落库")}</span> : null}
      </div>
      <span>{note}</span>
    </div>
  </section>;
}

function bucketText(seconds: number, text: LocalizedText): string {
  if (seconds % 86400 === 0) return text(`${seconds / 86400} d`, `${seconds / 86400} 天`);
  if (seconds % 3600 === 0) return text(`${seconds / 3600} h`, `${seconds / 3600} 小时`);
  return text(`${seconds / 60} min`, `${seconds / 60} 分钟`);
}

function BucketAside({ bucket, count: total, index, onDrill, onSelect, text }: { bucket: AuditPerformanceBucket | null; count: number; index: number; onDrill: AuditPerformanceOverviewProps["onDrill"]; onSelect(index: number): void; text: LocalizedText }): JSX.Element {
  if (!bucket) return <aside className="audit-performance-panel audit-performance-aside" />;
  const summary = bucket.summary;
  const flags = [
    bucket.recent_ingestion ? text("AuditLoader writes in batches; this bucket may still be filling.", "AuditLoader 批量写入有延迟，这一段可能尚未全部落库。") : null,
    positiveCount(summary.scan_bytes_sum.excluded_count) ? text(`${summary.scan_bytes_sum.excluded_count} records lack scanBytes; the sum covers the rest.`, `${summary.scan_bytes_sum.excluded_count} 条记录 scanBytes 为空，扫描量只含有值的记录。`) : null,
  ].filter((flag): flag is string => flag !== null);
  const pending = summary.pending_time.state === "available" ? factText(summary.pending_time.quantiles.p95) : text("unavailable", "不可用");
  return <aside aria-labelledby="audit-performance-bucket" className="audit-performance-panel audit-performance-aside">
    <div className="audit-performance-aside-head">
      <div className="muted">{text("Selected bucket", "所选时段")}</div>
      <h2 className="mono" id="audit-performance-bucket">{auditClockText(bucket.start_milliseconds)} – {auditClockText(bucket.end_milliseconds).slice(11)}</h2>
      <div className="audit-performance-steps">
        <button aria-label={text("Previous bucket", "上一时段")} disabled={index === 0} onClick={() => onSelect(index - 1)} type="button">‹ {text("Previous", "上一段")}</button>
        <button aria-label={text("Next bucket", "下一时段")} disabled={index >= total - 1} onClick={() => onSelect(index + 1)} type="button">{text("Next", "下一段")} ›</button>
      </div>
    </div>
    <dl>
      <dt>{text("Executions", "执行次数")}</dt><dd>{count(summary.execution_count)}</dd>
      <dt>{text("Failed", "失败")}</dt><dd>{count(summary.error_count)} · {percentText(summary.error_share_permyriad)}</dd>
      <dt>P50 / P95 / P99</dt><dd>{quantileText(summary.query_time)}</dd>
      <dt>{text("Scanned", "扫描量")}</dt><dd>{aggregateText(summary.scan_bytes_sum.aggregate)}</dd>
      <dt>{text("CPU", "CPU 累计")}</dt><dd>{aggregateText(summary.cpu_cost_sum.aggregate)}</dd>
      <dt>{text("Peak memory", "单查询内存峰值")}</dt><dd>{aggregateText(summary.memory_cost_max.aggregate)}</dd>
      <dt>{text("Queue P95", "排队 P95")}</dt><dd>{pending}</dd>
    </dl>
    {flags.map((flag) => <p className="audit-performance-flag" key={flag}>{flag}</p>)}
    <div className="audit-performance-aside-actions">
      <button className="button accent" disabled={summary.execution_count === "0"} onClick={() => onDrill(bucket.start_milliseconds, bucket.end_milliseconds, "occurrences")} type="button">{text("Executions in this bucket", "查看该时段的单次执行")}</button>
      <button className="button secondary" disabled={summary.execution_count === "0"} onClick={() => onDrill(bucket.start_milliseconds, bucket.end_milliseconds, "families")} type="button">{text("Repeated SQL in this bucket", "该时段内同类 SQL 排行")}</button>
    </div>
  </aside>;
}

function LatencyHistogram({ response, text }: { response: AuditPerformanceResponse; text: LocalizedText }): JSX.Element {
  const histogram = response.latency_histogram;
  const [threshold, setThreshold] = useState(1);
  const slow = histogram.slow[threshold] ?? null;
  const exclusions = histogram.exclusions;
  return <section aria-labelledby="audit-performance-histogram" className="audit-performance-panel">
    <div className="audit-performance-head">
      <h2 id="audit-performance-histogram">{text("Duration distribution", "耗时分布")}</h2>
      <label className="audit-performance-bucket">{text("Mark", "标记阈值")}<select onChange={(event) => setThreshold(Number(event.target.value))} value={threshold}>{histogram.slow.map((item, index) => <option key={item.threshold_ms} value={index}>≥ {thresholdText(item.threshold_ms, text)}</option>)}</select></label>
    </div>
    <div className="audit-performance-histogram">
      {histogram.buckets.map((item) => {
        const marked = slow !== null && BigInt(item.bucket.lower_ms_inclusive) >= BigInt(slow.threshold_ms);
        return <div className="audit-performance-histogram-row" key={item.bucket.bucket_id}>
          <span className="mono label">{item.bucket.display_label_seconds}</span>
          <span className="track"><span className={marked ? "fill marked" : "fill"} style={{ width: `${item.width_permyriad / 100}%` }} /></span>
          <span className="mono value">{count(item.bucket.count)} <span className="muted">· {percentText(item.share_permyriad)}</span></span>
        </div>;
      })}
    </div>
    {slow ? <p className="audit-performance-note">{text(`${count(slow.count)} executions at or above ${thresholdText(slow.threshold_ms, text)}, ${percentText(slow.share_permyriad)}. The mark only highlights and filters; it does not judge a query.`, `≥ ${thresholdText(slow.threshold_ms, text)} 的执行 ${count(slow.count)} 次，占 ${percentText(slow.share_permyriad)}。阈值只用于标记，不判定查询好坏。`)}</p> : null}
    <p className="audit-performance-note muted">{text(`Exact server counts; ${exclusions.sql_null_count} with a NULL, ${exclusions.negative_count} with a negative and ${exclusions.overflow_count} with an out-of-range queryTime are left out (not counted as 0).`, `分档为服务端精确计数；queryTime 为空 ${exclusions.sql_null_count} 条、为负 ${exclusions.negative_count} 条、超范围 ${exclusions.overflow_count} 条，均未计入（不是 0）。`)}</p>
  </section>;
}

function thresholdText(milliseconds: string, text: LocalizedText): string {
  const value = Number(milliseconds);
  return value >= 60000 ? text(`${value / 60000} min`, `${value / 60000} 分钟`) : `${value / 1000} s`;
}

function FailureBreakdown({ response, text }: { response: AuditPerformanceResponse; text: LocalizedText }): JSX.Element {
  const errors = response.errors;
  return <section aria-labelledby="audit-performance-errors" className="audit-performance-panel">
    <div className="audit-performance-head">
      <h2 id="audit-performance-errors">{text("Failures", "失败拆分")}</h2>
      <span className="muted">{text(`by errorCode · ${count(errors.error_count)} in total`, `按 errorCode · 共 ${count(errors.error_count)} 次`)}</span>
    </div>
    {errors.state === "unavailable"
      ? <p className="audit-empty">{text("The audit table has no errorCode column; only the failure total is shown.", "审计表没有 errorCode 列，只显示失败总数。")}</p>
      : errors.rows.length === 0
      ? <p className="audit-empty">{text("No failed executions in this window.", "此窗口没有失败的执行。")}</p>
      : <table className="audit-performance-table">
        <thead><tr><th scope="col">errorCode</th><th className="num" scope="col">{text("Count", "次数")}</th><th className="num" scope="col">{text("Of failures", "占失败")}</th><th className="num" scope="col">{text("Latest", "最近一次")}</th></tr></thead>
        <tbody>
          {errors.rows.map((row, index) => <tr key={index}><td className={row.code.state === "text" && row.code.value !== "" ? "mono" : "mono muted"}>{valueText(row.code, text)}</td><td className="num">{count(row.count)}</td><td className="num">{percentText(row.share_permyriad)}</td><td className="num">{auditClockText(row.last_event_milliseconds).slice(5, 16)}</td></tr>)}
          {positiveCount(errors.remainder_count) ? <tr><td className="muted">{text(`${errors.remainder_code_count} more codes`, `其余 ${errors.remainder_code_count} 种`)}</td><td className="num">{count(errors.remainder_count)}</td><td /><td /></tr> : null}
        </tbody>
      </table>}
    <p className="audit-performance-note muted">{text("An empty errorCode is counted on its own and never merged into another code.", "errorCode 为空单独计数，不并入其他类别。")}</p>
  </section>;
}

function dimensionLabel(id: AuditPerformanceDimensionId, text: LocalizedText): string {
  return { user: text("User", "用户"), database: text("Database", "数据库"), resource_group: text("Resource group", "资源组"), catalog: "Catalog" }[id];
}

function rankLabel(key: AuditPerformanceRankKey, text: LocalizedText): string {
  return { query_time_sum: text("Cumulative time", "累计耗时"), execution_count: text("Executions", "次数"), error_count: text("Failures", "失败") }[key];
}

function WorkloadComposition({ filterableUser, onFilter, response, text }: { filterableUser: boolean; onFilter: AuditPerformanceOverviewProps["onFilter"]; response: AuditPerformanceResponse; text: LocalizedText }): JSX.Element {
  const [dimension, setDimension] = useState<AuditPerformanceDimensionId>("user");
  const [rankKey, setRankKey] = useState<AuditPerformanceRankKey>("query_time_sum");
  const entry = response.dimensions.find((item) => item.dimension === dimension) ?? null;
  const state = entry?.state ?? null;
  const ranking = state?.state === "available" ? state.rankings.find((item) => item.key === rankKey) ?? null : null;
  return <section aria-labelledby="audit-performance-dimensions" className="audit-performance-panel audit-performance-wide">
    <div className="audit-performance-head">
      <h2 id="audit-performance-dimensions">{text("Workload composition", "负载构成")}</h2>
      <div className="audit-performance-controls">
        <div aria-label={text("Dimension", "维度")} className="audit-performance-segmented" role="group">{DIMENSIONS.map((id) => <button aria-pressed={dimension === id} key={id} onClick={() => setDimension(id)} type="button">{dimensionLabel(id, text)}</button>)}</div>
        <div aria-label={text("Rank by", "排序")} className="audit-performance-segmented" role="group">{RANK_KEYS.map((key) => <button aria-pressed={rankKey === key} key={key} onClick={() => setRankKey(key)} type="button">{rankLabel(key, text)}</button>)}</div>
      </div>
    </div>
    {state?.state === "unavailable" || !state
      ? <p className="audit-empty">{text("The audit table has no user column, so this dimension is unavailable. It is never folded into “other”.", "审计表没有 user 列，此维度不可用，也不会归入“其他”。")}</p>
      : !ranking || ranking.entries.length === 0
      ? <p className="audit-empty">{text("Nothing to rank by this key in this window.", "此窗口按该指标没有可排名的值。")}</p>
      : <div className="audit-performance-table-wrap"><table className="audit-performance-table">
        <thead><tr><th scope="col">{dimensionLabel(dimension, text)}</th><th scope="col">{text(`${rankLabel(rankKey, text)} share`, `${rankLabel(rankKey, text)} 占比`)}</th><th className="num" scope="col">{text("Executions", "次数")}</th><th className="num" scope="col">{text("Cumulative time", "累计耗时")}</th><th className="num" scope="col">{text("Failures", "失败")}</th><th scope="col"><span className="visually-hidden">{text("Action", "操作")}</span></th></tr></thead>
        <tbody>
          {ranking.entries.map((item) => {
            const row = state.rows[item.row_index];
            if (!row) return null;
            const filterValue = row.value.state === "text" && row.value.value !== "" && (dimension !== "user" || filterableUser) ? row.value.value : null;
            return <tr key={item.row_index}>
              <td className={row.value.state === "text" ? "mono" : "mono muted"}>{valueText(row.value, text)}</td>
              <td><span className="audit-performance-share"><span className="track"><span className={rankKey === "error_count" ? "fill marked" : "fill"} style={{ width: `${item.width_permyriad / 100}%` }} /></span><span className="mono">{percentText(item.share_permyriad)}</span></span></td>
              <td className="num">{count(row.execution_count)}</td>
              <td className="num">{aggregateText(row.query_time_sum.aggregate)}</td>
              <td className="num">{count(row.error_count)}</td>
              <td className="action">{filterValue !== null ? <button className="button secondary" onClick={() => onFilter(dimension, filterValue)} type="button">{text("Filter", "筛选")}</button> : null}</td>
            </tr>;
          })}
          {ranking.remainder_value_count !== "0" ? <tr><td className="muted">{text(`${ranking.remainder_value_count} other values`, `其余 ${ranking.remainder_value_count} 个值`)}</td><td><span className="mono muted">{percentText(ranking.remainder_share_permyriad)}</span></td><td /><td /><td /><td /></tr> : null}
        </tbody>
      </table></div>}
    <p className="audit-performance-note muted">{text("Exact top values per key, ranked by the server over every value. Empty values and duplicate rows that disagree are kept as their own rows.", "每个排序指标都是服务端在全部取值上的精确 Top；为空与重复行不一致的记录单独成行。")}</p>
  </section>;
}

function familyText(family: AuditFamilyPresentation, text: LocalizedText): string {
  const sample = family.statement_sample;
  if (sample.state === "text") return sample.value;
  return family.membership.state === "exact_native_digest" ? `digest ${family.membership.native_digest}` : text("(no SQL sample)", "（无 SQL 样本）");
}

function TopFamilies({ families, label, onOpen, text }: { families: readonly AuditFamilyPresentation[]; label: string; onOpen(): void; text: LocalizedText }): JSX.Element {
  const rows = families.slice(0, 5);
  return <section aria-labelledby="audit-performance-top" className="audit-performance-panel">
    <div className="audit-performance-head">
      <h2 id="audit-performance-top">{text("Top repeated SQL", "同类 SQL 排行")}</h2>
      <button className="link-button" onClick={onOpen} type="button">{text("All repeated SQL →", "查看全部同类 SQL →")}</button>
    </div>
    {rows.length === 0
      ? <p className="audit-empty">{text("No exact query families in this window.", "此窗口没有精确查询族。")}</p>
      : <ol className="audit-performance-top">{rows.map((family, index) => {
        const time = family.aggregates.find((item) => item.metric.metric_id === "query_time" && item.aggregation === "sum") ?? null;
        return <li key={index}>
          <span className="mono sql" title={familyText(family, text)}>{familyText(family, text)}</span>
          <span className="mono">{time ? aggregateText(time) : "—"}</span>
          <span className="muted">{family.membership.state === "exact_native_digest" ? `${family.membership.context.database ?? "—"} · ` : ""}{count(family.execution_count ?? "—")} {text("runs", "次")}</span>
        </li>;
      })}</ol>}
    <p className="audit-performance-note muted">{text(`Ranked by the server: ${label}. Same source as the Repeated SQL view.`, `服务端排序：${label}，与“同类 SQL”视图同源。`)}</p>
  </section>;
}

function CoveragePanel({ coverage, response, text }: { coverage: AuditCoverage | null; response: AuditPerformanceResponse; text: LocalizedText }): JSX.Element {
  const metricNulls = coverage?.metric_inputs ?? [];
  return <details className="audit-performance-panel audit-performance-coverage">
    <summary>{text("Data coverage and limitations", "数据覆盖与限制")}{coverage ? <span className="muted"> · {text(`${count(coverage.raw_row_count)} raw rows → ${count(coverage.retained_event_count)} executions`, `原始 ${count(coverage.raw_row_count)} 行 → 保留 ${count(coverage.retained_event_count)} 次执行`)}</span> : null}</summary>
    {coverage ? <dl>
      <dt>{text("Exact duplicate rows (deduplicated)", "完全重复的行（已去重）")}</dt><dd>{count(coverage.exact_duplicate_row_count)}</dd>
      <dt>{text("Conflicting rows of one queryId (excluded)", "同一 queryId 内容冲突（已排除）")}</dt><dd>{count(coverage.conflicting_duplicate_row_count)}</dd>
      <dt>{text("Invalid queryId (excluded)", "queryId 无效（已排除）")}</dt><dd>{count(coverage.invalid_query_id_count)}</dd>
      <dt>{text("Missing digest (not in Repeated SQL)", "缺少 digest（不进同类 SQL）")}</dt><dd>{count(coverage.missing_digest_count)}</dd>
      {metricNulls.map((item) => <Fragment key={item.metric_id}><dt>{text(`${item.metric_id} is NULL`, `${item.metric_id} 为空`)}</dt><dd>{count(item.sql_null_count)}</dd></Fragment>)}
    </dl> : null}
    <LimitationNotes codes={response.limitations} text={text} />
  </details>;
}
