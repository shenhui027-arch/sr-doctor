// Design board 02-overview (总览 · 审计性能概览): 6-column KPI grid, bucketed
// trend chart + selected-slot detail card with fixed-footer drill-downs, and
// distribution / failures + repeated SQL / workload composition + summary cards.
// Every number and coordinate comes from the server; this file only lays them out.
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
import { AnimatedFigure, Button, Card, InfoTip, PageGrid, SegTabs } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import type { DoctorApi } from "../../lib/api";
import { countText } from "../../lib/duration-display";
import { LimitationNotes } from "../shell/LimitationNotes";
import { auditClockText, metricText, positiveCount } from "./audit-display";
import { AuditMetricValue } from "./AuditMetricValue";
import "./audit-performance.css";

type Metric = "count" | "latency" | "scan" | "cpu" | "memory" | "pending";
type DeckId = "trend" | "dist";
type DistTab = "hist" | "errors";
type RankTab = "families" | "composition";
type PerformanceState = { key: string; response: AuditPerformanceResponse | null; error: string | null };

const X0 = 56;
const PW = 888;
const YB = 212;
const PH = 196;
const BUCKETS: Array<{ id: string; choice: AuditPerformanceBucketChoice }> = [
  { id: "auto", choice: { state: "auto" } },
  ...[60, 300, 900, 3600, 21600, 86400].map((seconds) => ({
    id: String(seconds),
    choice: { state: "fixed", seconds } as AuditPerformanceBucketChoice,
  })),
];
const DIMENSIONS: AuditPerformanceDimensionId[] = ["user", "database", "resource_group", "catalog"];
const RANK_KEYS: AuditPerformanceRankKey[] = ["query_time_sum", "execution_count", "error_count"];

const METRIC_LABELS: Record<Metric, readonly [string, string]> = {
  count: ["Volume and failures", "查询量与失败"],
  latency: ["Duration percentiles", "耗时分位"],
  scan: ["Scanned bytes", "扫描量"],
  cpu: ["CPU", "CPU 累计"],
  memory: ["Peak memory", "内存峰值"],
  pending: ["Queue time", "排队时间"],
};

const DIMENSION_LABELS: Record<AuditPerformanceDimensionId, readonly [string, string]> = {
  user: ["User", "用户"],
  database: ["Database", "数据库"],
  resource_group: ["Resource group", "资源组"],
  catalog: ["Catalog", "Catalog"],
};

const RANK_LABELS: Record<AuditPerformanceRankKey, readonly [string, string]> = {
  query_time_sum: ["Cumulative time", "累计耗时"],
  execution_count: ["Executions", "次数"],
  error_count: ["Failures", "失败"],
};

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
  /** The load card (task 52), drawn in the summary column. */
  loadCard?: ReactNode;
  /** Overview page lays panels out in one-screen decks at >=1280px. */
  dashboard?: boolean;
  /** Alerts summary card drawn next to the load card on Overview. */
  extraCard?: ReactNode;
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
      .then((response) => {
        if (!controller.signal.aborted) setState({ key, response, error: null });
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setState({ key, response: null, error: describeFailure(cause) });
      });
    return () => controller.abort();
    // `key` encodes every input of the read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const current = state?.key === key ? state : null;
  const response = current?.response && current.response.connection_id === connectionId ? current.response : null;
  return (
    <section aria-busy={!current} aria-label={text("Performance overview", "性能概览")} className="audit-performance">
      {current?.error ? (
        <div className="notice-banner notice-banner--danger" role="alert">{current.error}</div>
      ) : !response ? (
        <div className="state-box" role="status">{text("Reading the audit window aggregation…", "正在聚合读取审计窗口…")}</div>
      ) : (
        <AuditPerformanceView {...props} bucket={bucket} onBucket={setBucket} response={response} />
      )}
    </section>
  );
}

export function AuditPerformanceView({
  bucket,
  onBucket,
  response,
  ...props
}: AuditPerformanceOverviewProps & { bucket: string; onBucket(id: string): void; response: AuditPerformanceResponse }): JSX.Element {
  const { text } = props;
  const lastFilled = useMemo(() => {
    let index = response.buckets.length - 1;
    while (index > 0 && response.buckets[index].summary.execution_count === "0") index -= 1;
    return Math.max(index, 0);
  }, [response]);
  const [selected, setSelected] = useState(lastFilled);
  const [deck, setDeck] = useState<DeckId>("trend");
  useEffect(() => setSelected(lastFilled), [lastFilled]);

  return (
    <div className="audit-performance-stage">
      <PerformanceCards concurrency={props.concurrency} response={response} text={text} />

      <div className="audit-perf-deck-bar">
        <SegTabs<DeckId>
          aria-label={text("Performance overview card group", "性能概览卡片分组")}
          items={[
            { value: "trend", label: text("Trend & selected slot", "趋势与所选时段") },
            { value: "dist", label: text("Distribution · ranking · summary", "分布 · 排行 · 摘要") },
          ]}
          onValueChange={setDeck}
          size="sm"
          value={deck}
        />
        <span className="card-subtitle mono" role="status">
          {slotClockText(String(response.scope.window.start_seconds * 1000))} – {slotClockText(String(response.scope.window.end_seconds * 1000))}
          {" · "}
          {text("One bounded server aggregation", "服务端一次有界聚合")}
        </span>
      </div>

      {/* Group A: bucketed trend chart (col-8) + selected-slot detail with fixed-footer drill-downs (col-4) */}
      <div hidden={deck !== "trend"}>
        <PageGrid className="audit-perf-deck">
          <PerformanceChart bucket={bucket} onBucket={onBucket} onSelect={setSelected} response={response} selected={selected} text={text} />
          <BucketAside
            bucket={response.buckets[selected] ?? null}
            count={response.buckets.length}
            index={selected}
            onDrill={props.onDrill}
            onSelect={setSelected}
            text={text}
          />
        </PageGrid>
      </div>

      {/* Group B: distribution / failures (col-5) + repeated SQL / composition (col-4) + summary cards (col-3) */}
      <div hidden={deck !== "dist"}>
        <PageGrid className="audit-perf-deck">
          <DistributionAndFailuresCard response={response} text={text} />
          <RankingAndCompositionCard
            families={props.families}
            familySortLabel={props.familySortLabel}
            filterableUser={props.filterableUser}
            onFilter={props.onFilter}
            onOpenFamilies={props.onOpenFamilies}
            response={response}
            text={text}
          />
          <div className="col-3 span-lg-3 span-md-8 span-sm-4 audit-summary-col">
            {props.extraCard}
            {props.loadCard}
            <CoveragePanel coverage={props.coverage} response={response} text={text} />
          </div>
        </PageGrid>
      </div>
    </div>
  );
}

