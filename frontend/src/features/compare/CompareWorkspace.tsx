import { useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  AnalysisAttempt,
  AnalysisSnapshotReport,
  BoundedInterval,
  CanonicalNumber,
  Comparison,
  ConnectedProfileComparisonResponse,
  ConnectedQueryRef,
  Finding,
  FindingMeasurementChange,
  GraphNode,
  GraphSqlFactKind,
  InputOrigin,
  MetricCell,
  MetricDelta,
  OperatorRow,
  PlanComparison,
} from "../../../../contracts/generated/doctor-contracts";
import { localizedComparisonText } from "../../i18n/comparison-text";
import { localizedEnum } from "../../i18n/enum-label";
import { localizedFindingText } from "../../i18n/finding-text";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { measurementLabel, measurementThreshold } from "../../i18n/measurement-label";
import { metricLabel } from "../../i18n/metric-label";
import { ReportExportButton } from "../export/ReportExportButton";
import { physicalOperatorTypeOf, resolveSubjectLocation } from "../recommendations/recommendation-location";
import { findingContext } from "../findings/FindingsPanel";
import { originLabel, shortQueryId } from "../shell/ContextBar";
import {
  canonicalValueText,
  decimalSign,
  divergingBar,
  exactUnit,
  intervalContainsZero,
  intervalText,
  provenBound,
  matchKeyParts,
  orderQueryDeltas,
  ratioIntervalText,
  ratioPercentFullText,
  ratioPercentText,
  signedValueText,
} from "./compare-display";
import { hasVersionPack } from "../../lib/version-pack";
import "./compare.css";

export type CompareWorkspaceProps = {
  comparison: Comparison | null;
  baselineOrigin?: InputOrigin | null;
  candidateOrigin?: InputOrigin | null;
  connectedComparison?: ConnectedProfileComparisonResponse | null;
  planComparison?: PlanComparison | null;
  /** Leave this comparison and choose other Profiles. */
  onReplace?(): void;
  /** Only rendered when the host can actually re-run the comparison swapped. */
  onSwap?(): void;
};

type Side = "baseline" | "candidate";
type CellMaps = Record<Side, Map<string, MetricCell>>;
type RowMaps = Record<Side, { byRef: Map<string, OperatorRow>; byCell: Map<string, OperatorRow> }>;

/**
 * Renders one backend `Comparison`. Before a comparison exists the host shows
 * only the offline form, so this view renders nothing (no placeholder cards).
 */
export function CompareWorkspace({
  comparison,
  baselineOrigin = null,
  candidateOrigin = null,
  connectedComparison = null,
  planComparison = null,
  onReplace,
  onSwap,
}: CompareWorkspaceProps): JSX.Element | null {
  const { text } = useLocale();
  const [showProvenance, setShowProvenance] = useState(false);
  const baseline = comparison ? reportOf(comparison.baseline) : null;
  const candidate = comparison ? reportOf(comparison.candidate) : null;
  const cells = useMemo<CellMaps>(() => ({ baseline: cellsOf(baseline), candidate: cellsOf(candidate) }), [baseline, candidate]);
  const rows = useMemo<RowMaps>(() => ({ baseline: rowsOf(baseline), candidate: rowsOf(candidate) }), [baseline, candidate]);
  if (!comparison) return null;

  const queryDeltas = orderQueryDeltas(comparison.metric_deltas.filter((delta) => delta.subject_match === "query"));
  const operatorDeltas = comparison.metric_deltas.filter((delta) => delta.subject_match !== "query");

  return <section className="compare-workspace" aria-label={text("Profile comparison workspace", "Profile 对比工作区")}>
    <header className="compare-header">
      <div>
        <h1>{text("Before / after comparison", "前后对比")}</h1>
        <p>{text("Measured time and resource changes between two Profiles. Changes that cannot be compared are marked separately and never mixed into the result.", "对照优化前后的实测耗时与资源变化；不可比的指标单独标出，不混入结论。")}</p>
      </div>
      <div className="compare-header-actions">
        {onReplace && <button className="button secondary" onClick={onReplace} type="button">{text("Replace Profiles", "替换 Profile")}</button>}
        <ReportExportButton label={text("Export comparison report", "导出对比报告")} request={{ kind: "comparison", snapshot: comparison }} />
      </div>
    </header>

    <section className="compare-subjects" aria-label={text("Compared Profiles", "对比对象")}>
      <ProfileSlot attempt={comparison.baseline} origin={baselineOrigin} query={connectedComparison?.baseline.query ?? null} side="baseline" />
      <div className="compare-swap-slot">
        {onSwap
          ? <button aria-label={text("Swap baseline and candidate Profiles", "交换基线与候选 Profile")} className="compare-swap" onClick={onSwap} type="button"><SwapIcon /></button>
          : <span aria-hidden="true" className="compare-direction"><ArrowIcon /></span>}
      </div>
      <ProfileSlot attempt={comparison.candidate} origin={candidateOrigin} query={connectedComparison?.candidate.query ?? null} side="candidate" />
      <ComparabilityPanel comparison={comparison} connectedComparison={connectedComparison} />
    </section>

    <div className="compare-main">
      <QueryMetricTable cells={cells} deltas={queryDeltas} />
      <div className="compare-side">
        <FindingChanges baseline={baseline} candidate={candidate} comparison={comparison} />
        <LimitationsCard comparison={comparison} connectedLimitations={connectedComparison?.limitations ?? []} />
      </div>
    </div>

    {operatorDeltas.length > 0 && <OperatorCounters cells={cells} deltas={operatorDeltas} reports={{ baseline, candidate }} rows={rows} />}
    <StructureDiff baseline={baseline} candidate={candidate} comparison={comparison} />
    {planComparison && <PlanCheck planComparison={planComparison} />}

    <section className="compare-evidence" aria-label={text("Comparison evidence", "对比证据")}>
      <button aria-expanded={showProvenance} aria-pressed={showProvenance} className="button secondary" onClick={() => setShowProvenance((open) => !open)} type="button">{text("Raw / Provenance", "原始数据 / 溯源")}</button>
      {showProvenance && <ProvenancePanel baseline={baseline} candidate={candidate} />}
    </section>
  </section>;
}

