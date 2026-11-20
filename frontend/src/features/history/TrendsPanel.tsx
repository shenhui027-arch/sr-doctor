// Design board 02-overview (30 天模板趋势): the server's verdicts, one template's
// hourly medians against its baseline band, and representative executions.
// Every number is a DTO field; the chart only maps them to pixels.
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  HistorySeriesResponse,
  EvidenceCompleteness,
  HistoryTrendsResponse,
  TrendChange,
  TrendConfidence,
  TrendNotComparableReason,
  TrendPoint,
  TrendRow,
  TrendSeverity,
  TrendUnscoredReason,
  TrendVerdict,
} from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Button, Card, EvidenceMeter, InfoTip, PageGrid, type EvidenceLevel } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { metricText } from "../audit/audit-display";
import { clockText } from "./CollectionPanel";

export const BASELINE_CHOICES = [168, 336, 720] as const;
export const CURRENT_CHOICES = [6, 24, 72] as const;
/** Representative executions shown before "show all". */
export const EXEMPLARS_SHOWN = 3;

const NOT_COMPARABLE_REASON_LABELS: Record<TrendNotComparableReason, readonly [string, string]> = {
  engine_version_changed: ["StarRocks version changed · not comparable yet", "版本变化，暂不可比"],
  rule_version_changed: ["Rule set changed · not comparable yet", "规则集变化，暂不可比"],
};

const UNSCORED_REASON_LABELS: Record<TrendUnscoredReason, readonly [string, string]> = {
  insufficient_baseline: ["Too few baseline samples", "基线样本不足"],
  insufficient_current: ["Too few current samples", "当前样本不足"],
  zero_baseline: ["Baseline is 0 · absolute change only", "基线为 0，只给绝对变化"],
  workload_signal_missing: ["No scan or CPU signal · not judged", "缺少扫描量或 CPU 信号，暂不判定"],
};

const VERDICT_FALLBACK_LABELS: Record<string, readonly [string, string]> = {
  regression: ["↑ Regression", "↑ 回归"],
  improvement: ["↓ Improvement", "↓ 改善"],
  stable: ["Within baseline band", "在基线带内"],
};

const TREND_SEVERITY_LABELS: Record<TrendSeverity, readonly [string, string]> = {
  high: ["Severity high", "严重度 高"],
  medium: ["Severity medium", "严重度 中"],
  low: ["Severity low", "严重度 低"],
};

const TREND_CONFIDENCE_LABELS: Record<TrendConfidence, readonly [string, string]> = {
  high: ["Confidence high", "置信度 高"],
  medium: ["Confidence medium", "置信度 中"],
  low: ["Confidence low", "置信度 低"],
};

const TREND_EVIDENCE_LABELS: Record<EvidenceCompleteness, readonly [string, string]> = {
  complete: ["Evidence complete", "证据完整"],
  partial: ["Evidence partial", "证据部分"],
  unavailable: ["Evidence unavailable", "证据不可用"],
};

// `TrendRow.kind` is an open string; known kinds get a label, others show as-is.
const TREND_SOURCE_LABELS: Record<"official_digest" | "product_fingerprint", readonly [string, string]> = {
  official_digest: ["Official digest", "官方 digest"],
  product_fingerprint: ["Unofficial fingerprint", "非官方指纹"],
};

export function verdictText(verdict: TrendVerdict, text: LocalizedText): string {
  switch (verdict.state) {
    case "regression":
      return text("↑ Regression", "↑ 回归");
    case "improvement":
      return text("↓ Improvement", "↓ 改善");
    case "stable":
      return text("Within baseline band", "在基线带内");
    case "not_comparable":
      return lookupEnumLabel(NOT_COMPARABLE_REASON_LABELS, verdict.reason, text);
    case "unscored":
      return lookupEnumLabel(UNSCORED_REASON_LABELS, verdict.reason, text);
    default:
      return lookupEnumLabel(VERDICT_FALLBACK_LABELS, String((verdict as { state?: unknown })?.state ?? ""), text);
  }
}

/** The server's typed state, kept for the tooltip. */
function verdictState(verdict: TrendVerdict): string {
  if (verdict.state === "unscored" || verdict.state === "not_comparable") {
    return `${verdict.state} · ${verdict.reason}`;
  }
  return String((verdict as { state?: unknown })?.state ?? "unknown");
}