/** Drops trailing `.000` milliseconds on exact second boundaries while keeping non-zero ms. */
export function slotClockText(milliseconds: string): string {
  return auditClockText(milliseconds).replace(/\.000$/, "");
}

function nonBreakingUnit(raw: string): string {
  return raw.replace(/(\d)\s+([A-Za-zμ%]+)/g, "$1\u00A0$2");
}

function percentText(permyriad: number | null | undefined): string {
  return permyriad === null || permyriad === undefined ? "—" : `${(permyriad / 100).toFixed(permyriad < 1000 ? 2 : 1)}%`;
}

function formatCount(value: string): string {
  return countText(value) ?? value;
}

function factText(fact: AuditMetricFact | null | undefined): string {
  return fact ? metricText(fact) : "—";
}

/** Binds each number and unit with `&nbsp;` + `.u-nowrap` so units never wrap alone onto the next line. */
export function quantileText(quantiles: AuditPerformanceQuantiles): JSX.Element {
  const plain = [quantiles.p50, quantiles.p95, quantiles.p99].map(factText).join(" / ");
  return (
    <span className="quantile-group" title={plain}>
      <MetricFigure fact={quantiles.p50} />
      <span className="kpi-sep">/</span>
      <MetricFigure fact={quantiles.p95} />
      <span className="kpi-sep">/</span>
      <MetricFigure fact={quantiles.p99} />
    </span>
  );
}

/** An exact fact renders as its number; any other state as its honest stamp, never raw "(missing)". */
function MetricFigure({ fact, animate = false }: { fact: AuditMetricFact | null | undefined; animate?: boolean }): JSX.Element {
  if (!fact || fact.state !== "exact") return <AuditMetricValue value={fact ?? null} />;
  const value = nonBreakingUnit(metricText(fact));
  return <span className="u-nowrap">{animate ? <AnimatedFigure value={value} /> : value}</span>;
}

function aggregateText(fact: AuditAggregateFact): string {
  return metricText(fact.metric);
}

function tickText(value: AuditPerformanceTickValue): string {
  return value.kind === "count" ? formatCount(value.count) : metricText(value.metric);
}

function valueText(value: AuditPerformanceValue, text: LocalizedText): string {
  if (value.state === "sql_null") return text("(empty)", "（为空）");
  if (value.state === "conflicting") return text("(duplicate rows disagree)", "（重复行不一致）");
  return value.value === "" ? text("(empty string)", "（空字符串）") : value.value;
}

const y = (permyriad: number): number => YB - (PH * permyriad) / 10000;
const x = (permyriad: number): number => X0 + (PW * permyriad) / 10000;

function spark(buckets: readonly AuditPerformanceBucket[], pick: (bucket: AuditPerformanceBucket) => number | null | undefined): string {
  return buckets
    .flatMap((bucket) => {
      const value = pick(bucket);
      if (value === null || value === undefined) return [];
      const at = (68 * (bucket.x_permyriad + bucket.width_permyriad / 2)) / 10000;
      return [`${at.toFixed(1)},${(20 - (18 * value) / 10000).toFixed(1)}`];
    })
    .join(" ");
}