/* ---------- Pending baseline ---------- */

/**
 * A baseline was set from a pulled Profile; the candidate is still to be
 * picked from the same query list (the backend compares two discovered
 * executions of one discovery generation).
 */
export function PendingComparison({ baseline, onPickCandidate, onClear }: { baseline: { query: ConnectedQueryRef; report: AnalysisSnapshotReport | null }; onPickCandidate?(): void; onClear?(): void }): JSX.Element {
  const { text } = useLocale();
  const clock = baseline.query.observed_at_utc ? /T(\d\d:\d\d:\d\d)/.exec(baseline.query.observed_at_utc)?.[1] ?? null : null;
  const source = [
    text("Fetched from the database", "从数据库拉取"),
    clock ? `${clock} UTC` : null,
    baseline.report ? text(`${baseline.report.findings.length} findings`, `${baseline.report.findings.length} 条发现`) : null,
  ].filter(Boolean).join(" · ");
  return <section className="compare-workspace compare-pending" aria-label={text("Comparison waiting for a candidate", "等待候选的对比")}>
    <header className="compare-header">
      <div>
        <h1>{text("Before / after comparison", "前后对比")}</h1>
        <p>{text("The baseline is set. Pick the candidate: another execution of the same query in the same query list.", "基线已设定。请选择候选：同一查询列表中同一查询的另一次执行。")}</p>
      </div>
      {onClear && <div className="compare-header-actions"><button className="button secondary" onClick={onClear} type="button">{text("Clear baseline", "清除基线")}</button></div>}
    </header>
    <section className="compare-subjects" aria-label={text("Compared Profiles", "对比对象")}>
      <div className="compare-slot baseline">
        <div className="compare-slot-label"><span aria-hidden="true" className="compare-swatch" />{text("Before optimization · baseline", "优化前 · 基线")}</div>
        <div className="compare-slot-id" title={baseline.query.query_id}>{shortQueryId(baseline.query.query_id)}</div>
        <div className="compare-slot-source">{source}</div>
      </div>
      <div className="compare-swap-slot"><span aria-hidden="true" className="compare-direction"><ArrowIcon /></span></div>
      <div className="compare-slot candidate pending">
        <div className="compare-slot-label"><span aria-hidden="true" className="compare-swatch" />{text("After change · candidate", "调整后 · 候选")}</div>
        <p className="compare-pending-steps">{text(
          "Run the changed query once (SQL workspace · Runtime Top), refresh the audit history, then select that execution in the same query family and choose “Compare with baseline”.",
          "修改后执行一次（SQL 工作区 · 运行时 Top），刷新审计历史，在同一查询族中选中那次执行，点“与基线对比”。",
        )}</p>
        {onPickCandidate && <button className="button accent" onClick={onPickCandidate} type="button">{text("Pick the candidate", "去选择候选")}</button>}
      </div>
    </section>
  </section>;
}

/* ---------- Compared Profiles ---------- */

function ProfileSlot({ attempt, origin, query, side }: { attempt: AnalysisAttempt; origin: InputOrigin | null; query: ConnectedQueryRef | null; side: Side }): JSX.Element {
  const { text } = useLocale();
  const report = reportOf(attempt);
  const label = side === "baseline" ? text("Before optimization · baseline", "优化前 · 基线") : text("After change · candidate", "调整后 · 候选");
  const queryId = query?.query_id ?? report?.query_summary?.query_id ?? null;
  const clock = query?.observed_at_utc ? /T(\d\d:\d\d:\d\d)/.exec(query.observed_at_utc)?.[1] ?? null : null;
  const source = [
    origin ? originLabel(origin, text) : null,
    clock ? `${clock} UTC` : null,
    report ? text(`${report.findings.length} findings`, `${report.findings.length} 条发现`) : null,
  ].filter(Boolean).join(" · ");
  const failed = "error" in attempt && attempt.error ? attempt.error : null;
  const flagged = failed !== null || (report !== null && (report.analysis_state === "partial" || report.version.compatibility !== "compatible"));
  return <div className={`compare-slot ${side}`}>
    <div className="compare-slot-label"><span aria-hidden="true" className="compare-swatch" />{label}</div>
    <div className="compare-slot-id" title={queryId ?? undefined}>{queryId ? shortQueryId(queryId) : <span className="chip dashed">{text("No query ID", "无查询 ID")}</span>}</div>
    {source && <div className="compare-slot-source">{source}</div>}
    {flagged ? <div className="compare-slot-chips">
      {failed
        ? <><span className="chip critical">{text("Analysis failed", "分析失败")}</span><code className="compare-code">{failed.code}</code></>
        : <>
          {report && report.version.compatibility !== "compatible" && <span className="chip warn">{localizedEnum(report.version.compatibility, text)}</span>}
          {report?.analysis_state === "partial" && <span className="chip warn">{text("Partial analysis", "分析不完整")}</span>}
        </>}
    </div> : null}
  </div>;
}