function renderVerdictBadge(verdict: TrendVerdict, text: LocalizedText): JSX.Element {
  const label = verdictText(verdict, text);
  const tip = verdictState(verdict);
  switch (verdict.state) {
    case "regression":
      return <span className="delta-up mono" title={tip}>{label}</span>;
    case "improvement":
      return <span className="delta-down mono" title={tip}>{label}</span>;
    case "not_comparable":
      return <span className="state-stamp state-stamp--warn" title={tip}>≠ {label}</span>;
    case "unscored":
      return <span className="state-stamp" title={tip}>— {label}</span>;
    default:
      return <span className="state-stamp" title={tip}>{label}</span>;
  }
}

function nonBreakingUnit(raw: string): string {
  return raw.replace(/(\d)\s+([A-Za-zμ%]+)/g, "$1\u00A0$2");
}

export function changeText(row: TrendRow): string {
  return trendChangeText(row.change);
}

export function trendChangeText(change: TrendChange | null): string {
  if (!change) return "—";
  const sign = change.direction === "up" ? "+" : change.direction === "down" ? "−" : "±";
  const relative = change.relative_percent === null
    ? ""
    : ` · ${change.relative_percent.startsWith("-") ? "−" + change.relative_percent.slice(1) : "+" + change.relative_percent}%`;
  return `${sign}${metricText(change.magnitude)}${relative}`;
}

export function severityText(row: TrendRow, text: LocalizedText): string {
  if (row.verdict.state !== "regression") return text("Severity —", "严重度 —");
  return lookupEnumLabel(TREND_SEVERITY_LABELS, row.verdict.severity, text);
}

export function confidenceText(row: TrendRow, text: LocalizedText): string {
  if (!row.confidence) return text("Confidence —", "置信度 —");
  return lookupEnumLabel(TREND_CONFIDENCE_LABELS, row.confidence, text);
}

export function evidenceText(row: TrendRow, text: LocalizedText): string {
  return lookupEnumLabel(TREND_EVIDENCE_LABELS, row.evidence, text);
}

export function sourceText(row: TrendRow, text: LocalizedText): string {
  return lookupEnumLabel(TREND_SOURCE_LABELS, row.kind, text);
}

function mapEvidenceLevel(evidence: EvidenceCompleteness): EvidenceLevel {
  switch (evidence) {
    case "complete":
      return "complete";
    case "partial":
      return "partial";
    case "unavailable":
    default:
      return "missing";
  }
}

const LIMITATION_TEXT: Record<string, [string, string]> = {
  quantiles_are_log_histogram_approximations: ["Medians are histogram approximations (≤ 6.25%)", "中位数为直方图近似（误差 ≤ 6.25%）"],
  current_window_spans_a_version_change: ["The current window spans a version change; only the newest version is used", "当前窗口跨越版本变化，只用最新版本"],
  older_version_buckets_excluded_from_baseline: ["Baseline hours under another version are excluded", "基线中其他版本的小时已排除"],
  current_window_not_fully_collected: ["Part of the current window was not fully collected", "当前窗口有未完整采集的小时"],
  some_executions_lack_a_valid_query_time: ["Some executions have no valid query time (not counted as 0)", "部分执行没有有效耗时（不按 0 计）"],
  baseline_waits_for_samples_under_the_current_version: ["Waiting for baseline samples under the current version", "待基线窗口积累当前版本的样本"],
  scan_bytes_and_cpu_unavailable: ["Scan bytes and CPU are unavailable", "扫描量与 CPU 不可用"],
  workload_stable_latency_rose_possible_contention: ["Workload unchanged while latency rose: possibly contention", "工作量未变而耗时上升：可能是资源争用"],
};

export function limitationText(key: string, text: LocalizedText): string {
  const pair = Object.prototype.hasOwnProperty.call(LIMITATION_TEXT, key) ? LIMITATION_TEXT[key] : undefined;
  return pair ? text(...pair) : key;
}

function nanos(point: TrendPoint): number | null {
  const value = point.median?.canonical_number?.value;
  return value && /^\d+$/.test(value) ? Number(value) : null;
}