function PerformanceCards({
  concurrency,
  response,
  text,
}: {
  concurrency: AuditConcurrencyPresentation | null;
  response: AuditPerformanceResponse;
  text: LocalizedText;
}): JSX.Element {
  const window = response.window;
  const nullTime = response.latency_histogram.exclusions.sql_null_count;
  const peak = concurrency?.series.state === "exact" ? concurrency.series.maximum_active_count : null;
  const hasErrors = window.error_count !== "0";

  const cards: Array<{
    key: string;
    label: string;
    infoLabel: string;
    info: string;
    value: ReactNode;
    valClass?: string;
    sparkLine?: string;
    sparkStroke?: string;
    gapFoot: string | null;
    gapWarn?: boolean;
  }> = [
    {
      key: "exec",
      label: text("Executions", "执行次数"),
      infoLabel: text("Executions definition", "执行次数口径"),
      info: text("Unique retained executions · deduplicated by queryId", "已按 queryId 去重后的唯一执行数"),
      value: <AnimatedFigure value={formatCount(window.execution_count)} />,
      sparkLine: spark(response.buckets, (item) => item.projection.execution_permyriad),
      sparkStroke: "var(--chart-1)",
      gapFoot: null,
    },
    {
      key: "err",
      label: text("Failed", "失败"),
      infoLabel: text("Failed count definition", "失败计数口径"),
      info: text("Exact state = ERR count", "state = ERR 的精确计数"),
      value: (
        <>
          <AnimatedFigure value={formatCount(window.error_count)} />
          <span className="kpi-unit">{percentText(window.error_share_permyriad)}</span>
        </>
      ),
      valClass: hasErrors ? "kpi-val kpi-val--critical" : "kpi-val",
      sparkLine: spark(response.buckets, (item) => item.projection.error_permyriad),
      sparkStroke: hasErrors ? "var(--critical)" : "var(--chart-1)",
      gapFoot: null,
    },
    {
      key: "lat",
      label: text("Duration P50 / P95 / P99", "耗时 P50 / P95 / P99"),
      infoLabel: text("Percentile definition", "分位数计算口径"),
      info: text(
        `Exact whole-window percentiles (${formatCount(window.query_time.eligible_count)} durations); NULL or negative queryTime excluded, never counted as 0.`,
        `全窗口精确分位（${formatCount(window.query_time.eligible_count)} 个耗时）；queryTime 为 NULL / 负值的记录排除，不按 0 计。`,
      ),
      value: quantileText(window.query_time),
      valClass: "kpi-val kpi-val--quantiles",
      sparkLine: spark(response.buckets, (item) => item.projection.query_time_p95_permyriad),
      sparkStroke: "var(--heat-high)",
      gapFoot: positiveCount(nullTime)
        ? text(`${nullTime} records with a NULL queryTime are left out`, `${nullTime} 条为空未计`)
        : null,
      gapWarn: true,
    },
    {
      key: "cpu",
      label: text("Cumulative CPU", "CPU 累计"),
      infoLabel: text("Cumulative CPU definition", "CPU 累计口径"),
      info: text("Summed cpuCostNs across cores, not wall-clock elapsed time", "审计 cpuCostNs 多核累加时间，非墙钟经过时间"),
      value: <MetricFigure animate fact={window.cpu_cost_sum.aggregate.metric} />,
      sparkLine: spark(response.buckets, (item) => item.projection.cpu_cost_permyriad),
      sparkStroke: "var(--chart-1)",
      gapFoot: positiveCount(window.cpu_cost_sum.excluded_count)
        ? text(`${window.cpu_cost_sum.excluded_count} records lack cpuCostNs`, `${window.cpu_cost_sum.excluded_count} 条缺少 cpuCostNs`)
        : null,
      gapWarn: true,
    },
    {
      key: "scan",
      label: text("Cumulative scanned bytes", "扫描量累计"),
      infoLabel: text("Scanned bytes definition", "扫描量累计口径"),
      info: text("Summed scanBytes across retained executions", "保留执行的 scanBytes 求和"),
      value: <MetricFigure animate fact={window.scan_bytes_sum.aggregate.metric} />,
      sparkLine: spark(response.buckets, (item) => item.projection.scan_bytes_permyriad),
      sparkStroke: "var(--chart-1)",
      gapFoot: positiveCount(window.scan_bytes_sum.excluded_count)
        ? text(`${window.scan_bytes_sum.excluded_count} records lack scanBytes, not counted`, `${window.scan_bytes_sum.excluded_count} 条为空未计`)
        : null,
      gapWarn: true,
    },
    {
      key: "peak",
      label: text("Peak in flight", "执行中峰值"),
      infoLabel: text("Peak in flight definition", "执行中峰值口径"),
      info: text("Estimated from retained queryTime intervals", "按 queryTime 区间推算"),
      value: peak ? (
        <AnimatedFigure value={formatCount(peak)} />
      ) : (
        <span className="state-stamp" data-honest-state="missing">∅ {text("Missing · not 0", "缺失 · 非 0")}</span>
      ),
      gapFoot: peak ? null : text("not available for this window", "此窗口不可用"),
    },
  ];

  return (
    <section aria-label={text("Window summary", "窗口汇总指标")} className="kpi-grid kpi-grid--6">
      {cards.map((card) => (
        <article className="card card--kpi" key={card.key}>
          <div className="kpi-head">
            <span>{card.label}</span>
            <InfoTip label={card.infoLabel}>{card.info}</InfoTip>
          </div>
          <div className="kpi-main">
            <span className={card.valClass ?? "kpi-val"}>{card.value}</span>
            {card.sparkLine ? (
              <svg aria-hidden="true" className="kpi-spark" preserveAspectRatio="none" viewBox="0 0 68 22">
                <polyline fill="none" points={card.sparkLine} stroke={card.sparkStroke} strokeWidth="1.6" />
              </svg>
            ) : null}
          </div>
          {card.gapFoot ? (
            <div className={card.gapWarn ? "kpi-foot kpi-foot--warn" : "kpi-foot"} title={card.gapFoot}>
              {card.gapFoot}
            </div>
          ) : null}
        </article>
      ))}
    </section>
  );
}

function metricLabel(metric: Metric, text: LocalizedText): string {
  return lookupEnumLabel(METRIC_LABELS, metric, text);
}

function axisFor(metric: Metric, response: AuditPerformanceResponse): AuditPerformanceValueTick[] {
  const axes = response.axes;
  switch (metric) {
    case "count":
      return axes.execution;
    case "latency":
      return axes.query_time_log;
    case "scan":
      return axes.scan_bytes;
    case "cpu":
      return axes.cpu_cost;
    case "memory":
      return axes.memory_cost;
    case "pending":
    default:
      return axes.pending_time;
  }
}

