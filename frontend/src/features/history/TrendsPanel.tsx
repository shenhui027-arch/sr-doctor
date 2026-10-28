// Design board C (模板趋势与回归): the server's verdicts, one template's
// hourly medians against its baseline band, and representative executions.
// Every number is a DTO field; the chart only maps them to pixels.
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type { HistorySeriesResponse, HistoryTrendsResponse, TrendChange, TrendPoint, TrendRow, TrendVerdict } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { metricText } from "../audit/audit-display";
import { clockText } from "./CollectionPanel";

export const BASELINE_CHOICES = [168, 336, 720] as const;
export const CURRENT_CHOICES = [6, 24, 72] as const;
/** Representative executions shown before "show all". */
export const EXEMPLARS_SHOWN = 3;

export function verdictText(verdict: TrendVerdict, text: LocalizedText): string {
  switch (verdict.state) {
    case "regression": return text("↑ Regression", "↑ 回归");
    case "improvement": return text("↓ Improvement", "↓ 改善");
    case "stable": return text("Within baseline band", "在基线带内");
    case "not_comparable": return verdict.reason === "engine_version_changed"
      ? text("StarRocks version changed · not comparable yet", "版本变化，暂不可比")
      : text("Rule set changed · not comparable yet", "规则集变化，暂不可比");
    case "unscored": return {
      insufficient_baseline: text("Too few baseline samples", "基线样本不足"),
      insufficient_current: text("Too few current samples", "当前样本不足"),
      zero_baseline: text("Baseline is 0 · absolute change only", "基线为 0，只给绝对变化"),
      workload_signal_missing: text("No scan or CPU signal · not judged", "缺少扫描量或 CPU 信号，暂不判定"),
    }[verdict.reason];
  }
}

/** The server's typed state, kept for the tooltip. */
function verdictState(verdict: TrendVerdict): string {
  return verdict.state === "unscored" || verdict.state === "not_comparable" ? `${verdict.state} · ${verdict.reason}` : verdict.state;
}

function verdictTone(verdict: TrendVerdict): string {
  return verdict.state === "regression" ? "up" : verdict.state === "improvement" ? "down" : "dash";
}

export function changeText(row: TrendRow): string {
  return trendChangeText(row.change);
}

export function trendChangeText(change: TrendChange | null): string {
  if (!change) return "—";
  const sign = change.direction === "up" ? "+" : change.direction === "down" ? "−" : "±";
  const relative = change.relative_percent === null ? "" : ` · ${change.relative_percent.startsWith("-") ? "−" + change.relative_percent.slice(1) : "+" + change.relative_percent}%`;
  return `${sign}${metricText(change.magnitude)}${relative}`;
}

function severityText(row: TrendRow, text: LocalizedText): string {
  if (row.verdict.state !== "regression") return text("Severity —", "严重度 —");
  return { high: text("Severity high", "严重度 高"), medium: text("Severity medium", "严重度 中"), low: text("Severity low", "严重度 低") }[row.verdict.severity];
}

function confidenceText(row: TrendRow, text: LocalizedText): string {
  if (!row.confidence) return text("Confidence —", "置信度 —");
  return { high: text("Confidence high", "置信度 高"), medium: text("Confidence medium", "置信度 中"), low: text("Confidence low", "置信度 低") }[row.confidence];
}

function evidenceText(row: TrendRow, text: LocalizedText): string {
  return { complete: text("Evidence complete", "证据完整"), partial: text("Evidence partial", "证据部分"), unavailable: text("Evidence unavailable", "证据不可用") }[row.evidence];
}

function sourceText(row: TrendRow, text: LocalizedText): string {
  return row.kind === "official_digest" ? text("Official digest", "官方 digest") : text("Unofficial fingerprint", "非官方指纹");
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
  const pair = LIMITATION_TEXT[key];
  return pair ? text(...pair) : key;
}

function nanos(point: TrendPoint): number | null {
  const value = point.median?.canonical_number?.value;
  return value && /^\d+$/.test(value) ? Number(value) : null;
}