/** Hourly medians with the baseline band; gaps break the line, never bridged. */
export function TrendChart({ series, text }: { series: HistorySeriesResponse; text: LocalizedText }): JSX.Element {
  const width = 960;
  const height = 210;
  const left = 40;
  const right = 12;
  const top = 14;
  const bottom = 26;
  const points = series.points;
  const base = series.row.baseline;
  const baseValue = base.median?.canonical_number?.value ? Number(base.median.canonical_number.value) : null;
  const madValue = base.mad?.canonical_number?.value ? Number(base.mad.canonical_number.value) : 0;
  const values = points.map(nanos).filter((value): value is number => value !== null);
  const ceiling = Math.max(1, ...values, baseValue !== null ? baseValue + madValue * 2 : 0) * 1.15;
  const step = (width - left - right) / Math.max(1, points.length - 1);
  const x = (index: number) => left + index * step;
  const y = (value: number) => top + (height - top - bottom) * (1 - value / ceiling);

  const segments: string[] = [];
  let current: string[] = [];
  points.forEach((point, index) => {
    const value = nanos(point);
    if (value === null || !point.comparable || point.coverage === "not_collected") {
      if (current.length) segments.push(current.join(" "));
      current = [];
      return;
    }
    current.push(`${x(index).toFixed(1)},${y(value).toFixed(1)}`);
  });
  if (current.length) segments.push(current.join(" "));

  return (
    <svg
      aria-label={text("Hourly median query time", "每小时查询耗时中位数")}
      className="trend-chart"
      role="img"
      viewBox={`0 0 ${width} ${height}`}
    >
      {points.map((point, index) =>
        point.coverage === "complete" ? null : (
          <rect
            className={`trend-gap ${point.coverage}`}
            height={height - top - bottom}
            key={`gap-${point.hour_start_seconds}`}
            width={Math.max(step, 2)}
            x={x(index) - step / 2}
            y={top}
          />
        ),
      )}
      {baseValue !== null ? (
        <>
          <rect
            className="trend-band"
            height={Math.max(2, y(Math.max(0, baseValue - madValue)) - y(baseValue + madValue))}
            width={width - left - right}
            x={left}
            y={y(baseValue + madValue)}
          />
          <line className="trend-base" x1={left} x2={width - right} y1={y(baseValue)} y2={y(baseValue)} />
          <text className="trend-axis" x={left + 4} y={y(baseValue) - 4}>{metricText(base.median)}</text>
        </>
      ) : null}
      {segments.map((segment) => (segment.includes(" ") ? <polyline className="trend-line" key={segment} points={segment} /> : null))}
      {points.map((point, index) => {
        const value = nanos(point);
        if (value === null || point.coverage === "not_collected") return null;
        return (
          <circle
            className={point.thin ? "trend-dot thin" : point.comparable ? "trend-dot" : "trend-dot other"}
            cx={x(index)}
            cy={y(value)}
            key={`dot-${point.hour_start_seconds}`}
            r={point.thin ? 3.5 : 2.5}
          >
            <title>{`${clockText(point.hour_start_seconds)} · ${metricText(point.median)} · ${point.valid_samples}`}</title>
          </circle>
        );
      })}
      {points.map((point, index) =>
        index % 6 === 0 ? (
          <text className="trend-axis" key={`t-${point.hour_start_seconds}`} x={x(index)} y={height - 8}>
            {clockText(point.hour_start_seconds).slice(5, 13)}
          </text>
        ) : null,
      )}
    </svg>
  );
}