function PerformanceChart({
  bucket,
  onBucket,
  onSelect,
  response,
  selected,
  text,
}: {
  bucket: string;
  onBucket(id: string): void;
  onSelect(index: number): void;
  response: AuditPerformanceResponse;
  selected: number;
  text: LocalizedText;
}): JSX.Element {
  const [metric, setMetric] = useState<Metric>("count");
  const pendingAvailable = response.window.pending_time.state === "available";
  const buckets = response.buckets;
  const selectedBucket = buckets[selected];

  const bars = (pick: (item: AuditPerformanceBucket) => number | null, excluded: (item: AuditPerformanceBucket) => string) =>
    buckets.map((item, index) => {
      const left = x(item.x_permyriad) + 1;
      const width = Math.max((PW * item.width_permyriad) / 10000 - 2, 1);
      const value = pick(item);
      if (value === null) {
        return item.summary.execution_count === "0" ? null : (
          <rect className="gap" height={PH - 8} key={index} width={width} x={left} y={YB - PH + 8} />
        );
      }
      return (
        <rect
          className={positiveCount(excluded(item)) ? "bar partial" : "bar"}
          height={(PH * value) / 10000}
          key={index}
          width={width}
          x={left}
          y={y(value)}
        />
      );
    });

  const line = (pick: (item: AuditPerformanceBucket) => number | null, className: string) => {
    const segments: string[][] = [[]];
    for (const item of buckets) {
      const value = pick(item);
      if (value === null) {
        if (segments[segments.length - 1].length) segments.push([]);
        continue;
      }
      segments[segments.length - 1].push(`${(x(item.x_permyriad) + (PW * item.width_permyriad) / 20000).toFixed(1)},${y(value).toFixed(1)}`);
    }
    return segments
      .filter((points) => points.length)
      .map((points, index) =>
        points.length === 1 ? (
          <circle className={className} cx={points[0].split(",")[0]} cy={points[0].split(",")[1]} key={`${className}-${index}`} r={2.5} />
        ) : (
          <polyline className={className} key={`${className}-${index}`} points={points.join(" ")} />
        ),
      );
  };

  const marks = metric === "count"
    ? buckets.map((item, index) => {
        const left = x(item.x_permyriad) + 1;
        const width = Math.max((PW * item.width_permyriad) / 10000 - 2, 1);
        const total = (PH * item.projection.execution_permyriad) / 10000;
        const failed = (PH * item.projection.error_permyriad) / 10000;
        return (
          <g key={index}>
            <rect className="ok" height={Math.max(total - failed, 0)} width={width} x={left} y={YB - total + failed} />
            <rect className="failed" height={failed} width={width} x={left} y={YB - total} />
          </g>
        );
      })
    : metric === "latency"
      ? [
          ...line((item) => item.projection.query_time_p99_permyriad, "p99"),
          ...line((item) => item.projection.query_time_p95_permyriad, "p95"),
          ...line((item) => item.projection.query_time_p50_permyriad, "p50"),
        ]
      : metric === "scan"
        ? bars((item) => item.projection.scan_bytes_permyriad, (item) => item.summary.scan_bytes_sum.excluded_count)
        : metric === "cpu"
          ? bars((item) => item.projection.cpu_cost_permyriad, (item) => item.summary.cpu_cost_sum.excluded_count)
          : metric === "memory"
            ? line((item) => item.projection.memory_cost_permyriad, "line")
            : line((item) => item.projection.pending_time_p95_permyriad, "line");

  const legend: Array<[string, string]> = metric === "count"
    ? [
        ["ok", text("Succeeded", "成功执行")],
        ["failed", text("Failed (state = ERR)", "失败 (ERR)")],
      ]
    : metric === "latency"
      ? [
          ["p99", "P99"],
          ["p95", "P95"],
          ["p50", "P50"],
        ]
      : metric === "scan" || metric === "cpu"
        ? [
            ["bar", metricLabel(metric, text)],
            ["partial", text("Some records lack the value", "部分记录缺少该值")],
            ["gap", text("No value · not drawn", "无可用值 · 不画")],
          ]
        : [["line", metric === "memory" ? text("Largest single-execution memory", "段内单查询内存最大值") : "pendingTimeMs P95"]];

  const note = metric === "latency"
    ? text("Exact per bucket, log axis; buckets are never combined", "每段内精确分位，对数轴；段间不合并")
    : metric === "cpu"
      ? text("Summed CPU, not elapsed time", "CPU 求和，不是墙钟时间")
      : metric === "memory"
        ? text("Not concurrent total memory", "不是并发总内存")
        : metric === "pending"
          ? text("Only where the audit table has pendingTimeMs", "仅审计表有 pendingTimeMs 时可用")
          : text("Exact server counts", "服务端精确计数");

  const metricItems = (["count", "latency", "scan", "cpu", "memory", "pending"] as Metric[]).map((item) => ({
    value: item,
    label: metricLabel(item, text),
    disabled: item === "pending" && !pendingAvailable,
  }));

  return (
    <Card
      actions={
        <>
          <SegTabs<Metric>
            aria-label={text("Metric", "主图指标")}
            items={metricItems}
            onValueChange={setMetric}
            size="sm"
            value={metric}
          />
          <div className="filter-chip-input">
            <label htmlFor="audit-perf-bucket-select">{text("Bucket", "粒度")}</label>
            <select id="audit-perf-bucket-select" onChange={(event) => onBucket(event.target.value)} value={bucket}>
              {BUCKETS.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.id === "auto"
                    ? text(`Auto · ${bucketText(response.bucket_seconds, text)}`, `自动 · ${bucketText(response.bucket_seconds, text)}`)
                    : bucketText(Number(item.id), text)}
                </option>
              ))}
            </select>
          </div>
        </>
      }
      bodyClassName="audit-chart-body"
      footer={
        <>
          <div className="audit-chart-legend">
            {legend.map(([kind, label]) => (
              <span key={kind}><i aria-hidden="true" className={`swatch ${kind}`} />{label}</span>
            ))}
            {buckets.some((item) => item.recent_ingestion) ? (
              <span><i aria-hidden="true" className="swatch recent" />{text("May still be arriving", "可能尚未全部落库")}</span>
            ) : null}
          </div>
          <span>{note}</span>
        </>
      }
      id="audit-performance-chart"
      span={{ lg: 8, md: 8, sm: 4 }}
      title={`${metricLabel(metric, text)} · ${text(`every ${bucketText(response.bucket_seconds, text)}`, `每 ${bucketText(response.bucket_seconds, text)}`)}`}
      variant="chart"
    >
      <svg
        aria-label={`${metricLabel(metric, text)} · ${buckets.length} ${text("buckets", "段")}`}
        className={`audit-performance-plot metric-${metric}`}
        role="img"
        viewBox="0 0 960 250"
      >
        <defs>
          <pattern height="8" id="audit-performance-hatch" patternTransform="rotate(45)" patternUnits="userSpaceOnUse" width="8">
            <rect fill="#fff7ed" height="8" width="8" />
            <line stroke="#fdba74" strokeWidth="2" x1="0" x2="0" y1="0" y2="8" />
          </pattern>
        </defs>
        {selectedBucket ? (
          <rect
            className="selected"
            height={PH + 4}
            rx={4}
            width={Math.max((PW * selectedBucket.width_permyriad) / 10000, 2)}
            x={x(selectedBucket.x_permyriad)}
            y={YB - PH - 4}
          />
        ) : null}
        {axisFor(metric, response).map((tick) => (
          <g className="tick" key={tick.y_permyriad}>
            <line x1={X0} x2={X0 + PW} y1={y(tick.y_permyriad)} y2={y(tick.y_permyriad)} />
            <text textAnchor="end" x={X0 - 6} y={y(tick.y_permyriad) + 4}>{tickText(tick.value)}</text>
          </g>
        ))}
        {buckets.map((item, index) =>
          item.recent_ingestion ? (
            <rect
              className="recent"
              height={PH}
              key={`recent-${index}`}
              width={Math.max((PW * item.width_permyriad) / 10000 - 1, 1)}
              x={x(item.x_permyriad) + 0.5}
              y={YB - PH}
            />
          ) : null,
        )}
        {marks}
        <line className="axis" x1={X0} x2={X0 + PW} y1={YB} y2={YB} />
        {response.axes.time_ticks.map((tick) => (
          <text
            className="time"
            key={tick.at_milliseconds}
            textAnchor={tick.x_permyriad === 0 ? "start" : tick.x_permyriad === 10000 ? "end" : "middle"}
            x={x(tick.x_permyriad)}
            y={YB + 20}
          >
            {tick.label}
          </text>
        ))}
        {buckets.map((item, index) => (
          <rect
            className="hit"
            height={PH + 4}
            key={`hit-${index}`}
            onClick={() => onSelect(index)}
            width={Math.max((PW * item.width_permyriad) / 10000, 2)}
            x={x(item.x_permyriad)}
            y={YB - PH - 4}
          >
            <title>{slotClockText(item.start_milliseconds)}</title>
          </rect>
        ))}
      </svg>
    </Card>
  );
}