/** Comparability in plain lines: versions, connection and SQL identity, what was excluded, then the comparator's reasons. */
function ComparabilityPanel({ comparison, connectedComparison }: { comparison: Comparison; connectedComparison: ConnectedProfileComparisonResponse | null }): JSX.Element {
  const { text } = useLocale();
  const reports = [reportOf(comparison.baseline), reportOf(comparison.candidate)];
  const versions = reports.map((report) => report?.version.effective_version ?? report?.version.reported_version?.value ?? null);
  const packed = reports.every((report) => report !== null && report !== undefined && hasVersionPack(report.version));
  const packs = reports.map((report) => report?.version.selected_pack ? `${report.version.selected_pack.pack_id}@${report.version.selected_pack.product_pack_version}` : text("no pack", "无版本包"));
  const sameVersion = versions[0] !== null && versions[0] === versions[1];
  const versionLine = `${packed ? text("Version pack matched", "版本包匹配") : text("Version pack not matched on both sides", "版本包未在两侧都匹配")} · ${versions[0] ?? text("unknown", "未知")} ${sameVersion ? "=" : "≠"} ${versions[1] ?? text("unknown", "未知")}`;
  const context = connectedComparison?.validation_context ?? null;
  const digests = [connectedComparison?.baseline.query.digest ?? null, connectedComparison?.candidate.query.digest ?? null];
  const digestText = digests[0] && digests[1]
    ? digests[0] === digests[1] ? text("same SQL digest", "同一 SQL digest") : text("different SQL digests", "SQL digest 不同")
    : text("SQL digest unknown", "SQL digest 未知");
  const connectionLine = connectedComparison
    ? `${context?.same_connection_session ? text("Same connection", "同一连接") : text("Different connection sessions", "不同连接会话")} · ${digestText}`
    : text("Two imported Profiles", "两个导入的 Profile");
  const semantic = comparison.metric_deltas.filter((delta) => delta.percentage_state === "semantic_mismatch").length;
  const reasons = countedCodes(comparison.reasons);
  return <div className={`compare-verdict ${comparisonTone(comparison.state)}`} aria-label={text("Comparability", "可比性")} role="group">
    <div className="compare-verdict-state">{localizedEnum(comparison.state, text)}</div>
    <ul className="compare-reasons" data-readable-text="comparison-reasons">
      <li title={packs.join(" · ")}>{versionLine}</li>
      <li title={context ? `${localizedEnum(context.interpretation_state, text)} · ${context.fetch_order} · ${context.candidate_attestation}` : undefined}>{connectionLine}</li>
      {semantic > 0 && <li>{text(`${semantic} metric(s) differ in meaning and are excluded`, `${semantic} 项指标语义不同，已排除`)}</li>}
      {reasons.map(({ code, count }) => {
        const affected = comparison.metric_deltas.filter((delta) => delta.limitations.includes(code)).length;
        return <li key={code} title={code}>
          {codeText(code, text)}
          {count > 1 && <span className="compare-count"> ×{count}</span>}
          {affected > 0 && <span className="compare-affected"> · {text(`${affected} metrics`, `${affected} 项指标`)}</span>}
        </li>;
      })}
    </ul>
  </div>;
}

/* ---------- Metric tables ---------- */

function QueryMetricTable({ cells, deltas }: { cells: CellMaps; deltas: MetricDelta[] }): JSX.Element {
  const { text } = useLocale();
  return <section className="compare-card compare-query-metrics" aria-labelledby="compare-query-metrics-title">
    <div className="compare-card-head">
      <h2 id="compare-query-metrics-title">{text("Measured time and resource changes", "实测耗时与资源变化")}</h2>
      <span>{text("Green = down · orange = up · % relative to baseline", "绿色 = 下降 · 橙色 = 上升 · 百分比相对基线")}</span>
    </div>
    {deltas.length
      ? <MetricTable cells={cells} deltas={deltas} variant="query" />
      : <p className="compare-empty">{text("The comparison returned no query-level metrics.", "对比结果没有查询级指标。")}</p>}
    <p className="compare-card-foot" title={text("Bytes use StarRocks' units: KB / MB / GB are binary (1 MB = 1,048,576 bytes).", "字节沿用 StarRocks 的单位：KB / MB / GB 为二进制（1 MB = 1,048,576 字节）。")}>{text("A zero baseline gives only the absolute change; missing and invalid values are never treated as 0; metrics whose meaning differs are marked “not comparable”.", "基线为 0 时只给绝对变化；缺失与无效值不按 0 计算；语义不同的指标标为「不可比较」。")}</p>
  </section>;
}

function MetricTable({ cells, deltas, variant }: { cells: CellMaps; deltas: MetricDelta[]; variant: "query" | "operator" }): JSX.Element {
  const { text } = useLocale();
  return <div className="compare-table-scroll">
    <table className={`compare-metric-table ${variant}`}>
      <thead><tr>
        <th scope="col">{text("Metric", "指标")}</th>
        <th className="num" scope="col">{text("Before", "基线")}</th>
        <th className="num" scope="col">{text("After", "候选")}</th>
        <th className="num" scope="col">{text("Absolute change", "绝对变化")}</th>
        <th className="num" scope="col">{text("% vs baseline", "相对基线")}</th>
        <th className="compare-bar-col" scope="col"><span className="compare-visually-hidden">{text("Change bar", "变化示意")}</span></th>
      </tr></thead>
      <tbody>{deltas.map((delta, index) => <MetricRow cells={cells} delta={delta} key={`${delta.metric_id}:${delta.baseline_metric_cell_ref ?? "-"}:${delta.candidate_metric_cell_ref ?? "-"}:${index}`} variant={variant} />)}</tbody>
    </table>
  </div>;
}

function MetricRow({ cells, delta, variant }: { cells: CellMaps; delta: MetricDelta; variant: "query" | "operator" }): JSX.Element {
  const { text } = useLocale();
  const baseline = delta.baseline_metric_cell_ref ? cells.baseline.get(delta.baseline_metric_cell_ref) : undefined;
  const candidate = delta.candidate_metric_cell_ref ? cells.candidate.get(delta.candidate_metric_cell_ref) : undefined;
  const unit = exactUnit(baseline) ?? exactUnit(candidate);
  const key = matchKeyParts(baseline?.match_key ?? candidate?.match_key);
  const ratio = delta.percentage_state === "computed" ? delta.percentage_value
    : delta.percentage_state === "approximate" && delta.percentage_bounds ? provenBound(delta.percentage_bounds) : null;
  const bar = divergingBar(ratio);
  const comparable = delta.state === "comparable";
  return <tr>
    <th className="compare-metric-name" scope="row">
      <strong title={delta.metric_id}>{variant === "query" ? queryMetricLabel(delta.metric_id, text) : metricLabel(delta.metric_id, text)}</strong>
      <span className="compare-metric-id">{delta.metric_id}</span>
      {variant === "operator" && key && <span className="compare-metric-hint" title={baseline?.match_key ?? candidate?.match_key}>
        <code>{key.counter}</code>{key.scope && <code className="compare-metric-scope">{key.scope}</code>}
      </span>}
    </th>
    <td className="num">{cellValue(baseline, delta.baseline_metric_cell_ref !== null, text)}</td>
    <td className={`num${comparable ? "" : " muted"}`}>{cellValue(candidate, delta.candidate_metric_cell_ref !== null, text)}</td>
    <td className="num">{absoluteChange(delta, unit, text)}</td>
    <td className="num">{percentChange(delta, text)}</td>
    <td className="compare-bar-col"><div aria-hidden="true" className="compare-bar">
      {bar && <div className={`compare-bar-fill ${bar.direction}${bar.clamped ? " clamped" : ""}`} style={{ left: `${bar.left}%`, width: `${bar.width}%` }} />}
    </div></td>
  </tr>;
}