/** Hourly medians with the baseline band; gaps break the line, never bridged. */
export function TrendChart({ series, text }: { series: HistorySeriesResponse; text: LocalizedText }): JSX.Element {
  const width = 960, height = 230, left = 40, right = 10, top = 14, bottom = 26;
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
  return <svg aria-label={text("Hourly median query time", "每小时查询耗时中位数")} className="trend-chart" role="img" viewBox={`0 0 ${width} ${height}`}>
    {points.map((point, index) => point.coverage === "complete" ? null : <rect className={`trend-gap ${point.coverage}`} height={height - top - bottom} key={`gap-${point.hour_start_seconds}`} width={Math.max(step, 2)} x={x(index) - step / 2} y={top} />)}
    {baseValue !== null && <>
      <rect className="trend-band" height={Math.max(2, y(Math.max(0, baseValue - madValue)) - y(baseValue + madValue))} width={width - left - right} x={left} y={y(baseValue + madValue)} />
      <line className="trend-base" x1={left} x2={width - right} y1={y(baseValue)} y2={y(baseValue)} />
      <text className="trend-axis" x={left + 4} y={y(baseValue) - 4}>{metricText(base.median)}</text>
    </>}
    {segments.map((segment) => segment.includes(" ") ? <polyline className="trend-line" key={segment} points={segment} /> : null)}
    {points.map((point, index) => {
      const value = nanos(point);
      if (value === null || point.coverage === "not_collected") return null;
      return <circle className={point.thin ? "trend-dot thin" : point.comparable ? "trend-dot" : "trend-dot other"} cx={x(index)} cy={y(value)} key={`dot-${point.hour_start_seconds}`} r={point.thin ? 3.5 : 2.5}><title>{`${clockText(point.hour_start_seconds)} · ${metricText(point.median)} · ${point.valid_samples}`}</title></circle>;
    })}
    {points.map((point, index) => index % 6 === 0 ? <text className="trend-axis" key={`t-${point.hour_start_seconds}`} x={x(index)} y={height - 8}>{clockText(point.hour_start_seconds).slice(5, 13)}</text> : null)}
  </svg>;
}