function bucketText(seconds: number, text: LocalizedText): string {
  if (seconds % 86400 === 0) return text(`${seconds / 86400} d`, `${seconds / 86400} 天`);
  if (seconds % 3600 === 0) return text(`${seconds / 3600} h`, `${seconds / 3600} 小时`);
  return text(`${seconds / 60} min`, `${seconds / 60} 分钟`);
}

function BucketAside({
  bucket,
  count: total,
  index,
  onDrill,
  onSelect,
  text,
}: {
  bucket: AuditPerformanceBucket | null;
  count: number;
  index: number;
  onDrill: AuditPerformanceOverviewProps["onDrill"];
  onSelect(index: number): void;
  text: LocalizedText;
}): JSX.Element {
  if (!bucket) {
    return (
      <Card span={{ lg: 4, md: 8, sm: 4 }} title={text("Selected slot", "所选时段")} variant="detail">
        <div className="state-box"><span>{text("No bucket in this window.", "当前窗口无时段数据。")}</span></div>
      </Card>
    );
  }

  const summary = bucket.summary;
  const startLabel = slotClockText(bucket.start_milliseconds);
  const endFull = slotClockText(bucket.end_milliseconds);
  const endLabel = endFull.length > 11 && startLabel.slice(0, 10) === endFull.slice(0, 10) ? endFull.slice(11) : endFull;
  const scanExcluded = summary.scan_bytes_sum.excluded_count;

  return (
    <Card
      actions={
        <div className="audit-bucket-steps">
          <Button
            aria-label={text("Previous bucket", "上一时段")}
            disabled={index === 0}
            onClick={() => onSelect(index - 1)}
            size="sm"
            variant="secondary"
          >
            ‹ {text("Previous", "上一段")}
          </Button>
          <Button
            aria-label={text("Next bucket", "下一时段")}
            disabled={index >= total - 1}
            onClick={() => onSelect(index + 1)}
            size="sm"
            variant="secondary"
          >
            {text("Next", "下一段")} ›
          </Button>
        </div>
      }
      footer={
        <>
          <Button
            disabled={summary.execution_count === "0"}
            onClick={() => onDrill(bucket.start_milliseconds, bucket.end_milliseconds, "occurrences")}
            size="sm"
            variant="primary"
          >
            {text("Executions in this bucket", "查看单次执行")}
          </Button>
          <Button
            disabled={summary.execution_count === "0"}
            onClick={() => onDrill(bucket.start_milliseconds, bucket.end_milliseconds, "families")}
            size="sm"
            variant="secondary"
          >
            {text("Repeated SQL in this bucket", "同类 SQL")}
          </Button>
        </>
      }
      id="audit-performance-bucket"
      span={{ lg: 4, md: 8, sm: 4 }}
      subtitle={text(`Selected slot (${index + 1} / ${total})`, `所选时段 (${index + 1} / ${total})`)}
      title={<span className="mono">{startLabel} – {endLabel}</span>}
      variant="detail"
    >
      <div className="audit-bucket-body">
        <dl className="kv-grid">
          <dt>{text("Executions", "执行次数")}</dt>
          <dd className="mono"><AnimatedFigure value={formatCount(summary.execution_count)} /></dd>

          <dt>{text("Failed", "失败")}</dt>
          <dd className={summary.error_count !== "0" ? "mono delta-up" : "mono"}>
            <AnimatedFigure value={`${formatCount(summary.error_count)} · ${percentText(summary.error_share_permyriad)}`} />
          </dd>

          <dt>{text("Duration P50 / P95 / P99", "耗时 P50 / P95 / P99")}</dt>
          <dd className="mono">{quantileText(summary.query_time)}</dd>

          <dt>{text("Scanned", "扫描量累计")}</dt>
          <dd className="mono">
            <MetricFigure fact={summary.scan_bytes_sum.aggregate.metric} />
            {positiveCount(scanExcluded) ? (
              <>
                {" "}
                <span className="state-stamp state-stamp--warn" data-honest-state="partial">
                  ◐ {text(`${scanExcluded} empty`, `${scanExcluded} 条为空未计`)}
                </span>
              </>
            ) : null}
          </dd>

          <dt>{text("CPU", "CPU 累计")}</dt>
          <dd className="mono"><MetricFigure fact={summary.cpu_cost_sum.aggregate.metric} /></dd>

          <dt>{text("Peak memory", "单查询内存峰值")}</dt>
          <dd className="mono"><MetricFigure fact={summary.memory_cost_max.aggregate.metric} /></dd>

          <dt>{text("Queue P95", "排队时间 P95")}</dt>
          <dd className="mono">
            {summary.pending_time.state === "available" ? (
              <MetricFigure fact={summary.pending_time.quantiles.p95} />
            ) : (
              <span
                className="state-stamp"
                data-honest-state="missing"
                title={text("The audit table has no pendingTimeMs", "审计表未包含 pendingTimeMs 列")}
              >
                ∅ {text("Missing · not 0", "缺失 · 非 0")}
              </span>
            )}
          </dd>
        </dl>

        {bucket.recent_ingestion ? (
          <div className="notice-banner notice-banner--warn" role="note">
            <span>{text("AuditLoader writes in batches; this bucket may still be filling.", "AuditLoader 批量写入有延迟，这一段可能尚未全部落库。")}</span>
          </div>
        ) : null}
      </div>
    </Card>
  );
}