/** Query metrics use the Doctor summary's names, so the two pages read the same. */
function queryMetricLabel(metricId: string, text: LocalizedText): string {
  switch (metricId) {
    case "QueryTotalTime": return text("Total time", "总耗时");
    case "QueryExecutionWallTime": return text("Execution (wall)", "执行（墙钟）");
    case "QueryCumulativeCpuTime": return text("CPU (sum)", "CPU 累计");
    default: return metricLabel(metricId, text);
  }
}

function cellValue(cell: MetricCell | undefined, referenced: boolean, text: LocalizedText): ReactNode {
  if (!cell) return <span className="compare-missing">{referenced ? text("Unavailable", "不可用") : text("Missing", "缺失")}</span>;
  if (cell.state !== "exact") return <StateBadge tone={cell.state === "invalid" ? "critical" : cell.state === "partial" ? "warn" : undefined}>{localizedEnum(cell.state, text)}</StateBadge>;
  const fidelity = cell.fidelity === "exact" ? "" : ` · ${localizedEnum(cell.fidelity, text)}`;
  return <span title={`${cell.display_text}${fidelity}`}>{canonicalValueText(cell.canonical_number, cell.canonical_unit)}</span>;
}

/** Direction of a signed canonical value: the colour says down or up, never better or worse. */
function signTone(value: CanonicalNumber | null): "down" | "up" | "none" {
  if (!value) return "none";
  const number = Number(value.value);
  return number < 0 ? "down" : number > 0 ? "up" : "none";
}

/** Direction of an interval that excludes 0 (its two ends share one sign); one that admits 0 has none. */
function boundsTone(bounds: BoundedInterval): "down" | "up" | "none" {
  if (intervalContainsZero(bounds)) return "none";
  return decimalSign(bounds.lower.value) >= 0 ? "up" : "down";
}

function absoluteChange(delta: MetricDelta, unit: string | null, text: LocalizedText): ReactNode {
  if (delta.state !== "comparable") {
    const short = delta.limitations.includes("metric_missing_on_one_side") ? text("Unavailable", "不可用") : text("Not comparable", "不可比较");
    return <span className="compare-change none" title={[notComparableText(delta, text), ...delta.limitations].join(" · ")}>{short}</span>;
  }
  if (delta.absolute_delta) return <span className={`compare-change ${signTone(delta.absolute_delta)}`}>{signedValueText(delta.absolute_delta, unit)}</span>;
  if (delta.bounds) {
    const interval = intervalText(delta.bounds, unit);
    if (intervalContainsZero(delta.bounds)) return <span className="compare-no-change" title={`${text("Change interval", "变化区间")} ${interval}`}>{text("No change within display precision", "在显示精度内无变化")}</span>;
    return <span className={`compare-change ${boundsTone(delta.bounds)}`} title={`${text("Change interval", "变化区间")} ${interval}`}>≈ {approximateText(canonicalValueText(delta.bounds.lower, unit), canonicalValueText(delta.bounds.upper, unit))}</span>;
  }
  return <span className="compare-change none">{text("Unavailable", "不可用")}</span>;
}

/** An interval whose two ends read the same at display precision shows once; the exact interval stays in the title. */
function approximateText(lower: string, upper: string): string {
  return lower === upper ? lower : `${lower} … ${upper}`;
}

function notComparableText(delta: MetricDelta, text: LocalizedText): string {
  if (delta.limitations.includes("duplicate_metric_match")) return text("Ambiguous: several counters share this name", "多个计数器同名，无法唯一配对");
  if (delta.limitations.includes("metric_missing_on_one_side")) return text("Only on one side", "仅一侧存在");
  if (delta.limitations.includes("metric_delta_overflow")) return text("Overflow", "溢出");
  if (delta.limitations.includes("parallelism_shape_mismatch")) return text("Parallelism differs", "并行度不同");
  if (delta.limitations.includes("parallelism_shape_unavailable")) return text("Parallelism unknown", "并行度未知");
  return text("Not comparable", "不可比较");
}

function percentChange(delta: MetricDelta, text: LocalizedText): ReactNode {
  switch (delta.percentage_state) {
    case "computed":
      return delta.percentage_value ? <span className={`compare-change ${signTone(delta.percentage_value)}`} title={ratioPercentFullText(delta.percentage_value)}>{ratioPercentText(delta.percentage_value)}</span> : <Dash />;
    case "approximate": {
      if (!delta.percentage_bounds) return <Dash />;
      const interval = ratioIntervalText(delta.percentage_bounds);
      if (delta.bounds && intervalContainsZero(delta.bounds)) return <Dash title={`${text("Percentage interval", "百分比区间")} ${interval}`} />;
      return <span className={`compare-change ${boundsTone(delta.percentage_bounds)}`} title={`${text("Percentage interval", "百分比区间")} ${interval}`}>≈ {approximateText(ratioPercentText(delta.percentage_bounds.lower), ratioPercentText(delta.percentage_bounds.upper))}</span>;
    }
    case "undefined_baseline_zero":
      return <StateBadge>{text("Undefined · baseline = 0", "未定义 · 基线 = 0")}</StateBadge>;
    case "undefined_baseline_interval_contains_zero":
      return <StateBadge>{text("Undefined · baseline range includes 0", "未定义 · 基线区间含 0")}</StateBadge>;
    case "missing":
      return <StateBadge>{delta.baseline_metric_cell_ref === null || delta.candidate_metric_cell_ref !== null
        ? text("No baseline observation", "缺少基线观测值")
        : text("No candidate observation", "缺少候选观测值")}</StateBadge>;
    case "invalid":
      return <StateBadge tone="critical">{text("Invalid observation", "观测值无效")}</StateBadge>;
    case "semantic_mismatch":
      return <StateBadge title={notComparableText(delta, text)}>{delta.limitations.includes("duplicate_metric_match") ? text("No unique pairing", "无法唯一配对") : text("Meaning differs", "指标含义不同")}</StateBadge>;
  }
}