export function TrendsPanel({
  trends,
  series,
  selectedKey,
  onSelect,
  baselineHours,
  currentHours,
  onWindow,
  onPullProfile,
  onAddToQueue,
  toolbar,
  text,
}: {
  trends: HistoryTrendsResponse;
  series: HistorySeriesResponse | null;
  selectedKey: string | null;
  onSelect(key: string): void;
  baselineHours: number;
  currentHours: number;
  onWindow(baselineHours: number, currentHours: number): void;
  onPullProfile?: (queryId: string) => void;
  onAddToQueue?: (row: TrendRow) => void;
  toolbar?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const [allExemplars, setAllExemplars] = useState(false);
  const counts = trends.counts;
  const row = series?.row ?? null;
  const exemplars = series ? (allExemplars ? series.exemplars : series.exemplars.slice(0, EXEMPLARS_SHOWN)) : [];

  return (
    <div className="trends-stage">
      <div className="workspace-bar trends-filter-bar">
        <div className="filter-chip-input">
          <label htmlFor="trends-baseline-window">{text("Baseline window", "基线窗口")}</label>
          <select
            id="trends-baseline-window"
            onChange={(event) => onWindow(Number(event.target.value), currentHours)}
            value={baselineHours}
          >
            {BASELINE_CHOICES.map((hours) => (
              <option key={hours} value={hours}>{text(`Previous ${hours / 24} days`, `前 ${hours / 24} 天`)}</option>
            ))}
          </select>
        </div>

        <div className="filter-chip-input">
          <label htmlFor="trends-current-window">{text("Current window", "当前窗口")}</label>
          <select
            id="trends-current-window"
            onChange={(event) => onWindow(baselineHours, Number(event.target.value))}
            value={currentHours}
          >
            {CURRENT_CHOICES.map((hours) => (
              <option key={hours} value={hours}>{text(`Last ${hours} hours`, `最近 ${hours} 小时`)}</option>
            ))}
          </select>
        </div>

        <div className="workspace-bar-right">
          <span className="badge-stamp">{text("Metric: Median query time · MAD band", "指标：查询耗时中位数 · MAD 基线带")}</span>
          {toolbar}
        </div>
      </div>

      <PageGrid className="trends-grid">
        {/* Left Card (col-7): SQL template trend list */}
        <Card
          actions={
            <span className="card-subtitle">
              {text(
                `Regression ${counts.regression} · Improvement ${counts.improvement} · Not comparable ${counts.not_comparable} · Unscored ${counts.unscored}`,
                `回归 ${counts.regression} · 改善 ${counts.improvement} · 暂不可比 ${counts.not_comparable} · 样本不足 ${counts.unscored}`,
              )}
            </span>
          }
          footer={
            <span>
              {text(
                `${trends.rows.length} templates listed · ${counts.stable} within their baseline band · ${counts.unscored} with too few samples · ${counts.not_comparable} not comparable`,
                `列出 ${trends.rows.length} 个模板 · ${counts.stable} 个在基线带内 · ${counts.unscored} 个样本不足 · ${counts.not_comparable} 个暂不可比`,
              )}
              {trends.truncated ? text(" · list truncated", " · 列表已截断") : ""}
            </span>
          }
          id="trend-list"
          span={{ lg: 7, md: 8, sm: 4 }}
          title={text("SQL template trends", "SQL 模板趋势")}
          titleExtra={
            <InfoTip label={text("Regression judgement rules", "回归判定边界说明")}>
              {text(
                "Never judged by latency alone; compares hourly medians against the baseline MAD band alongside scan/CPU workload.",
                "不以耗时单独判定好坏；结合 MAD 基线带与扫描量/CPU 工作量对照判定。",
              )}
            </InfoTip>
          }
          variant="table"
        >
          {trends.rows.length === 0 ? (
            <div className="state-box">
              <strong>{text("No template outside its baseline band", "当前窗口没有超出基线带或需要说明的模板")}</strong>
            </div>
          ) : (
            <table aria-label={text("SQL template trend list", "SQL 模板回归趋势列表")} className="data-table trend-table">
              <thead>
                <tr>
                  <th scope="col">{text("Template", "脱敏模板与指纹来源")}</th>
                  <th scope="col">{text("Verdict", "判定结论")}</th>
                  <th className="num" scope="col">{text("Baseline → current", "基线 → 当前")}</th>
                  <th className="num" scope="col">{text("Change", "变化")}</th>
                  <th className="num" scope="col">{text("Samples", "样本 基线 / 当前")}</th>
                  <th scope="col">{text("Severity · confidence · evidence", "严重度 · 置信度 · 证据")}</th>
                </tr>
              </thead>
              <tbody>
                {trends.rows.map((item) => {
                  const isSelected = item.template_key === selectedKey;
                  const evLabel = evidenceText(item, text);
                  return (
                    <tr className={isSelected ? "is-selected" : undefined} key={item.template_key} onClick={() => onSelect(item.template_key)}>
                      <td className="trend-template-cell">
                        <button
                          aria-pressed={isSelected}
                          className="row-pick-btn mono trend-pick-btn"
                          onClick={() => onSelect(item.template_key)}
                          type="button"
                        >
                          {item.template ?? text("Template unavailable", "模板不可用")}
                        </button>
                        <div className="trend-stamp-row">
                          {item.kind === "official_digest" ? (
                            <BadgeStamp kind="official_digest" />
                          ) : item.kind === "product_fingerprint" ? (
                            <BadgeStamp kind="unofficial_fingerprint" />
                          ) : (
                            <span className="badge-stamp">{sourceText(item, text)}</span>
                          )}
                        </div>
                      </td>
                      <td>{renderVerdictBadge(item.verdict, text)}</td>
                      <td className="num">
                        <span className="u-nowrap">{nonBreakingUnit(metricText(item.baseline.median))}</span>
                        {" → "}
                        <span className="u-nowrap">{nonBreakingUnit(metricText(item.current.median))}</span>
                      </td>
                      <td className={item.change?.direction === "up" ? "num delta-up" : item.change?.direction === "down" ? "num delta-down" : "num"}>
                        <span className="u-nowrap">{nonBreakingUnit(changeText(item))}</span>
                      </td>
                      <td className="num">{item.baseline.valid_samples} / {item.current.valid_samples}</td>
                      <td>
                        <div className="trend-evidence-cell">
                          <span className="status-inline">
                            {item.verdict.state === "regression" ? (
                              <span
                                aria-hidden="true"
                                className={item.verdict.severity === "high" ? "status-dot status-dot--critical" : "status-dot status-dot--warn"}
                              />
                            ) : null}
                            <span>{severityText(item, text)}</span>
                          </span>
                          <span className="card-subtitle">{confidenceText(item, text)}</span>
                          <EvidenceMeter label={evLabel} level={mapEvidenceLevel(item.evidence)} />
                          <span className="sr-only">{evLabel}</span>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </Card>

        {/* Right Card (col-5): Hourly median curve, verdict & representative executions */}
        {row && series ? (
          <Card
            actions={<span className="badge-stamp">{text(`Last ${series.points.length} hours (UTC)`, `最近 ${series.points.length} 小时 (UTC)`)}</span>}
            footer={
              <>
                <span>
                  {text(
                    `StarRocks ${row.comparability.engine_version} · fingerprint rule ${row.comparability.rule_version}`,
                    `StarRocks ${row.comparability.engine_version} · 指纹规则 ${row.comparability.rule_version}`,
                  )}
                </span>
                {onAddToQueue ? (
                  <Button onClick={() => onAddToQueue(row)} size="sm" variant="primary">
                    {text("Add to governance queue", "加入治理队列")}
                  </Button>
                ) : null}
              </>
            }
            id="trend-detail"
            span={{ lg: 5, md: 8, sm: 4 }}
            title={text("Hourly median query time", "每小时耗时中位数与代表执行")}
            variant="detail"
          >
            <div className="trend-detail-stack">
              <pre className="sql-well">{row.template ?? text("Template unavailable", "模板不可用")}</pre>

              <TrendChart series={series} text={text} />

              <div className="collection-legend">
                <span><i aria-hidden="true" className="trend-swatch trend-swatch--band" />{text("Baseline median ± MAD", "基线中位数 ± MAD")}</span>
                <span><i aria-hidden="true" className="trend-swatch trend-swatch--line" />{text("Hourly median", "每小时中位数")}</span>
                <span><i aria-hidden="true" className="trend-swatch trend-swatch--thin" />{text("Thin hour (not judged)", "样本不足（不判定）")}</span>
                <span><i aria-hidden="true" className="cov-swatch cov-cell cov-cell--partial" />{text("Partially collected", "部分采集")}</span>
                <span><i aria-hidden="true" className="cov-swatch cov-cell cov-cell--gap" />{text("Not collected · line breaks", "未采集 · 曲线断开")}</span>
              </div>

              <div className="trend-verdict-row">
                {renderVerdictBadge(row.verdict, text)}
                {!series.rule.calibrated ? (
                  <span className="state-stamp state-stamp--warn">{text("Rule on trial", "规则试运行中")}</span>
                ) : null}
              </div>

              <dl className="kv-grid">
                <dt>{text("Baseline", "基线统计")}</dt>
                <dd className="mono">
                  {row.baseline.median
                    ? text(
                        `median ${nonBreakingUnit(metricText(row.baseline.median))} · ${row.baseline.executions} runs · ${row.baseline.hours} h`,
                        `中位数 ${nonBreakingUnit(metricText(row.baseline.median))} · ${row.baseline.executions} 次 · ${row.baseline.hours} 小时`,
                      )
                    : text(`no baseline · ${row.baseline.thin_hours} thin hours`, `无基线 · ${row.baseline.thin_hours} 个样本不足的小时`)}
                </dd>
                <dt>{text("Current", "当前窗口")}</dt>
                <dd className="mono">
                  {text(
                    `median ${nonBreakingUnit(metricText(row.current.median))} · ${row.current.valid_samples} / ${row.current.executions} valid`,
                    `中位数 ${nonBreakingUnit(metricText(row.current.median))} · 有效 ${row.current.valid_samples} / ${row.current.executions} 次`,
                  )}
                </dd>
                <dt>{text("Change", "变化")}</dt>
                <dd className="mono">{nonBreakingUnit(changeText(row))}</dd>
                {row.limitations.length > 0 ? (
                  <>
                    <dt>{text("Limitations", "限制")}</dt>
                    <dd>{row.limitations.map((key) => limitationText(key, text)).join("；")}</dd>
                  </>
                ) : null}
              </dl>

              <div className="trend-exemplar-block">
                <span className="field-label">{text("Representative executions · slowest first", "代表慢执行")}</span>
                {series.exemplars.length === 0 ? (
                  <p className="card-subtitle">{text("No execution kept for this window.", "当前窗口没有保留的执行。")}</p>
                ) : (
                  <ul className="trend-exemplars">
                    {exemplars.map((exemplar) => (
                      <li key={exemplar.query_id}>
                        <code>{exemplar.query_id}</code>
                        <span className="mono u-nowrap">{nonBreakingUnit(metricText(exemplar.query_time))}</span>
                        <span className="card-subtitle mono">{clockText(exemplar.event_seconds)}</span>
                        {onPullProfile ? (
                          <Button onClick={() => onPullProfile(exemplar.query_id)} size="sm" variant="secondary">
                            {text("Pull Profile and diagnose", "拉取 Profile 并诊断")}
                          </Button>
                        ) : (
                          <span className="card-subtitle">{text("Connect to this cluster to pull", "连接该集群后可拉取")}</span>
                        )}
                      </li>
                    ))}
                  </ul>
                )}
                {series.exemplars.length > EXEMPLARS_SHOWN ? (
                  <Button onClick={() => setAllExemplars((value) => !value)} size="sm" variant="ghost">
                    {allExemplars ? text("Show fewer", "收起") : text(`Show all ${series.exemplars.length}`, `显示全部 ${series.exemplars.length} 条`)}
                  </Button>
                ) : null}
              </div>

              <details className="disclosure history-details">
                <summary>{text("How this is judged", "查看判定规则")}</summary>
                <dl className="kv-grid collection-disclosure-kv">
                  <dt>{text("Rule", "规则")}</dt>
                  <dd>
                    {text(
                      `outside baseline median ± ${series.rule.k_mad}·1.4826·MAD and ≥ ${series.rule.min_relative_percent}%`,
                      `超出基线中位数 ± ${series.rule.k_mad}·1.4826·MAD 且变化 ≥ ${series.rule.min_relative_percent}%`,
                    )}
                  </dd>
                  <dt>{text("Samples", "样本条件")}</dt>
                  <dd>
                    {text(
                      `≥ ${series.rule.min_baseline_hours} baseline hours with ≥ ${series.rule.min_hour_samples} runs each; ≥ ${series.rule.min_current_samples} current samples`,
                      `基线 ≥ ${series.rule.min_baseline_hours} 个小时、每小时 ≥ ${series.rule.min_hour_samples} 次；当前 ≥ ${series.rule.min_current_samples} 个样本`,
                    )}
                  </dd>
                  <dt>{text("Baseline spread", "基线离散")}</dt>
                  <dd className="mono">MAD {nonBreakingUnit(metricText(row.baseline.mad))}</dd>
                  <dt>{text("Calibration", "校准")}</dt>
                  <dd>
                    {series.rule.calibrated
                      ? text("Calibrated on production samples", "已用生产样本校准")
                      : text("Thresholds not yet calibrated on production workload; read verdicts as hints.", "阈值尚未用真实业务分布校准，判定结果仅供参考。")}
                  </dd>
                </dl>
              </details>
            </div>
          </Card>
        ) : (
          <Card
            id="trend-detail"
            span={{ lg: 5, md: 8, sm: 4 }}
            title={text("Hourly median query time", "每小时耗时中位数与代表执行")}
            variant="detail"
          >
            <div className="state-box">
              <strong>{text("Select a template", "选择左侧一条 SQL 模板")}</strong>
              <span>{text("Hourly medians, baseline band and representative executions appear here.", "在此查看每小时耗时中位数、基线带与代表慢执行。")}</span>
            </div>
          </Card>
        )}
      </PageGrid>
    </div>
  );
}