function thresholdText(milliseconds: string, text: LocalizedText): string {
  const value = Number(milliseconds);
  return value >= 60000 ? text(`${value / 60000}\u00A0min`, `${value / 60000}\u00A0分钟`) : `${value / 1000}\u00A0s`;
}

function DistributionAndFailuresCard({ response, text }: { response: AuditPerformanceResponse; text: LocalizedText }): JSX.Element {
  const [tab, setTab] = useState<DistTab>("hist");
  const histogram = response.latency_histogram;
  const [threshold, setThreshold] = useState(0);
  const slow = histogram.slow[threshold] ?? null;
  const exclusions = histogram.exclusions;
  const errors = response.errors;

  const hasExclusions = positiveCount(exclusions.sql_null_count) || positiveCount(exclusions.negative_count) || positiveCount(exclusions.overflow_count);

  return (
    <Card
      actions={
        <SegTabs<DistTab>
          aria-label={text("Distribution or failures", "分布与失败切换")}
          items={[
            { value: "hist", label: text("Duration distribution", "耗时分布") },
            { value: "errors", label: text("Failures", "失败拆分"), count: formatCount(errors.error_count) },
          ]}
          onValueChange={setTab}
          size="sm"
          value={tab}
        />
      }
      bodyClassName={tab === "errors" ? "card-bd--flush" : undefined}
      footer={
        tab === "hist" ? (
          hasExclusions ? (
            <span>
              {text(
                `NULL ${exclusions.sql_null_count} · negative ${exclusions.negative_count} · overflow ${exclusions.overflow_count} excluded (not 0)`,
                `queryTime 为空 ${exclusions.sql_null_count} 条、负值 ${exclusions.negative_count} 条、超范围 ${exclusions.overflow_count} 条，均未计入（非 0）`,
              )}
            </span>
          ) : (
            <span>{text("Exact server histogram · invalid queryTime never counted as 0", "服务端精确分档 · 异常耗时绝不按 0 计")}</span>
          )
        ) : (
          <span>{text("An empty errorCode is counted on its own and never merged.", "errorCode 为空单独计数，不并入其他类别。")}</span>
        )
      }
      span={{ lg: 5, md: 8, sm: 4 }}
      title={text("Distribution & failures", "分布与失败原因")}
      variant="default"
    >
      <div hidden={tab !== "hist"}>
        <div className="audit-dist-toolbar">
          {slow ? (
            <span className="card-subtitle">
              {text(
                `Mark ≥ ${thresholdText(slow.threshold_ms, text)}: ${formatCount(slow.count)} (${percentText(slow.share_permyriad)}, highlight only)`,
                `标记阈值 ≥ ${thresholdText(slow.threshold_ms, text)}：共 ${formatCount(slow.count)} 次 · ${percentText(slow.share_permyriad)}（仅标记不判定好坏）`,
              )}
            </span>
          ) : null}
          {histogram.slow.length > 1 ? (
            <div className="filter-chip-input">
              <label htmlFor="audit-hist-threshold">{text("Mark", "标记")}</label>
              <select id="audit-hist-threshold" onChange={(event) => setThreshold(Number(event.target.value))} value={threshold}>
                {histogram.slow.map((item, idx) => (
                  <option key={item.threshold_ms} value={idx}>≥ {thresholdText(item.threshold_ms, text)}</option>
                ))}
              </select>
            </div>
          ) : null}
        </div>
        <div className="audit-histogram-list">
          {histogram.buckets.map((item) => {
            const marked = slow !== null && BigInt(item.bucket.lower_ms_inclusive) >= BigInt(slow.threshold_ms);
            return (
              <div className="audit-histogram-row" key={item.bucket.bucket_id}>
                <span className="mono label">{item.bucket.display_label_seconds}</span>
                <span className="share-track">
                  <span
                    className={marked ? "share-fill share-fill--high" : "share-fill share-fill--accent"}
                    style={{ width: `${item.width_permyriad / 100}%` }}
                  />
                </span>
                <span className="mono value">
                  {formatCount(item.bucket.count)} · {percentText(item.share_permyriad)}
                </span>
              </div>
            );
          })}
        </div>
      </div>

      <div hidden={tab !== "errors"}>
        {errors.state === "unavailable" ? (
          <div className="state-box">
            <span>{text("The audit table has no errorCode column; only the failure total is shown.", "审计表没有 errorCode 列，只显示失败总数。")}</span>
          </div>
        ) : errors.rows.length === 0 ? (
          <div className="state-box">
            <span>{text("No failed executions in this window.", "此窗口没有失败的执行。")}</span>
          </div>
        ) : (
          <table aria-label={text("Failures by errorCode", "失败 errorCode 拆分")} className="data-table">
            <thead>
              <tr>
                <th scope="col">errorCode</th>
                <th className="num" scope="col">{text("Count", "次数")}</th>
                <th className="num" scope="col">{text("Of failures", "占失败")}</th>
                <th className="num" scope="col">{text("Latest", "最近一次")}</th>
              </tr>
            </thead>
            <tbody>
              {errors.rows.map((row, index) => (
                <tr key={index}>
                  <td className="mono">{valueText(row.code, text)}</td>
                  <td className="num">{formatCount(row.count)}</td>
                  <td className="num">{percentText(row.share_permyriad)}</td>
                  <td className="num mono">{slotClockText(row.last_event_milliseconds).slice(5, 16)}</td>
                </tr>
              ))}
              {positiveCount(errors.remainder_count) ? (
                <tr>
                  <td>{text(`${errors.remainder_code_count} more codes`, `其余 ${errors.remainder_code_count} 种`)}</td>
                  <td className="num">{formatCount(errors.remainder_count)}</td>
                  <td />
                  <td />
                </tr>
              ) : null}
            </tbody>
          </table>
        )}
      </div>
    </Card>
  );
}