function StateBadge({ children, title, tone }: { children: ReactNode; title?: string; tone?: "critical" | "warn" }): JSX.Element {
  return <span className={`compare-state${tone ? ` ${tone}` : ""}`} title={title}>{children}</span>;
}

function Dash({ title }: { title?: string }): JSX.Element {
  return <span className="compare-dash" title={title}>—</span>;
}

/* ---------- Operator counters ---------- */

type OperatorGroup = { key: string; label: string | null; physical: string[]; deltas: MetricDelta[] };
type ReportPair = Record<Side, AnalysisSnapshotReport | null>;

function OperatorCounters({ cells, deltas, reports, rows }: { cells: CellMaps; deltas: MetricDelta[]; reports: ReportPair; rows: RowMaps }): JSX.Element {
  const { text } = useLocale();
  const groups = operatorGroups(deltas, rows, cells, reports);
  const labelTotals = new Map<string, number>();
  for (const group of groups) if (group.label) labelTotals.set(group.label, (labelTotals.get(group.label) ?? 0) + 1);
  const labelSeen = new Map<string, number>();
  return <section className="compare-card compare-operators" aria-labelledby="compare-operators-title">
    <div className="compare-card-head">
      <h2 id="compare-operators-title">{text("Operator counters by plan node", "按计划节点的算子计数器")} <span className="compare-head-count">{deltas.length}</span></h2>
      <span>{text(`${groups.length} operator groups · collapsed by default`, `${groups.length} 个算子组 · 默认折叠`)}</span>
    </div>
    {groups.map((group) => {
      const total = group.label ? labelTotals.get(group.label) ?? 1 : 1;
      const ordinal = group.label ? (labelSeen.get(group.label) ?? 0) + 1 : 1;
      if (group.label) labelSeen.set(group.label, ordinal);
      const notComparable = group.deltas.filter((delta) => delta.state !== "comparable").length;
      return <details className="compare-operator-group" key={group.key}>
        <summary>
          <span className="compare-operator-label">{group.label ?? text("Unlabelled subject", "未标注对象")}</span>
          {group.physical.length > 0
            ? <span className="compare-affected">{group.physical.join(" / ")}</span>
            : total > 1 && <span className="compare-affected" title={text("The same plan node is reported by more than one operator group, e.g. its sending and receiving fragments.", "同一计划节点由多个算子组上报，例如发送端与接收端 Fragment。")}>{text(`group ${ordinal} of ${total}`, `第 ${ordinal}/${total} 组`)}</span>}
          <span className="compare-affected">{text(`${group.deltas.length} counters`, `${group.deltas.length} 个计数器`)}</span>
          {notComparable > 0 && <span className="chip dashed">{text(`${notComparable} not comparable`, `${notComparable} 项不可比`)}</span>}
        </summary>
        <MetricTable cells={cells} deltas={group.deltas} variant="operator" />
      </details>;
    })}
  </section>;
}

function operatorGroups(deltas: MetricDelta[], rows: RowMaps, cells: CellMaps, reports: ReportPair): OperatorGroup[] {
  const groups = new Map<string, OperatorGroup>();
  for (const delta of deltas) {
    const key = delta.subject_match ?? "\u0000unattributed";
    let group = groups.get(key);
    if (!group) {
      group = { key, label: operatorLabel(operatorRowFor(delta, rows)), physical: [], deltas: [] };
      groups.set(key, group);
    }
    group.deltas.push(delta);
    const type = physicalTypeFor(delta, cells, rows, reports);
    if (type && !group.physical.includes(type)) group.physical.push(type);
  }
  return [...groups.values()];
}

/** Physical operator of a delta's baseline (else candidate) cell, by the exact source-line join; never parsed from refs. */
function physicalTypeFor(delta: MetricDelta, cells: CellMaps, rows: RowMaps, reports: ReportPair): string | null {
  for (const side of ["baseline", "candidate"] as const) {
    const ref = side === "baseline" ? delta.baseline_metric_cell_ref : delta.candidate_metric_cell_ref;
    const report = reports[side];
    const cell = ref ? cells[side].get(ref) : undefined;
    const row = ref ? rows[side].byCell.get(ref) : undefined;
    if (!report || !cell || !row) continue;
    const nodeId = resolveSubjectLocation(report, row.operator_ref).node?.node_id ?? null;
    const type = physicalOperatorTypeOf(report, cell, nodeId);
    if (type) return type;
  }
  return null;
}

/** The operator row named by the subject, else the row that owns either side's cell. Never parsed from the ref text. */
function operatorRowFor(delta: MetricDelta, rows: RowMaps): OperatorRow | null {
  const subject = delta.subject_match;
  if (subject) {
    const row = rows.baseline.byRef.get(subject) ?? rows.candidate.byRef.get(subject);
    if (row) return row;
  }
  return (delta.baseline_metric_cell_ref ? rows.baseline.byCell.get(delta.baseline_metric_cell_ref) : undefined)
    ?? (delta.candidate_metric_cell_ref ? rows.candidate.byCell.get(delta.candidate_metric_cell_ref) : undefined)
    ?? null;
}

function operatorLabel(row: OperatorRow | null): string | null {
  if (!row) return null;
  return hasPlanNode(row.plan_node_id) ? `Plan ${row.plan_node_id} · ${row.operator_type}` : row.operator_type;
}

/** StarRocks prints plan node id -1 for operators outside the plan tree (RESULT_SINK); it is not a node to show. */
function hasPlanNode(planNodeId: string | null): planNodeId is string {
  return planNodeId !== null && planNodeId !== "-1";
}

/* ---------- Findings, limitations, structure ---------- */

type FindingKind = "resolved" | "added" | "persisting" | "changed" | "incomparable" | "algorithm_changed";