export function TrendsPanel({ trends, series, selectedKey, onSelect, baselineHours, currentHours, onWindow, onPullProfile, onAddToQueue, toolbar, text }: {
  trends: HistoryTrendsResponse;
  series: HistorySeriesResponse | null;
  selectedKey: string | null;
  onSelect(key: string): void;
  baselineHours: number;
  currentHours: number;
  onWindow(baselineHours: number, currentHours: number): void;
  /** Present only while a session to the same target is connected. */
  onPullProfile?: (queryId: string) => void;
  /** Opens a governance item for the template. */
  onAddToQueue?: (row: TrendRow) => void;
  /** Target and refresh controls, shown at the end of the filter bar. */
  toolbar?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const [allExemplars, setAllExemplars] = useState(false);
  const counts = trends.counts;
  const row = series?.row ?? null;
  const exemplars = series ? (allExemplars ? series.exemplars : series.exemplars.slice(0, EXEMPLARS_SHOWN)) : [];
  // List first, then the selected template: verdict and key evidence on top,
  // the rule and its calibration one click away.
  return <div className="history-page">
    <div className="trend-filters">
      <label>{text("Baseline window", "基线窗口")}<select onChange={(event) => onWindow(Number(event.target.value), currentHours)} value={baselineHours}>{BASELINE_CHOICES.map((hours) => <option key={hours} value={hours}>{text(`Previous ${hours / 24} days`, `前 ${hours / 24} 天`)}</option>)}</select></label>
      <label>{text("Current window", "当前窗口")}<select onChange={(event) => onWindow(baselineHours, Number(event.target.value))} value={currentHours}>{CURRENT_CHOICES.map((hours) => <option key={hours} value={hours}>{text(`Last ${hours} hours`, `最近 ${hours} 小时`)}</option>)}</select></label>
      <span className="trend-filter-fixed">{text("Metric", "指标")}<strong>{text("Median query time", "查询耗时 中位数")}</strong></span>
      {toolbar && <span className="trend-toolbar">{toolbar}</span>}
    </div>
    <section aria-labelledby="trend-list" className="history-card">
      <div className="history-card-head">
        <h2 id="trend-list">{text("Regression list", "回归列表")}</h2>
        <span className="history-muted">{text(`${trends.rows.length} templates listed · ${counts.stable} within their baseline band · ${counts.unscored} with too few samples · ${counts.not_comparable} not comparable`, `列出 ${trends.rows.length} 个模板 · 另有 ${counts.stable} 个在基线带内 · ${counts.unscored} 个样本不足 · ${counts.not_comparable} 个暂不可比`)}{trends.truncated ? text(" · list truncated", " · 列表已截断") : ""}</span>
      </div>
      {trends.rows.length === 0 ? <p className="history-muted">{text("No template has a verdict outside its baseline band in this window.", "当前窗口没有超出基线带或需要说明的模板。")}</p> : <div className="history-table-wrap trend-list-scroll"><table className="history-table trend-table">
        <thead><tr>
          <th scope="col">{text("Template", "模板")}</th><th scope="col">{text("Verdict", "判定")}</th><th className="num" scope="col">{text("Baseline → current", "基线 → 当前")}</th>
          <th className="num" scope="col">{text("Change", "变化")}</th><th className="num" scope="col">{text("Samples baseline / current", "样本 基线 / 当前")}</th><th scope="col">{text("Severity · confidence · evidence", "严重度 · 置信度 · 证据")}</th>
        </tr></thead>
        <tbody>{trends.rows.map((item) => <tr aria-selected={item.template_key === selectedKey} className={item.template_key === selectedKey ? "selected" : undefined} key={item.template_key} onClick={() => onSelect(item.template_key)}>
          <td className="trend-template-cell"><button className="link-button" onClick={() => onSelect(item.template_key)} type="button">{item.template ?? text("Template unavailable", "模板不可用")}</button><span className={`trend-source ${item.kind}`}>{sourceText(item, text)}</span></td>
          <td><span className={`trend-verdict ${verdictTone(item.verdict)}`} title={verdictState(item.verdict)}>{verdictText(item.verdict, text)}</span></td>
          <td className="num">{metricText(item.baseline.median)} → {metricText(item.current.median)}</td>
          <td className="num">{changeText(item)}</td>
          <td className="num">{item.baseline.valid_samples} / {item.current.valid_samples}</td>
          <td className="trend-chips"><span>{severityText(item, text)}</span><span>{confidenceText(item, text)}</span><span className={`evidence ${item.evidence}`}>{evidenceText(item, text)}</span></td>
        </tr>)}</tbody>
      </table></div>}
    </section>
    {row && series ? <div className="history-row">
      <section aria-labelledby="trend-detail" className="history-card wide">
        <div className="history-card-head">
          <h2 id="trend-detail">{text("Hourly median query time", "查询耗时中位数 · 每小时")}</h2>
          <span className="history-muted">{text(`Last ${series.points.length} hours (UTC)`, `最近 ${series.points.length} 小时（UTC）`)}</span>
        </div>
        <code className="trend-template">{row.template ?? text("Template unavailable", "模板不可用")}</code>
        <TrendChart series={series} text={text} />
        <div className="history-legend">
          <span><i className="band" />{text("Baseline median ± MAD", "基线中位数 ± MAD")}</span>
          <span><i className="line" />{text("Hourly median", "每小时中位数")}</span>
          <span><i className="thin" />{text("Thin hour (not judged)", "样本不足的小时（不判定）")}</span>
          <span><i className="partial" />{text("Partially collected", "部分采集")}</span>
          <span><i className="not_collected" />{text("Not collected · line breaks", "未采集 · 曲线断开")}</span>
        </div>
        <h3 className="trend-subhead">{text("Representative executions · current window, slowest first", "代表执行 · 当前窗口 · 按耗时")}</h3>
        {series.exemplars.length === 0 ? <p className="history-muted">{text("No execution kept for this window.", "当前窗口没有保留的执行。")}</p> : <ul className="trend-exemplars">
          {exemplars.map((exemplar) => <li key={exemplar.query_id}>
            <code>{exemplar.query_id}</code><span>{metricText(exemplar.query_time)}</span><span className="history-muted">{clockText(exemplar.event_seconds)}</span>
            {onPullProfile ? <button className="button secondary" onClick={() => onPullProfile(exemplar.query_id)} type="button">{text("Pull Profile and diagnose", "拉取 Profile 并诊断")}</button> : <span className="history-muted">{text("Connect to this cluster to pull", "连接该集群后可拉取")}</span>}
          </li>)}
        </ul>}
        {series.exemplars.length > EXEMPLARS_SHOWN && <button className="link-button trend-more" onClick={() => setAllExemplars((value) => !value)} type="button">{allExemplars ? text("Show fewer", "收起") : text(`Show all ${series.exemplars.length}`, `显示全部 ${series.exemplars.length} 条`)}</button>}
      </section>
      <section aria-labelledby="trend-basis" className="history-card">
        <h2 id="trend-basis">{text("Verdict", "判定")}</h2>
        <p className="trend-verdict-line">
          <span className={`trend-verdict ${verdictTone(row.verdict)}`} title={verdictState(row.verdict)}>{verdictText(row.verdict, text)}</span>
          {!series.rule.calibrated && <span className="history-pending"><span>{text("Rule on trial", "规则试运行中")}</span></span>}
        </p>
        <dl className="history-facts">
          <dt>{text("Baseline", "基线")}</dt><dd>{row.baseline.median ? text(`median ${metricText(row.baseline.median)} · ${row.baseline.executions} runs · ${row.baseline.hours} h`, `中位数 ${metricText(row.baseline.median)} · ${row.baseline.executions} 次 · ${row.baseline.hours} 小时`) : text(`no baseline · ${row.baseline.thin_hours} thin hours`, `无基线 · ${row.baseline.thin_hours} 个样本不足的小时`)}</dd>
          <dt>{text("Current", "当前")}</dt><dd>{text(`median ${metricText(row.current.median)} · ${row.current.valid_samples} / ${row.current.executions} valid`, `中位数 ${metricText(row.current.median)} · 有效 ${row.current.valid_samples} / ${row.current.executions} 次`)}</dd>
          <dt>{text("Change", "变化")}</dt><dd>{changeText(row)}</dd>
          {row.limitations.length > 0 && <><dt>{text("Limitations", "限制")}</dt><dd>{row.limitations.map((key) => limitationText(key, text)).join("；")}</dd></>}
        </dl>
        {onAddToQueue && <button className="button secondary" onClick={() => onAddToQueue(row)} type="button">{text("Add to governance queue", "加入治理队列")}</button>}
        <details className="history-details">
          <summary>{text("How this is judged", "查看判定规则")}</summary>
          <dl className="history-facts">
            <dt>{text("Rule", "规则")}</dt><dd>{text(`outside baseline median ± ${series.rule.k_mad}·1.4826·MAD and ≥ ${series.rule.min_relative_percent}%`, `超出基线中位数 ± ${series.rule.k_mad}·1.4826·MAD 且变化 ≥ ${series.rule.min_relative_percent}%`)}</dd>
            <dt>{text("Samples", "样本条件")}</dt><dd>{text(`≥ ${series.rule.min_baseline_hours} baseline hours with ≥ ${series.rule.min_hour_samples} runs each; ≥ ${series.rule.min_current_samples} current samples`, `基线 ≥ ${series.rule.min_baseline_hours} 个小时、每小时 ≥ ${series.rule.min_hour_samples} 次；当前 ≥ ${series.rule.min_current_samples} 个样本`)}</dd>
            <dt>{text("Baseline spread", "基线离散")}</dt><dd>MAD {metricText(row.baseline.mad)}</dd>
            <dt>{text("Calibration", "校准")}</dt><dd>{series.rule.calibrated ? text("Calibrated on production samples", "已用生产样本校准") : text("The thresholds have not been calibrated on real workload yet; read verdicts as hints.", "阈值尚未用真实业务分布校准，判定结果仅供参考。")}</dd>
            <dt>{text("Comparability", "可比性")}</dt><dd>{text(`StarRocks ${row.comparability.engine_version} · fingerprint rule ${row.comparability.rule_version}`, `StarRocks ${row.comparability.engine_version} · 指纹规则 ${row.comparability.rule_version}`)}</dd>
          </dl>
        </details>
      </section>
    </div> : null}
  </div>;
}