function dimensionLabel(id: AuditPerformanceDimensionId, text: LocalizedText): string {
  return lookupEnumLabel(DIMENSION_LABELS, id, text);
}

function rankLabel(key: AuditPerformanceRankKey, text: LocalizedText): string {
  return lookupEnumLabel(RANK_LABELS, key, text);
}

function familyText(family: AuditFamilyPresentation, text: LocalizedText): string {
  const sample = family.statement_sample;
  if (sample.state === "text") return sample.value;
  return family.membership.state === "exact_native_digest" ? `digest ${family.membership.native_digest}` : text("(no SQL sample)", "（无 SQL 样本）");
}

function RankingAndCompositionCard({
  families,
  familySortLabel,
  filterableUser,
  onFilter,
  onOpenFamilies,
  response,
  text,
}: {
  families: readonly AuditFamilyPresentation[];
  familySortLabel: string;
  filterableUser: boolean;
  onFilter: AuditPerformanceOverviewProps["onFilter"];
  onOpenFamilies(): void;
  response: AuditPerformanceResponse;
  text: LocalizedText;
}): JSX.Element {
  const [tab, setTab] = useState<RankTab>("families");
  const [dimension, setDimension] = useState<AuditPerformanceDimensionId>("user");
  const [rankKey, setRankKey] = useState<AuditPerformanceRankKey>("query_time_sum");
  const topRows = families.slice(0, 5);
  const entry = response.dimensions.find((item) => item.dimension === dimension) ?? null;
  const dimState = entry?.state ?? null;
  const ranking = dimState?.state === "available" ? dimState.rankings.find((item) => item.key === rankKey) ?? null : null;

  return (
    <Card
      actions={
        <SegTabs<RankTab>
          aria-label={text("Ranking or workload composition", "排行与构成切换")}
          items={[
            { value: "families", label: text("Top repeated SQL", "同类 SQL") },
            { value: "composition", label: text("Workload composition", "负载构成") },
          ]}
          onValueChange={setTab}
          size="sm"
          value={tab}
        />
      }
      footer={
        <>
          <span>
            {tab === "families"
              ? text(`Server sort: ${familySortLabel}`, `服务端排序：${familySortLabel}`)
              : text("Empty and conflicting rows kept separate", "为空与重复行不一致记录单独成行")}
          </span>
          <Button onClick={onOpenFamilies} size="sm" variant="ghost">
            {text("All repeated SQL →", "全部同类 SQL →")}
          </Button>
        </>
      }
      span={{ lg: 4, md: 8, sm: 4 }}
      title={text("Ranking & composition", "排行与构成")}
      variant="table"
    >
      <div hidden={tab !== "families"}>
        {topRows.length === 0 ? (
          <div className="state-box">
            <span>{text("No exact query families in this window.", "此窗口没有精确查询族。")}</span>
          </div>
        ) : (
          <table aria-label={text("Top repeated SQL", "同类 SQL 排行 Top 5")} className="data-table">
            <thead>
              <tr>
                <th scope="col">{text("SQL sample", "SQL 样本")}</th>
                <th className="num" scope="col">{text("Runs", "次数")}</th>
                <th className="num" scope="col">{text("Cumulative time", "累计耗时")}</th>
              </tr>
            </thead>
            <tbody>
              {topRows.map((family, index) => {
                const time = family.aggregates.find((item) => item.metric.metric_id === "query_time" && item.aggregation === "sum") ?? null;
                const sample = familyText(family, text);
                return (
                  <tr key={index}>
                    <td className="mono audit-sql-cell" title={sample}>{sample}</td>
                    <td className="num">{formatCount(family.execution_count ?? "—")}</td>
                    <td className="num"><span className="u-nowrap">{time ? <MetricFigure fact={time.metric} /> : "—"}</span></td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      <div hidden={tab !== "composition"}>
        <div className="audit-composition-toolbar">
          <SegTabs<AuditPerformanceDimensionId>
            aria-label={text("Dimension", "维度")}
            items={DIMENSIONS.map((id) => ({ value: id, label: dimensionLabel(id, text) }))}
            onValueChange={setDimension}
            size="sm"
            value={dimension}
          />
          <SegTabs<AuditPerformanceRankKey>
            aria-label={text("Rank by", "排序")}
            items={RANK_KEYS.map((key) => ({ value: key, label: rankLabel(key, text) }))}
            onValueChange={setRankKey}
            size="sm"
            value={rankKey}
          />
        </div>
        {dimState?.state === "unavailable" || !dimState ? (
          <div className="state-box">
            <span>
              {text(
                "The audit table lacks this column, so this dimension is unavailable. It is never folded into “other”.",
                "审计表没有该列，此维度不可用，也不会归入“其他”。",
              )}
            </span>
          </div>
        ) : !ranking || ranking.entries.length === 0 ? (
          <div className="state-box">
            <span>{text("Nothing to rank by this key in this window.", "此窗口按该指标没有可排名的值。")}</span>
          </div>
        ) : (
          <table aria-label={text("Workload composition", "负载构成表")} className="data-table">
            <thead>
              <tr>
                <th scope="col">{dimensionLabel(dimension, text)}</th>
                <th scope="col">{text(`${rankLabel(rankKey, text)} share`, `${rankLabel(rankKey, text)} 占比`)}</th>
                <th className="num" scope="col">{text("Executions", "次数")}</th>
                <th className="num" scope="col">{text("Cumulative time", "累计耗时")}</th>
                <th className="num" scope="col">{text("Failures", "失败")}</th>
                <th scope="col"><span className="sr-only">{text("Action", "操作")}</span></th>
              </tr>
            </thead>
            <tbody>
              {ranking.entries.map((item) => {
                const row = dimState.rows[item.row_index];
                if (!row) return null;
                const filterValue =
                  row.value.state === "text" && row.value.value !== "" && (dimension !== "user" || filterableUser)
                    ? row.value.value
                    : null;
                return (
                  <tr key={item.row_index}>
                    <td className="mono">{valueText(row.value, text)}</td>
                    <td>
                      <span className="share-cell">
                        <span className="share-track">
                          <span
                            className={rankKey === "error_count" ? "share-fill share-fill--high" : "share-fill share-fill--accent"}
                            style={{ width: `${item.width_permyriad / 100}%` }}
                          />
                        </span>
                        <span className="mono">{percentText(item.share_permyriad)}</span>
                      </span>
                    </td>
                    <td className="num">{formatCount(row.execution_count)}</td>
                    <td className="num"><MetricFigure fact={row.query_time_sum.aggregate.metric} /></td>
                    <td className="num">{formatCount(row.error_count)}</td>
                    <td className="num">
                      {filterValue !== null ? (
                        <Button onClick={() => onFilter(dimension, filterValue)} size="sm" variant="secondary">
                          {text("Filter", "筛选")}
                        </Button>
                      ) : null}
                    </td>
                  </tr>
                );
              })}
              {ranking.remainder_value_count !== "0" ? (
                <tr>
                  <td>{text(`${ranking.remainder_value_count} other values`, `其余 ${ranking.remainder_value_count} 个值`)}</td>
                  <td><span className="mono">{percentText(ranking.remainder_share_permyriad)}</span></td>
                  <td />
                  <td />
                  <td />
                  <td />
                </tr>
              ) : null}
            </tbody>
          </table>
        )}
      </div>
    </Card>
  );
}

function CoveragePanel({
  coverage,
  response,
  text,
}: {
  coverage: AuditCoverage | null;
  response: AuditPerformanceResponse;
  text: LocalizedText;
}): JSX.Element {
  const metricNulls = coverage?.metric_inputs ?? [];
  return (
    <details className="audit-coverage-card">
      <summary>
        <span>{text("Data coverage and limitations", "数据覆盖与限制")}</span>
        {coverage ? (
          <span className="card-subtitle">
            · {text(`${formatCount(coverage.raw_row_count)} raw rows → ${formatCount(coverage.retained_event_count)} executions`, `原始 ${formatCount(coverage.raw_row_count)} 行 → 保留 ${formatCount(coverage.retained_event_count)} 次执行`)}
          </span>
        ) : null}
      </summary>
      {coverage ? (
        <dl className="kv-grid audit-coverage-dl">
          <dt>{text("Exact duplicate rows (deduplicated)", "完全重复的行（已去重）")}</dt>
          <dd className="mono">{formatCount(coverage.exact_duplicate_row_count)}</dd>
          <dt>{text("Conflicting rows of one queryId (excluded)", "同一 queryId 内容冲突（已排除）")}</dt>
          <dd className="mono">{formatCount(coverage.conflicting_duplicate_row_count)}</dd>
          <dt>{text("Invalid queryId (excluded)", "queryId 无效（已排除）")}</dt>
          <dd className="mono">{formatCount(coverage.invalid_query_id_count)}</dd>
          <dt>{text("Missing digest (not in Repeated SQL)", "缺少 digest（不进同类 SQL）")}</dt>
          <dd className="mono">{formatCount(coverage.missing_digest_count)}</dd>
          {metricNulls.map((item) => (
            <Fragment key={item.metric_id}>
              <dt>{text(`${item.metric_id} is NULL`, `${item.metric_id} 为空`)}</dt>
              <dd className="mono">{formatCount(item.sql_null_count)}</dd>
            </Fragment>
          ))}
        </dl>
      ) : null}
      <LimitationNotes codes={response.limitations} text={text} />
    </details>
  );
}