function FindingChanges({ baseline, candidate, comparison }: { baseline: AnalysisSnapshotReport | null; candidate: AnalysisSnapshotReport | null; comparison: Comparison }): JSX.Element {
  const { text } = useLocale();
  const delta = comparison.finding_delta;
  const counts: { kind: FindingKind; label: string; tone: string }[] = [
    { kind: "resolved", label: text("Resolved", "已解决"), tone: "ok" },
    { kind: "added", label: text("New", "新增"), tone: "warn" },
    { kind: "persisting", label: text("Persisting", "持续"), tone: "" },
    ...(delta.changed.length ? [{ kind: "changed" as const, label: text("Changed", "已变化"), tone: "accent" }] : []),
  ];
  const extra: { kind: FindingKind; label: string; tone: string }[] = [
    { kind: "incomparable", label: text("Not comparable", "不可比较"), tone: "dashed" },
    { kind: "algorithm_changed", label: text("Rule version changed", "规则版本变化"), tone: "dashed" },
  ];
  const items = [...counts, ...extra].flatMap(({ kind, label, tone }) => delta[kind].map((entry) => ({ kind, label, tone, entry })));
  // The comparator names resolved/new entries decided on partial evidence; nothing is inferred here.
  const partial = new Set(delta.partial_evidence ?? []);
  return <section className="compare-card compare-findings" aria-labelledby="compare-findings-title">
    <div className="compare-card-head"><h2 id="compare-findings-title">{text("Finding changes", "发现项变化")}</h2></div>
    <div className="compare-finding-counts" style={{ gridTemplateColumns: `repeat(${counts.length}, minmax(0, 1fr))` }}>
      {counts.map(({ kind, label, tone }) => <div key={kind}><span>{label}</span><strong className={tone}>{delta[kind].length}</strong></div>)}
    </div>
    {extra.some(({ kind }) => delta[kind].length > 0) && <p className="compare-card-note">{extra.filter(({ kind }) => delta[kind].length > 0).map(({ kind, label }) => `${label} ${delta[kind].length}`).join(" · ")}</p>}
    {partial.size > 0 && <p className="compare-card-note">{text(`${partial.size} resolved or new finding(s) rest on partial evidence`, `其中 ${partial.size} 项已解决或新增依据部分证据判定`)}</p>}
    {items.length
      ? <ul className="compare-finding-list">{items.map(({ kind, label, tone, entry }) => {
        // A resolved finding exists only in the baseline; the others are read from the candidate first.
        const sides = kind === "resolved" ? [baseline] : [candidate, baseline];
        const match = sides.map((report) => ({ report, finding: findingFor(report, entry) })).find((item) => item.finding) ?? null;
        const context = match?.report && match.finding ? findingContext(match.report, match.finding) : null;
        // The lead measurement on both sides, from the comparator; kept in the row's title.
        const lead = delta.measurement_changes.find((change) => change.finding_entry === entry) ?? null;
        return <li key={`${kind}:${entry}`} title={lead ? measurementChangeText(lead, text) : undefined}>
          <span className={`chip ${tone}`}>{label}</span>
          {(kind === "resolved" || kind === "added") && partial.has(entry) && <span className="chip dashed" title={text("Decided on partial evidence: the rule ran on both sides with no missing prerequisite, but some inputs were incomplete.", "依据部分证据判定：两侧都完成了该规则评估且没有缺少前提，但部分输入不完整。")}>{text("Partial evidence", "证据部分")}</span>}
          <span className="compare-finding-title">{match?.finding ? localizedFindingText(match.finding.rule_id, "category", match.finding.category, text) : <code>{entry}</code>}</span>
          {match?.finding && <code className="compare-finding-subject" title={`${match.finding.rule_id}${context?.label ? ` · ${context.label}` : ""}`}>{context?.planNodeId != null ? `Plan ${context.planNodeId}` : context?.label ?? text("Whole query", "整条查询")}</code>}
        </li>;
      })}</ul>
      : <p className="compare-empty">{text("Neither Profile produced findings.", "两个 Profile 均没有发现项。")}</p>}
  </section>;
}

/** "Share of operator time 46% → 12% · rule: > 30%"; an absent side is named, never 0. */
function measurementChangeText(change: FindingMeasurementChange, text: LocalizedText): string {
  const side = (value: string | null) => value ?? text("absent", "无");
  const before = change.baseline_threshold_text;
  const after = change.candidate_threshold_text;
  const threshold = before === null || after === null || before === after ? after ?? before : `${before} → ${after}`;
  return `${measurementLabel(change.measurement_id, text)} ${side(change.baseline_display_text)} → ${side(change.candidate_display_text)}${threshold ? ` · ${measurementThreshold(threshold, text)}` : ""}`;
}

/** FindingDelta entries are `rule_id:subject_ref`; match them by whole-string equality only. */
function findingFor(report: AnalysisSnapshotReport | null, entry: string): Finding | null {
  return report?.findings.find((finding) => `${finding.rule_id}:${finding.subject_ref ?? ""}` === entry) ?? null;
}

function LimitationsCard({ comparison, connectedLimitations }: { comparison: Comparison; connectedLimitations: string[] }): JSX.Element | null {
  const { text } = useLocale();
  // Top-level reasons are already shown once in the comparability panel.
  const shown = new Set(comparison.reasons);
  const limitations = countedCodes([...comparison.limitations, ...connectedLimitations].filter((code) => !shown.has(code)));
  if (!limitations.length) return null;
  return <section className="compare-card compare-limitations" aria-labelledby="compare-limitations-title">
    <h2 id="compare-limitations-title">{text("Limitations", "限制")}</h2>
    <ul data-readable-text="comparison-limitations">{limitations.map(({ code, count }) => <li key={code} title={code}>{codeText(code, text)}{count > 1 && <span className="compare-count"> ×{count}</span>}</li>)}</ul>
  </section>;
}

type StructureRow = { key: string; node: ReactNode; baseline: ReactNode; candidate: ReactNode; change: string; tone: string; muted?: boolean };

function StructureDiff({ baseline, candidate, comparison }: { baseline: AnalysisSnapshotReport | null; candidate: AnalysisSnapshotReport | null; comparison: Comparison }): JSX.Element {
  const { text } = useLocale();
  const delta = comparison.structure_delta;
  const nodeIn = (report: AnalysisSnapshotReport | null, id: string) => report?.graph_view.nodes.find((node) => node.node_id === id) ?? null;
  const none = <span className="compare-missing">—</span>;
  const edgeLabel = (id: string): ReactNode => {
    const edge = baseline?.graph_view.edges.find((item) => item.edge_id === id) ?? candidate?.graph_view.edges.find((item) => item.edge_id === id);
    if (!edge) return <code>{id}</code>;
    const from = nodeIn(baseline, edge.from) ?? nodeIn(candidate, edge.from);
    const to = nodeIn(baseline, edge.to) ?? nodeIn(candidate, edge.to);
    return <code>{nodeLabel(from, edge.from)} → {nodeLabel(to, edge.to)}</code>;
  };
  const inBaseline = (id: string) => baseline?.graph_view.edges.some((edge) => edge.edge_id === id) ?? false;
  // Nodes the backend could not connect on either side take no part in the structure comparison.
  const unconnected = Array.from(new Set([...(baseline?.graph_view.unconnected_node_ids ?? []), ...(candidate?.graph_view.unconnected_node_ids ?? [])]));
  const unproven = text("Relation unproven", "关系未证实");
  const rows: StructureRow[] = [
    ...delta.changed.map((id) => {
      const before = nodeIn(baseline, id);
      const after = nodeIn(candidate, id);
      const kinds = changedFactKinds(before, after);
      return { key: `changed:${id}`, node: <code>{nodeLabel(after ?? before, id)}</code>, baseline: <code>{factText(before, kinds)}</code>, candidate: <code>{factText(after, kinds)}</code>, change: `${factChangeLabel(kinds, text)} · ${text("explicit", "显式")}`, tone: "accent" };
    }),
    ...delta.added.map((id) => ({ key: `added:${id}`, node: <code>{nodeLabel(nodeIn(candidate, id), id)}</code>, baseline: none, candidate: <code>{factText(nodeIn(candidate, id), null)}</code>, change: text("Added in candidate", "候选新增"), tone: "accent" })),
    ...delta.removed.map((id) => ({ key: `removed:${id}`, node: <code>{nodeLabel(nodeIn(baseline, id), id)}</code>, baseline: <code>{factText(nodeIn(baseline, id), null)}</code>, candidate: none, change: text("Removed in candidate", "候选中移除"), tone: "warn" })),
    ...delta.edge_delta.map((id) => ({ key: `edge:${id}`, node: edgeLabel(id), baseline: inBaseline(id) ? text("Present", "存在") : none, candidate: inBaseline(id) ? none : text("Present", "存在"), change: text("Relation differs", "关系不同"), tone: "" })),
    ...unconnected.map((id) => ({ key: `unconnected:${id}`, node: <code>{nodeLabel(nodeIn(candidate, id) ?? nodeIn(baseline, id), id)}</code>, baseline: unproven, candidate: unproven, change: text("Not compared", "不参与比较"), tone: "dashed", muted: true })),
    ...delta.unresolved.map((code) => ({ key: `unresolved:${code}`, node: <span title={code}>{localizedComparisonText("detail", code, text)}</span>, baseline: none, candidate: none, change: text("Not compared", "不参与比较"), tone: "dashed", muted: true })),
  ];
  return <section className="compare-card compare-structure" aria-labelledby="compare-structure-title">
    <div className="compare-card-head">
      <h2 id="compare-structure-title">{text("Structure diff", "结构差异")} <span className="compare-head-count">{rows.filter((row) => !row.muted).length}</span></h2>
      <span>{text("Only proven plan-node mappings are compared", "仅比较可证实的计划节点映射")}</span>
    </div>
    {rows.length
      ? <div className="compare-table-scroll"><table className="compare-structure-table">
        <thead><tr><th scope="col">{text("Plan node", "计划节点")}</th><th scope="col">{text("Baseline", "基线")}</th><th scope="col">{text("Candidate", "候选")}</th><th scope="col">{text("Change", "变化")}</th></tr></thead>
        <tbody>{rows.map((row) => <tr className={row.muted ? "muted" : undefined} key={row.key}><td>{row.node}</td><td>{row.baseline}</td><td>{row.candidate}</td><td><span className={`chip ${row.tone}`}>{row.change}</span></td></tr>)}</tbody>
      </table></div>
      : <p className="compare-empty">{text(`No structural change: ${delta.matches.length} plan nodes matched.`, `结构无变化：${delta.matches.length} 个计划节点已匹配。`)}</p>}
  </section>;
}

const FACT_LABELS: Record<GraphSqlFactKind, string> = { table: "TABLE", predicate: "PREDICATES", join_condition: "JOIN", fields: "FIELDS" };

/** Fact kinds whose printed values differ between the two sides of a node the backend marked changed. */
function changedFactKinds(before: GraphNode | null, after: GraphNode | null): GraphSqlFactKind[] {
  const values = (node: GraphNode | null, kind: GraphSqlFactKind) => (node?.sql_facts ?? []).filter((fact) => fact.kind === kind).map((fact) => fact.value).sort().join("\u0000");
  return (Object.keys(FACT_LABELS) as GraphSqlFactKind[]).filter((kind) => values(before, kind) !== values(after, kind));
}

/** "PREDICATES: order_date >= …"; a kind the side does not print reads "—". `null` lists every printed fact. */
function factText(node: GraphNode | null, kinds: GraphSqlFactKind[] | null): string {
  const facts = node?.sql_facts ?? [];
  const shown = kinds ?? Array.from(new Set(facts.map((fact) => fact.kind)));
  if (!shown.length) return "—";
  return shown.map((kind) => `${FACT_LABELS[kind]}: ${facts.filter((fact) => fact.kind === kind).map((fact) => fact.value).join("; ") || "—"}`).join(" · ");
}

function factChangeLabel(kinds: GraphSqlFactKind[], text: LocalizedText): string {
  if (kinds.length === 1 && kinds[0] === "predicate") return text("Predicate changed", "谓词变化");
  if (kinds.length === 1 && kinds[0] === "join_condition") return text("Join condition changed", "关联条件变化");
  if (kinds.length === 1 && kinds[0] === "table") return text("Table changed", "表变化");
  return text("Plan facts changed", "计划事实变化");
}

/** "Plan 0 · OLAP_SCAN orders": plan id, operator and the table it reads. */
function nodeLabel(node: GraphNode | null, fallback: string): string {
  if (!node) return fallback;
  const name = node.plan_node_type ?? node.display_name ?? node.node_id;
  const table = node.sql_facts?.find((fact) => fact.kind === "table")?.value.split(".").pop();
  const operator = `${name}${table ? ` ${table}` : ""}`;
  return hasPlanNode(node.plan_node_id) ? `Plan ${node.plan_node_id} · ${operator}` : operator;
}

function PlanCheck({ planComparison }: { planComparison: PlanComparison }): JSX.Element {
  const { text } = useLocale();
  return <aside className="compare-card compare-plan-check">
    <div className="compare-card-head"><h2>{text("Plan check", "计划检查")}</h2><span className="chip">{localizedEnum(planComparison.state, text)}</span></div>
    <p className="compare-card-note">{text("Plan structure only; actual performance needs execution observations.", "仅比较计划结构；实际性能需结合执行观测。")}</p>
    {planComparison.limitations.length > 0 && <ul>{planComparison.limitations.map((reason) => <li key={reason}><code>{localizedComparisonText("limitation", reason, text)}</code></li>)}</ul>}
  </aside>;
}

/* ---------- Raw / Provenance ---------- */

export function ProvenancePanel({ baseline, candidate }: { baseline: AnalysisSnapshotReport | null; candidate: AnalysisSnapshotReport | null }): JSX.Element {
  const { text } = useLocale();
  return <section className="compare-card compare-provenance-panel" aria-labelledby="comparison-provenance-title">
    <div className="compare-card-head"><h2 id="comparison-provenance-title">{text("Raw / Provenance", "原始数据 / 溯源")}</h2><span>{text("Identity and source references of both analyses. The comparison never returns raw Profile text.", "两次分析的标识与来源引用；对比不返回 Profile 原文。")}</span></div>
    <div className="compare-provenance-sides">
      <ProvenanceSide label={text("Baseline", "基线")} report={baseline} />
      <ProvenanceSide label={text("Candidate", "候选")} report={candidate} />
    </div>
  </section>;
}

function ProvenanceSide({ label, report }: { label: string; report: AnalysisSnapshotReport | null }): JSX.Element {
  const { text } = useLocale();
  if (!report) return <section className="compare-provenance-side"><h3>{label}</h3><p>{text("No analyzed Profile selected.", "未选择已分析的 Profile。")}</p></section>;
  const sourceRefs = Array.from(new Set([...report.provenance_index, ...report.profile_context.evidence_refs, ...report.graph_view.source_refs]));
  const versionEvidence = report.version.reported_version ?? report.version.detection_evidence[0] ?? null;
  return <section className="compare-provenance-side">
    <h3>{label}</h3>
    <dl>
      <div><dt>{text("Analysis key", "分析键")}</dt><dd><code>{report.analysis_key}</code></dd></div>
      <div><dt>{text("Adapter", "适配器")}</dt><dd><code>{`${report.input.detected_kind} · ${report.input.adapter_evidence.join(" / ") || text("none", "无")}`}</code></dd></div>
      <div><dt>{text("Version evidence", "版本证据")}</dt><dd><code>{versionEvidence ? `${versionEvidence.value} · ${versionEvidence.source_ref}` : text("No version evidence", "无版本证据")}</code></dd></div>
    </dl>
    <details className="compare-provenance-refs">
      <summary>{text(`Source refs (${sourceRefs.length})`, `来源引用（${sourceRefs.length}）`)}</summary>
      {sourceRefs.length ? <ul>{sourceRefs.map((reference) => <li key={reference}><code>{reference}</code></li>)}</ul> : <p>{text("No source refs supplied.", "未提供来源引用。")}</p>}
    </details>
  </section>;
}

/* ---------- Helpers ---------- */

function reportOf(attempt: AnalysisAttempt): AnalysisSnapshotReport | null {
  return "outcome" in attempt ? attempt.outcome?.report ?? null : null;
}

function cellsOf(report: AnalysisSnapshotReport | null): Map<string, MetricCell> {
  const cells = new Map<string, MetricCell>();
  if (!report) return cells;
  for (const cell of report.derived_metric_cells) cells.set(cell.metric_cell_ref, cell);
  for (const row of report.operator_rows) for (const cell of row.metric_cells) cells.set(cell.metric_cell_ref, cell);
  return cells;
}

function rowsOf(report: AnalysisSnapshotReport | null): RowMaps[Side] {
  const byRef = new Map<string, OperatorRow>();
  const byCell = new Map<string, OperatorRow>();
  for (const row of report?.operator_rows ?? []) {
    if (!byRef.has(row.operator_ref)) byRef.set(row.operator_ref, row);
    for (const cell of row.metric_cells) byCell.set(cell.metric_cell_ref, row);
  }
  return { byRef, byCell };
}

/** First-occurrence order with a repeat count; codes are never re-derived. */
function countedCodes(codes: string[]): { code: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const code of codes) counts.set(code, (counts.get(code) ?? 0) + 1);
  return [...counts].map(([code, count]) => ({ code, count }));
}

function codeText(code: string, text: LocalizedText): string {
  const limitation = localizedComparisonText("limitation", code, text);
  return limitation !== code ? limitation : localizedComparisonText("reason", code, text);
}

function comparisonTone(state: Comparison["state"]): "ok" | "warn" | "critical" {
  return state === "comparable" ? "ok" : state === "partially_comparable" ? "warn" : "critical";
}

function SwapIcon(): JSX.Element {
  return <svg aria-hidden="true" fill="none" height="16" stroke="currentColor" strokeLinecap="round" strokeWidth="2" viewBox="0 0 24 24" width="16"><path d="M7 7h12l-3-3M17 17H5l3 3" /></svg>;
}

function ArrowIcon(): JSX.Element {
  return <svg aria-hidden="true" fill="none" height="16" stroke="currentColor" strokeLinecap="round" strokeWidth="2" viewBox="0 0 24 24" width="16"><path d="M5 12h14M13 6l6 6-6 6" /></svg>;
}
