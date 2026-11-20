import { Fragment, useMemo, useReducer, useState } from "react";
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
  SelectedVersionPack,
} from "../../../../contracts/generated/doctor-contracts";
import {
  Button,
  Card,
  Dialog,
  EvidenceMeter,
  gridSpanClasses,
  InfoTip,
  MetricName,
  PageGrid,
  SegTabs,
  type EvidenceLevel,
  type SegItem,
} from "../../design";
import { localizedComparisonText } from "../../i18n/comparison-text";
import { localizedEnum } from "../../i18n/enum-label";
import { localizedFindingText } from "../../i18n/finding-text";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { measurementLabel, measurementThreshold } from "../../i18n/measurement-label";
import { metricLabel } from "../../i18n/metric-label";
import { hasVersionPack } from "../../lib/version-pack";
import { findingContext, findingObjectLabel, findingTitleText } from "../findings/FindingsPanel";
import { physicalOperatorTypeOf, resolveSubjectLocation } from "../recommendations/recommendation-location";
import { originLabel, shortQueryId } from "../shell/ContextBar";
import {
  canonicalValueText,
  decimalSign,
  divergingBar,
  exactUnit,
  intervalContainsZero,
  intervalText,
  matchKeyParts,
  orderQueryDeltas,
  provenBound,
  ratioIntervalText,
  ratioPercentFullText,
  ratioPercentText,
  signedValueText,
} from "./compare-display";
import "./compare.css";

export type CompareTab = "metrics" | "operators" | "structure" | "plan" | "provenance";

export type CompareDrawerState =
  | { comparisonKey: string; kind: "finding"; itemKey: string }
  | { comparisonKey: string; kind: "limitations" }
  | null;

export type CompareDrawerAction =
  | { type: "open_finding"; comparisonKey: string; itemKey: string }
  | { type: "open_limitations"; comparisonKey: string }
  | { type: "close" };

export function reduceCompareDrawerState(
  _state: CompareDrawerState,
  action: CompareDrawerAction,
): CompareDrawerState {
  switch (action.type) {
    case "open_finding":
      return { comparisonKey: action.comparisonKey, kind: "finding", itemKey: action.itemKey };
    case "open_limitations":
      return { comparisonKey: action.comparisonKey, kind: "limitations" };
    case "close":
      return null;
  }
}

export function activeCompareDrawer(
  state: CompareDrawerState,
  comparisonKey: string,
): CompareDrawerState {
  return state && state.comparisonKey === comparisonKey ? state : null;
}

export type CompareWorkspaceProps = {
  comparison: Comparison | null;
  baselineOrigin?: InputOrigin | null;
  candidateOrigin?: InputOrigin | null;
  connectedComparison?: ConnectedProfileComparisonResponse | null;
  planComparison?: PlanComparison | null;
  /** Initial active tab (defaults to "metrics"; lets SSR tests render non-default tabs). */
  initialTab?: CompareTab;
  /** Optional finding-change item key (`${kind}:${entry}`) opened in the detail drawer (defaults to null: closed on load). */
  initialFindingKey?: string | null;
  /** Optional initial open state for the limitations drawer (defaults to false: closed on load). */
  initialLimitationsOpen?: boolean;
  /** Retained for App.tsx compatibility; ContextBar owns comparison actions in v1.1. */
  onReplace?(): void;
  /** Retained for App.tsx compatibility; ContextBar owns the swap action in v1.1. */
  onSwap?(): void;
};

type Side = "baseline" | "candidate";
type CellMaps = Record<Side, Map<string, MetricCell>>;
type RowMaps = Record<Side, { byRef: Map<string, OperatorRow>; byCell: Map<string, OperatorRow> }>;

const LOCAL_REASON_LABELS: Record<string, readonly [string, string]> = {
  version_pack_mismatch: ["Version packs differ between the two analyses", "两次分析的版本包不匹配"],
  starrocks_version_mismatch: ["StarRocks versions differ; cross-version metrics are not compared", "StarRocks 版本不同，跨版本指标不参与比较"],
  metric_state_not_comparable: ["Counter state is not comparable", "计数器状态不可比较"],
  bounded_percentage: ["Percentage change is bounded by display precision", "百分比变化受显示精度限制，以区间表示"],
  partial_inputs: ["Both analyses are partial", "两侧分析均不完整"],
  not_comparable: ["Not comparable", "不可比较"],
  baseline_metric_missing: ["A baseline counter is missing", "基线存在缺失的计数器"],
  baseline_metric_invalid: ["A baseline counter value is invalid", "基线存在无效的计数器观测值"],
  baseline_metric_partial: ["A baseline counter value is partial", "基线存在不完整的计数器观测值"],
  baseline_metric_overflow: ["A baseline counter overflowed", "基线存在溢出的计数器观测值"],
  baseline_metric_unavailable: ["A baseline counter is unavailable", "基线存在不可用的计数器"],
  baseline_metric_incompatible: ["A baseline counter is incompatible", "基线存在语义不兼容的计数器"],
  baseline_metric_unknown: ["A baseline counter state is unknown", "基线存在状态未知的计数器"],
  candidate_metric_missing: ["A candidate counter is missing", "候选存在缺失的计数器"],
  candidate_metric_invalid: ["A candidate counter value is invalid", "候选存在无效的计数器观测值"],
  candidate_metric_partial: ["A candidate counter value is partial", "候选存在不完整的计数器观测值"],
  candidate_metric_overflow: ["A candidate counter overflowed", "候选存在溢出的计数器观测值"],
  candidate_metric_unavailable: ["A candidate counter is unavailable", "候选存在不可用的计数器"],
  candidate_metric_incompatible: ["A candidate counter is incompatible", "候选存在语义不兼容的计数器"],
  candidate_metric_unknown: ["A candidate counter state is unknown", "候选存在状态未知的计数器"],
};

function packsEqual(left: SelectedVersionPack | null, right: SelectedVersionPack | null): boolean {
  if (!left || !right) return false;
  return (
    left.pack_id === right.pack_id
    && left.product_pack_version === right.product_pack_version
    && left.manifest_hash === right.manifest_hash
    && (left.release_deltas?.release ?? null) === (right.release_deltas?.release ?? null)
    && (left.release_deltas?.deltas_hash ?? null) === (right.release_deltas?.deltas_hash ?? null)
  );
}

/**
 * Fix #1: Version-pack comparability label driven by backend comparability reasons,
 * compatibility deltas, comparison state, and exact pack identity equality.
 */
export function versionPackStatusLabel(comparison: Comparison, text: LocalizedText): string {
  const codes = new Set([...comparison.reasons, ...comparison.compatibility_delta]);
  if (codes.has("version_pack_mismatch")) {
    return text("Version pack mismatch", "版本包不匹配");
  }
  if (codes.has("starrocks_version_mismatch")) {
    return text("StarRocks versions differ", "StarRocks 版本不一致");
  }
  if (codes.has("registry_semantic_mismatch")) {
    return text("Metric registry mismatch", "指标注册表语义不兼容");
  }

  const baseline = reportOf(comparison.baseline);
  const candidate = reportOf(comparison.candidate);
  if (!baseline || !candidate) {
    return comparison.state === "not_comparable"
      ? text("Version pack not comparable", "版本包不可比较")
      : text("Version pack not matched on both sides", "版本包未在两侧都匹配");
  }

  if (!hasVersionPack(baseline.version) || !hasVersionPack(candidate.version)) {
    return text("Version pack not matched on both sides", "版本包未在两侧都匹配");
  }

  const leftVersion = baseline.version.effective_version ?? baseline.version.reported_version?.value ?? null;
  const rightVersion = candidate.version.effective_version ?? candidate.version.reported_version?.value ?? null;
  if (leftVersion !== null && rightVersion !== null && leftVersion !== rightVersion) {
    return text("StarRocks versions differ", "StarRocks 版本不一致");
  }

  if (!packsEqual(baseline.version.selected_pack, candidate.version.selected_pack)) {
    return text("Version pack mismatch", "版本包不匹配");
  }

  if (comparison.state === "not_comparable") {
    return text("Version pack not comparable", "版本包不可比较");
  }

  return text("Version pack matched", "版本包匹配");
}

/**
 * Fix #2: Missing percentage badge driven by side-specific reason codes
 * (`baseline_metric_*` / `candidate_metric_*`), null cell refs, and non-exact cell states.
 */
export function missingObservationLabel(
  delta: MetricDelta,
  baselineCell: MetricCell | undefined,
  candidateCell: MetricCell | undefined,
  comparisonReasons: readonly string[],
  text: LocalizedText,
): string {
  if (delta.limitations.includes("parallelism_shape_unavailable")) {
    return text("Parallelism unknown", "并行度未知");
  }

  const hasPrefix = (codes: readonly string[], prefix: "baseline_metric_" | "candidate_metric_") =>
    codes.some((code) => code.startsWith(prefix));

  const deltaBase = hasPrefix(delta.limitations, "baseline_metric_");
  const deltaCand = hasPrefix(delta.limitations, "candidate_metric_");
  if (deltaBase && deltaCand) return text("Missing on both sides", "两侧均缺少观测值");
  if (deltaBase) return text("No baseline observation", "缺少基线观测值");
  if (deltaCand) return text("No candidate observation", "缺少候选观测值");

  const noBaseRef = delta.baseline_metric_cell_ref === null;
  const noCandRef = delta.candidate_metric_cell_ref === null;
  if (noBaseRef && noCandRef) return text("Missing on both sides", "两侧均缺少观测值");
  if (noBaseRef) return text("No baseline observation", "缺少基线观测值");
  if (noCandRef) return text("No candidate observation", "缺少候选观测值");

  const baseNonExact = baselineCell ? baselineCell.state !== "exact" : false;
  const candNonExact = candidateCell ? candidateCell.state !== "exact" : false;
  if (baseNonExact && candNonExact) return text("Missing on both sides", "两侧均缺少观测值");
  if (baseNonExact && !candNonExact) return text("No baseline observation", "缺少基线观测值");
  if (candNonExact && !baseNonExact) return text("No candidate observation", "缺少候选观测值");

  if (!baselineCell && candidateCell) return text("No baseline observation", "缺少基线观测值");
  if (baselineCell && !candidateCell) return text("No candidate observation", "缺少候选观测值");

  const reasonBase = hasPrefix(comparisonReasons, "baseline_metric_");
  const reasonCand = hasPrefix(comparisonReasons, "candidate_metric_");
  if (reasonBase && reasonCand) return text("Missing on both sides", "两侧均缺少观测值");
  if (reasonBase) return text("No baseline observation", "缺少基线观测值");
  if (reasonCand) return text("No candidate observation", "缺少候选观测值");

  return text("Missing observation", "缺少观测值");
}

/**
 * Renders one backend `Comparison` on the v1.1 responsive card grid (`05-compare.html`).
 * Before a comparison exists the host shows `OfflineCompareForm` or `PendingComparison`,
 * so this component renders nothing.
 */
export function CompareWorkspace({
  comparison,
  baselineOrigin = null,
  candidateOrigin = null,
  connectedComparison = null,
  planComparison = null,
  initialTab = "metrics",
  initialFindingKey = null,
  initialLimitationsOpen = false,
}: CompareWorkspaceProps): JSX.Element | null {
  if (!comparison) return null;

  return (
    <CompareResultView
      baselineOrigin={baselineOrigin}
      candidateOrigin={candidateOrigin}
      comparison={comparison}
      connectedComparison={connectedComparison}
      initialFindingKey={initialFindingKey}
      initialLimitationsOpen={initialLimitationsOpen}
      initialTab={initialTab}
      key={comparison.comparison_key}
      planComparison={planComparison}
    />
  );
}

function CompareResultView({
  comparison,
  baselineOrigin,
  candidateOrigin,
  connectedComparison,
  planComparison,
  initialTab,
  initialFindingKey,
  initialLimitationsOpen,
}: {
  comparison: Comparison;
  baselineOrigin: InputOrigin | null;
  candidateOrigin: InputOrigin | null;
  connectedComparison: ConnectedProfileComparisonResponse | null;
  planComparison: PlanComparison | null;
  initialTab: CompareTab;
  initialFindingKey: string | null;
  initialLimitationsOpen: boolean;
}): JSX.Element {
  const { text } = useLocale();
  const [tab, setTab] = useState<CompareTab>(initialTab);
  const [drawerState, dispatchDrawer] = useReducer(reduceCompareDrawerState, null, (): CompareDrawerState => {
    if (initialFindingKey) return { comparisonKey: comparison.comparison_key, kind: "finding", itemKey: initialFindingKey };
    if (initialLimitationsOpen) return { comparisonKey: comparison.comparison_key, kind: "limitations" };
    return null;
  });
  const baseline = reportOf(comparison.baseline);
  const candidate = reportOf(comparison.candidate);
  const cells = useMemo<CellMaps>(() => ({ baseline: cellsOf(baseline), candidate: cellsOf(candidate) }), [baseline, candidate]);
  const rows = useMemo<RowMaps>(() => ({ baseline: rowsOf(baseline), candidate: rowsOf(candidate) }), [baseline, candidate]);

  const queryDeltas = orderQueryDeltas(comparison.metric_deltas.filter((delta) => delta.subject_match === "query"));
  const operatorDeltas = comparison.metric_deltas.filter((delta) => delta.subject_match !== "query");
  const structureChangeCount =
    comparison.structure_delta.changed.length
    + comparison.structure_delta.added.length
    + comparison.structure_delta.removed.length
    + comparison.structure_delta.edge_delta.length;

  const tabItems: SegItem<CompareTab>[] = [
    { value: "metrics", label: text("Query metrics", "查询指标"), count: queryDeltas.length },
    ...(operatorDeltas.length > 0
      ? [{ value: "operators" as const, label: text("Operator counters", "算子计数"), count: operatorDeltas.length }]
      : []),
    { value: "structure", label: text("Structure diff", "结构差异"), count: structureChangeCount },
    ...(planComparison ? [{ value: "plan" as const, label: text("Plan check", "计划检查") }] : []),
    { value: "provenance", label: text("Raw / Provenance", "原始数据 / 溯源") },
  ];

  const activeTab: CompareTab = tabItems.some((item) => item.value === tab) ? tab : "metrics";
  const comparableQueryCount = queryDeltas.filter((delta) => delta.state === "comparable").length;
  const limitations = extraLimitations(comparison, connectedComparison?.limitations ?? []);
  const findingData = buildFindingChangesView(baseline, candidate, rows, comparison, text);
  const currentDrawer = activeCompareDrawer(drawerState, comparison.comparison_key);
  const activeFindingItem = currentDrawer?.kind === "finding"
    ? findingData.items.find((item) => item.key === currentDrawer.itemKey) ?? null
    : null;
  const limitationsOpen = currentDrawer?.kind === "limitations" && limitations.length > 0;

  return (
    <section
      aria-label={text("Profile comparison workspace", "Profile 对比工作区")}
      className="workspace-stage cmp-scene compare-workspace"
    >
      <PageGrid
        aria-label={text("Compared Profiles and comparability", "对比对象与可比性判定")}
        className="page-grid--auto cmp-top-grid"
      >
        <ProfileSlot
          attempt={comparison.baseline}
          origin={baselineOrigin}
          query={connectedComparison?.baseline.query ?? null}
          side="baseline"
        />
        <ProfileSlot
          attempt={comparison.candidate}
          origin={candidateOrigin}
          query={connectedComparison?.candidate.query ?? null}
          side="candidate"
        />
        <ComparabilityPanel comparison={comparison} connectedComparison={connectedComparison} />
      </PageGrid>

      <BlockingNoticeBanner comparison={comparison} />

      <div className="workbench-split cmp-workbench">
        <section aria-label={text("Comparison details", "对比明细视图")} className="card card--table cmp-main-card">
          <header className="card-hd">
            <SegTabs<CompareTab>
              aria-label={text("Comparison views", "对比明细标签页")}
              items={tabItems}
              onValueChange={setTab}
              size="sm"
              value={activeTab}
            />
            <div className="card-hd-actions">
              <span className="card-subtitle hide-lt-1440">
                {text("Green = down · orange = up · % relative to baseline", "下降 · 上升 · 百分比相对基线")}
              </span>
              <InfoTip
                label={text("Comparison units and zero-baseline rules", "对比计量与基线零值规则说明")}
                side="left"
              >
                {text(
                  "Bytes use StarRocks binary units (1 MB = 1,048,576 B). A zero baseline gives only the absolute change (percentage is undefined); missing or invalid values are never treated as 0; incompatible parallelism or version semantics are marked not comparable.",
                  "字节沿用 StarRocks 二进制口径（1 MB = 1,048,576 B）。基线 = 0 时仅给绝对变化，百分比为未定义；缺失或无效值绝不按 0 计算；并行度或版本语义不兼容显式标为 ≠ 不可比较。",
                )}
              </InfoTip>
            </div>
          </header>

          <div className="card-bd card-bd--flush cmp-main-body">
            {activeTab === "metrics" ? (
              <QueryMetricTable cells={cells} comparisonReasons={comparison.reasons} deltas={queryDeltas} />
            ) : null}
            {activeTab === "operators" ? (
              <OperatorCounters
                cells={cells}
                comparisonReasons={comparison.reasons}
                deltas={operatorDeltas}
                reports={{ baseline, candidate }}
                rows={rows}
              />
            ) : null}
            {activeTab === "structure" ? (
              <StructureDiff baseline={baseline} candidate={candidate} comparison={comparison} />
            ) : null}
            {activeTab === "plan" && planComparison ? (
              <PlanCheck planComparison={planComparison} />
            ) : null}
            {activeTab === "provenance" ? (
              <ProvenancePanel baseline={baseline} candidate={candidate} comparison={comparison} />
            ) : null}
          </div>

          <footer className="card-ft">
            <div className="cmp-ft-summary">
              <span>
                {text(
                  `${queryDeltas.length} query metrics · ${comparableQueryCount} comparable`,
                  `共 ${queryDeltas.length} 项查询指标 · ${comparableQueryCount} 项可直接比较`,
                )}
              </span>
              <InfoTip label={text("Zero-baseline and redaction boundary", "对比口径与导出脱敏边界")}>
                {text(
                  "A zero baseline gives only the absolute change; missing and invalid values are never treated as 0.",
                  "基线为 0 时只给绝对变化；缺失与无效值绝不按 0 计算。",
                )}
              </InfoTip>
            </div>
            <span className="cmp-export-privacy">
              {text(
                "Exported reports contain no original Profile text, SQL literals or passwords.",
                "导出报告不含原始 Profile 文本、SQL 字面量或连接口令。",
              )}
            </span>
          </footer>
        </section>

        <aside
          aria-label={text("Finding changes", "发现项变化")}
          className="cmp-findings-column"
        >
          <FindingChanges
            activeItemKey={activeFindingItem?.key ?? null}
            data={findingData}
            limitationsCount={limitations.length}
            onOpenFinding={(itemKey) =>
              dispatchDrawer({ type: "open_finding", comparisonKey: comparison.comparison_key, itemKey })
            }
            onOpenLimitations={() =>
              dispatchDrawer({ type: "open_limitations", comparisonKey: comparison.comparison_key })
            }
          />
        </aside>
      </div>

      <FindingChangeDrawer
        baseline={baseline}
        candidate={candidate}
        item={activeFindingItem}
        onClose={() => dispatchDrawer({ type: "close" })}
      />

      <LimitationsDrawer
        limitations={limitations}
        onClose={() => dispatchDrawer({ type: "close" })}
        open={limitationsOpen}
      />
    </section>
  );
}

/* ---------- Pending baseline ---------- */

/**
 * A baseline was set from a pulled Profile; the candidate is still to be
 * picked from the same query list (`05-compare.html` scene-pending).
 */
export function PendingComparison({
  baseline,
  onPickCandidate,
  onClear,
}: {
  baseline: { query: ConnectedQueryRef; report: AnalysisSnapshotReport | null };
  onPickCandidate?(): void;
  onClear?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const clock = baseline.query.observed_at_utc ? /T(\d\d:\d\d:\d\d)/.exec(baseline.query.observed_at_utc)?.[1] ?? null : null;
  const source = [
    text("Fetched from the database", "从数据库拉取"),
    clock ? `${clock} UTC` : null,
    baseline.report ? text(`${baseline.report.findings.length} findings`, `${baseline.report.findings.length} 条发现`) : null,
  ].filter(Boolean).join(" · ");
  const version = baseline.report?.version.effective_version ?? baseline.report?.version.reported_version?.value ?? null;
  const pack = baseline.report?.version.selected_pack
    ? `${baseline.report.version.selected_pack.pack_id}@${baseline.report.version.selected_pack.product_pack_version}`
    : null;
  const evidenceState = baseline.report?.completeness.summary.state ?? "unavailable";
  const evidenceLabel = `${text("Evidence", "证据")} ${localizedEnum(evidenceState, text)}`;

  return (
    <section
      aria-label={text("Comparison waiting for a candidate", "等待候选的对比")}
      className="workspace-stage cmp-scene compare-pending"
    >
      <PageGrid className="page-grid--auto">
        <Card
          actions={
            baseline.query.discovery_generation !== null ? (
              <span className="badge-stamp">
                {text(`Generation #${baseline.query.discovery_generation}`, `发现代次 #${baseline.query.discovery_generation}`)}
              </span>
            ) : undefined
          }
          footer={
            <>
              <span className="mono">{shortQueryId(baseline.query.query_id)}</span>
              {onClear ? (
                <Button onClick={onClear} size="sm" variant="secondary">
                  {text("Clear baseline", "清除基线")}
                </Button>
              ) : null}
            </>
          }
          span={{ lg: 5, md: 8, sm: 4 }}
          title={text("Before optimization · baseline", "优化前 · 基线")}
          variant="detail"
        >
          <div className="detail-stack">
            <div className="cmp-slot-top">
              <code className="cmp-qid" title={baseline.query.query_id}>
                {shortQueryId(baseline.query.query_id)}
              </code>
              {baseline.report ? (
                <span className="cmp-slot-evidence">
                  <EvidenceMeter label={evidenceLabel} level={toEvidenceLevel(evidenceState)} />
                  <span className="card-subtitle">{evidenceLabel}</span>
                </span>
              ) : null}
            </div>
            <dl className="kv-grid">
              <dt>{text("Capture source", "捕获来源")}</dt>
              <dd>{source}</dd>
              {version ? (
                <>
                  <dt>{text("Engine & pack", "引擎与版本包")}</dt>
                  <dd className="mono">StarRocks {version}{pack ? ` · ${pack}` : ""}</dd>
                </>
              ) : null}
              {baseline.query.digest ? (
                <>
                  <dt>{text("SQL digest", "SQL 指纹")}</dt>
                  <dd className="mono">{baseline.query.digest}</dd>
                </>
              ) : null}
            </dl>
          </div>
        </Card>

        <Card
          actions={
            <span className="state-stamp">
              {text("Waiting for another execution", "等待同一查询族另一次执行")}
            </span>
          }
          footer={
            <>
              <span>{text("Same connection session · read-only pairing", "同一连接会话 · 只读配对")}</span>
              {onPickCandidate ? (
                <Button onClick={onPickCandidate} size="sm" variant="primary">
                  {text("Pick the candidate", "去选择候选")}
                </Button>
              ) : null}
            </>
          }
          span={{ lg: 7, md: 8, sm: 4 }}
          title={text("After change · candidate", "调整后 · 候选")}
          variant="detail"
        >
          <div className="detail-stack">
            <div className="notice-banner">
              <span>
                {text(
                  "Controlled before/after verification pairs two real executions in the same connection session without automatically modifying SQL or session variables.",
                  "受控前后验证仅在同一连接会话与发现代次内配对两次真实执行，不自动修改 SQL 或会话参数。",
                )}
              </span>
            </div>
            <p className="cmp-pending-copy">
              {text(
                "Run the changed query once (SQL workspace · Runtime Top), refresh the audit history, then select that execution in the same query family and choose “Compare with baseline”.",
                "修改后执行一次（SQL 工作区 · 运行时 Top），刷新审计历史，在同一查询族中选中那次执行，点“与基线对比”。",
              )}
            </p>
          </div>
        </Card>
      </PageGrid>
    </section>
  );
}

/* ---------- Compared Profiles & Comparability ---------- */

function ProfileSlot({
  attempt,
  origin,
  query,
  side,
}: {
  attempt: AnalysisAttempt;
  origin: InputOrigin | null;
  query: ConnectedQueryRef | null;
  side: Side;
}): JSX.Element {
  const { text } = useLocale();
  const report = reportOf(attempt);
  const label = side === "baseline"
    ? text("Before optimization · baseline", "优化前 · 基线槽")
    : text("After change · candidate", "调整后 · 候选槽");
  const queryId = query?.query_id ?? report?.query_summary?.query_id ?? null;
  const clock = query?.observed_at_utc ? /T(\d\d:\d\d:\d\d)/.exec(query.observed_at_utc)?.[1] ?? null : null;
  const source = [
    origin ? originLabel(origin, text) : null,
    clock ? `${clock} UTC` : null,
    report ? text(`${report.findings.length} findings`, `${report.findings.length} 条发现`) : null,
  ].filter(Boolean).join(" · ");
  const failed = "error" in attempt && attempt.error ? attempt.error : null;
  const version = report?.version.effective_version ?? report?.version.reported_version?.value ?? null;
  const evidenceState = report?.completeness.summary.state ?? "unavailable";
  const evidenceLabel = `${text("Evidence", "证据")} ${localizedEnum(evidenceState, text)}`;
  const evidenceReasons = report?.completeness.summary.reasons.map((reason) => localizedEnum(reason, text)).join(" · ") ?? "";

  return (
    <article
      aria-label={label}
      className={`card cmp-slot-card ${gridSpanClasses({ lg: 4, md: 4, sm: 4 })}`}
    >
      <div className="cmp-slot-top">
        <span className="cmp-slot-label">
          <span
            aria-hidden="true"
            className={side === "baseline" ? "status-dot status-dot--hollow" : "status-dot status-dot--ok"}
          />
          {label}
        </span>
        {report ? (
          <span className="cmp-slot-evidence">
            <EvidenceMeter
              detail={evidenceReasons || undefined}
              label={evidenceLabel}
              level={toEvidenceLevel(evidenceState)}
            />
            <span className="card-subtitle">{evidenceLabel}</span>
          </span>
        ) : null}
      </div>

      <div className="cmp-slot-main">
        {queryId ? (
          <code className="cmp-qid" title={queryId}>{shortQueryId(queryId)}</code>
        ) : (
          <span className="state-stamp">{text("No query ID", "无查询 ID")}</span>
        )}
        <div className="cmp-slot-stamps">
          {failed ? (
            <>
              <span className="state-stamp state-stamp--critical">! {text("Analysis failed", "分析失败")}</span>
              <code className="cmp-code">{failed.code}</code>
            </>
          ) : (
            <>
              {report && report.version.compatibility !== "compatible" ? (
                <span className="state-stamp state-stamp--warn">{localizedEnum(report.version.compatibility, text)}</span>
              ) : null}
              {report?.analysis_state === "partial" ? (
                <span className="state-stamp state-stamp--warn">◐ {text("Partial analysis", "分析不完整")}</span>
              ) : version ? (
                <span className="badge-stamp">StarRocks {version}</span>
              ) : null}
            </>
          )}
        </div>
      </div>

      {source ? <div className="card-subtitle">{source}</div> : null}
    </article>
  );
}

function ComparabilityPanel({
  comparison,
  connectedComparison,
}: {
  comparison: Comparison;
  connectedComparison: ConnectedProfileComparisonResponse | null;
}): JSX.Element {
  const { text } = useLocale();
  const reports = [reportOf(comparison.baseline), reportOf(comparison.candidate)];
  const versions = reports.map((report) => report?.version.effective_version ?? report?.version.reported_version?.value ?? null);
  const packs = reports.map((report) =>
    report?.version.selected_pack
      ? `${report.version.selected_pack.pack_id}@${report.version.selected_pack.product_pack_version}`
      : text("no pack", "无版本包"),
  );
  const sameVersion = versions[0] !== null && versions[0] === versions[1];
  const packStatus = versionPackStatusLabel(comparison, text);
  const leftVer = versions[0] ?? text("unknown", "未知");
  const rightVer = versions[1] ?? text("unknown", "未知");

  const context = connectedComparison?.validation_context ?? null;
  const digests = [connectedComparison?.baseline.query.digest ?? null, connectedComparison?.candidate.query.digest ?? null];
  const digestText = digests[0] && digests[1]
    ? digests[0] === digests[1]
      ? text("same SQL digest", "同一 SQL 指纹")
      : text("different SQL digests", "SQL 指纹不同")
    : text("SQL digest unknown", "SQL 指纹未知");
  const connectionLine = connectedComparison
    ? `${context?.same_connection_session ? text("Same connection", "同一连接会话") : text("Different connection sessions", "不同连接会话")} · ${digestText}`
    : text("Two imported Profiles", "两个导入的 Profile");

  const semantic = comparison.metric_deltas.filter((delta) => delta.percentage_state === "semantic_mismatch").length;
  const reasons = countedCodes(comparison.reasons);

  return (
    <article
      aria-label={text("Comparability", "可比性判定")}
      className={`card cmp-slot-card cmp-verdict-card ${gridSpanClasses({ lg: 4, md: 8, sm: 4 })}`}
      role="group"
    >
      <div className="cmp-slot-top">
        <span className="status-inline">
          <span aria-hidden="true" className={statusDotClass(comparison.state)} />
          <strong>{localizedEnum(comparison.state, text)}</strong>
        </span>
      </div>
      <ul className="cmp-reasons" data-readable-text="comparison-reasons">
        <li title={packs.join(" · ")}>
          {packStatus} · <code className="mono">{leftVer} {sameVersion ? "=" : "≠"} {rightVer}</code>
        </li>
        <li
          title={
            context
              ? `${localizedEnum(context.interpretation_state, text)} · ${context.fetch_order} · ${context.candidate_attestation}`
              : undefined
          }
        >
          {connectionLine}
        </li>
        {semantic > 0 ? (
          <li>{text(`${semantic} metric(s) differ in meaning and are excluded`, `${semantic} 项指标语义不同，已排除`)}</li>
        ) : null}
        {reasons.map(({ code, count }) => {
          const affected = comparison.metric_deltas.filter((delta) => delta.limitations.includes(code)).length;
          return (
            <li key={code}>
              {codeText(code, text)}
              {count > 1 ? <span className="mono"> ×{count}</span> : null}
              {affected > 0 ? <span className="cmp-affected"> · {text(`${affected} metrics`, `${affected} 项指标`)}</span> : null}
            </li>
          );
        })}
      </ul>
    </article>
  );
}

function BlockingNoticeBanner({ comparison }: { comparison: Comparison }): JSX.Element | null {
  const { text } = useLocale();
  const codes = new Set([...comparison.reasons, ...comparison.compatibility_delta]);
  const baseline = reportOf(comparison.baseline);
  const candidate = reportOf(comparison.candidate);
  const leftVer = baseline?.version.effective_version ?? baseline?.version.reported_version?.value ?? null;
  const rightVer = candidate?.version.effective_version ?? candidate?.version.reported_version?.value ?? null;
  const crossVersion = leftVer !== null && rightVer !== null && leftVer !== rightVer;
  const packMismatch =
    codes.has("version_pack_mismatch")
    || codes.has("starrocks_version_mismatch")
    || codes.has("registry_semantic_mismatch")
    || (baseline !== null
      && candidate !== null
      && hasVersionPack(baseline.version)
      && hasVersionPack(candidate.version)
      && !packsEqual(baseline.version.selected_pack, candidate.version.selected_pack));

  if (comparison.state !== "not_comparable" && !crossVersion && !packMismatch) {
    return null;
  }

  const reasonSummary = countedCodes(comparison.reasons)
    .map(({ code }) => codeText(code, text))
    .join(" · ");

  return (
    <div className="notice-banner notice-banner--danger cmp-blocking-banner" role="alert">
      <div>
        <strong>
          {crossVersion || packMismatch
            ? text("Cross-version or semantic incompatibility", "跨版本语义阻断")
            : text("Not comparable", "不可比较")}
        </strong>
        {" · "}
        {crossVersion && leftVer && rightVer ? (
          <span>
            {text(
              `Baseline StarRocks ${leftVer} and candidate StarRocks ${rightVer} differ in version or pack semantics; incompatible metrics are marked not comparable and never synthesized into percentages.`,
              `基线 StarRocks ${leftVer} 与候选 StarRocks ${rightVer} 版本或规则包语义不一致，拒绝合成跨版本变化率，不兼容指标显式标为 ≠ 不可比较。`,
            )}
          </span>
        ) : (
          <span>
            {reasonSummary || text(
              "Version pack or metric semantics are incompatible; metrics are marked not comparable.",
              "版本包或指标语义不兼容，相关指标显式标为 ≠ 不可比较。",
            )}
          </span>
        )}
      </div>
    </div>
  );
}

/* ---------- Metric tables ---------- */

function QueryMetricTable({
  cells,
  deltas,
  comparisonReasons,
}: {
  cells: CellMaps;
  deltas: MetricDelta[];
  comparisonReasons: readonly string[];
}): JSX.Element {
  const { text } = useLocale();
  if (!deltas.length) {
    return (
      <div className="state-box compare-query-metrics">
        <span>{text("The comparison returned no query-level metrics.", "对比结果没有查询级指标。")}</span>
      </div>
    );
  }
  return (
    <div className="compare-query-metrics">
      <MetricTable cells={cells} comparisonReasons={comparisonReasons} deltas={deltas} variant="query" />
    </div>
  );
}

function MetricTable({
  cells,
  deltas,
  comparisonReasons,
  variant,
}: {
  cells: CellMaps;
  deltas: MetricDelta[];
  comparisonReasons: readonly string[];
  variant: "query" | "operator";
}): JSX.Element {
  const { text } = useLocale();
  return (
    <table className={`data-table cmp-metric-table ${variant}`}>
      <thead>
        <tr>
          <th scope="col">{text("Metric", "指标")}</th>
          <th className="num" scope="col">{text("Before", "基线")}</th>
          <th className="num" scope="col">{text("After", "候选")}</th>
          <th className="num" scope="col">{text("Absolute change", "绝对变化")}</th>
          <th className="num" scope="col">{text("% vs baseline", "相对基线")}</th>
          <th className="cmp-bar-col" scope="col">
            <span className="sr-only">{text("Change bar", "变化示意")}</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {deltas.map((delta, index) => (
          <MetricRow
            cells={cells}
            comparisonReasons={comparisonReasons}
            delta={delta}
            key={`${delta.metric_id}:${delta.baseline_metric_cell_ref ?? "-"}:${delta.candidate_metric_cell_ref ?? "-"}:${index}`}
            variant={variant}
          />
        ))}
      </tbody>
    </table>
  );
}

function MetricRow({
  cells,
  delta,
  comparisonReasons,
  variant,
}: {
  cells: CellMaps;
  delta: MetricDelta;
  comparisonReasons: readonly string[];
  variant: "query" | "operator";
}): JSX.Element {
  const { text } = useLocale();
  const baseline = delta.baseline_metric_cell_ref ? cells.baseline.get(delta.baseline_metric_cell_ref) : undefined;
  const candidate = delta.candidate_metric_cell_ref ? cells.candidate.get(delta.candidate_metric_cell_ref) : undefined;
  const unit = exactUnit(baseline) ?? exactUnit(candidate);
  const key = matchKeyParts(baseline?.match_key ?? candidate?.match_key);
  const ratio = delta.percentage_state === "computed"
    ? delta.percentage_value
    : delta.percentage_state === "approximate" && delta.percentage_bounds
      ? provenBound(delta.percentage_bounds)
      : null;
  const bar = divergingBar(ratio);
  const comparable = delta.state === "comparable";
  const displayName = variant === "query" ? queryMetricLabel(delta.metric_id, text) : metricLabel(delta.metric_id, text);

  return (
    <tr>
      <th className="cmp-metric-th" scope="row">
        <MetricName
          label={<strong className="cmp-metric-title" title={delta.metric_id}>{displayName}</strong>}
          official={delta.metric_id}
        />
        {variant === "operator" && key ? (
          <span className="cmp-metric-hint">
            <code>{key.counter}</code>
            {key.scope ? <code className="cmp-metric-scope">{key.scope}</code> : null}
          </span>
        ) : null}
      </th>
      <td className="num">{cellValue(baseline, delta.baseline_metric_cell_ref !== null, text)}</td>
      <td className={`num${comparable ? "" : " cmp-num-muted"}`}>{cellValue(candidate, delta.candidate_metric_cell_ref !== null, text)}</td>
      <td className="num">{absoluteChange(delta, unit, text)}</td>
      <td className="num">{percentChange(delta, baseline, candidate, comparisonReasons, text)}</td>
      <td className="cmp-bar-col">
        <div aria-hidden="true" className="cmp-diverge">
          {bar ? (
            <div
              className={`cmp-diverge-fill cmp-diverge-fill--${bar.direction}${bar.clamped ? " is-clamped" : ""}`}
              style={{ left: `${bar.left}%`, width: `${bar.width}%` }}
            />
          ) : null}
        </div>
      </td>
    </tr>
  );
}

/** Query metrics use the Doctor summary's names so both workspaces read identically. */
function queryMetricLabel(metricId: string, text: LocalizedText): string {
  switch (metricId) {
    case "QueryTotalTime":
      return text("Total time", "总耗时");
    case "QueryExecutionWallTime":
      return text("Execution (wall)", "执行（墙钟）");
    case "QueryCumulativeCpuTime":
      return text("CPU (sum)", "CPU 累计");
    default:
      return metricLabel(metricId, text);
  }
}

function cellValue(cell: MetricCell | undefined, referenced: boolean, text: LocalizedText): ReactNode {
  if (!cell) {
    return (
      <span className="state-stamp">
        {referenced ? text("Unavailable", "不可用") : `∅ ${text("Missing · not 0", "缺失 · 非 0")}`}
      </span>
    );
  }
  if (cell.state !== "exact") {
    return (
      <StateBadge tone={cell.state === "invalid" ? "critical" : cell.state === "partial" ? "warn" : undefined}>
        {localizedEnum(cell.state, text)}
      </StateBadge>
    );
  }
  const fidelity = cell.fidelity === "exact" ? "" : ` · ${localizedEnum(cell.fidelity, text)}`;
  return <span title={`${cell.display_text}${fidelity}`}>{canonicalValueText(cell.canonical_number, cell.canonical_unit)}</span>;
}

function signTone(value: CanonicalNumber | null): "down" | "up" | "none" {
  if (!value) return "none";
  const number = Number(value.value);
  return number < 0 ? "down" : number > 0 ? "up" : "none";
}

function boundsTone(bounds: BoundedInterval): "down" | "up" | "none" {
  if (intervalContainsZero(bounds)) return "none";
  return decimalSign(bounds.lower.value) >= 0 ? "up" : "down";
}

function deltaToneClass(tone: "down" | "up" | "none"): string {
  if (tone === "down") return "delta-down";
  if (tone === "up") return "delta-up";
  return "cmp-delta-none";
}

function absoluteChange(delta: MetricDelta, unit: string | null, text: LocalizedText): ReactNode {
  if (delta.state !== "comparable") {
    const short = delta.limitations.includes("metric_missing_on_one_side")
      ? text("Unavailable", "不可用")
      : `≠ ${text("Not comparable", "不可比较")}`;
    const primary = notComparableText(delta, text);
    const details = delta.limitations
      .map((code) => codeText(code, text))
      .filter((item) => item !== primary);
    return (
      <StateBadge
        title={[primary, ...details].join(" · ")}
        tone={delta.limitations.includes("parallelism_shape_mismatch") ? "warn" : undefined}
      >
        {short}
      </StateBadge>
    );
  }
  if (delta.absolute_delta) {
    const tone = signTone(delta.absolute_delta);
    return <span className={`cmp-delta ${deltaToneClass(tone)}`}>{signedValueText(delta.absolute_delta, unit)}</span>;
  }
  if (delta.bounds) {
    const interval = intervalText(delta.bounds, unit);
    if (intervalContainsZero(delta.bounds)) {
      return (
        <span className="cmp-no-change" title={`${text("Change interval", "变化区间")} ${interval}`}>
          {text("No change within display precision", "在显示精度内无变化")}
        </span>
      );
    }
    const tone = boundsTone(delta.bounds);
    return (
      <span className={`cmp-delta ${deltaToneClass(tone)}`} title={`${text("Change interval", "变化区间")} ${interval}`}>
        ≈ {approximateText(canonicalValueText(delta.bounds.lower, unit), canonicalValueText(delta.bounds.upper, unit))}
      </span>
    );
  }
  return <StateBadge>{text("Unavailable", "不可用")}</StateBadge>;
}

function approximateText(lower: string, upper: string): string {
  return lower === upper ? lower : `${lower} … ${upper}`;
}

function notComparableText(delta: MetricDelta, text: LocalizedText): string {
  if (delta.limitations.includes("duplicate_metric_match")) return text("Ambiguous: several counters share this name", "多个计数器同名，无法唯一配对");
  if (delta.limitations.includes("metric_missing_on_one_side")) return text("Only on one side", "仅一侧存在");
  if (delta.limitations.includes("metric_delta_overflow")) return text("Overflow", "溢出");
  if (delta.limitations.includes("parallelism_shape_mismatch")) return text("Parallelism differs", "并行度不同");
  if (delta.limitations.includes("parallelism_shape_unavailable")) return text("Parallelism unknown", "并行度未知");
  if (delta.limitations.includes("version_pack_mismatch")) return text("Version pack mismatch", "版本包不匹配");
  if (delta.limitations.includes("starrocks_version_mismatch")) return text("StarRocks versions differ", "StarRocks 版本不一致");
  if (delta.limitations.includes("registry_semantic_mismatch")) return text("Metric registry mismatch", "指标注册表语义不兼容");
  return text("Not comparable", "不可比较");
}

function percentChange(
  delta: MetricDelta,
  baselineCell: MetricCell | undefined,
  candidateCell: MetricCell | undefined,
  comparisonReasons: readonly string[],
  text: LocalizedText,
): ReactNode {
  switch (delta.percentage_state) {
    case "computed":
      return delta.percentage_value ? (
        <span className={`cmp-delta ${deltaToneClass(signTone(delta.percentage_value))}`} title={ratioPercentFullText(delta.percentage_value)}>
          {ratioPercentText(delta.percentage_value)}
        </span>
      ) : (
        <Dash />
      );
    case "approximate": {
      if (!delta.percentage_bounds) return <Dash />;
      const interval = ratioIntervalText(delta.percentage_bounds);
      if (delta.bounds && intervalContainsZero(delta.bounds)) {
        return <Dash title={`${text("Percentage interval", "百分比区间")} ${interval}`} />;
      }
      const tone = boundsTone(delta.percentage_bounds);
      return (
        <span className={`cmp-delta ${deltaToneClass(tone)}`} title={`${text("Percentage interval", "百分比区间")} ${interval}`}>
          ≈ {approximateText(ratioPercentText(delta.percentage_bounds.lower), ratioPercentText(delta.percentage_bounds.upper))}
        </span>
      );
    }
    case "undefined_baseline_zero":
      return <StateBadge>{text("Undefined · baseline = 0", "未定义 · 基线 = 0")}</StateBadge>;
    case "undefined_baseline_interval_contains_zero":
      return <StateBadge>{text("Undefined · baseline range includes 0", "未定义 · 基线区间含 0")}</StateBadge>;
    case "missing":
      return <StateBadge>{missingObservationLabel(delta, baselineCell, candidateCell, comparisonReasons, text)}</StateBadge>;
    case "invalid":
      return <StateBadge tone="critical">{text("Invalid observation", "观测值无效")}</StateBadge>;
    case "semantic_mismatch":
      return (
        <StateBadge title={notComparableText(delta, text)} tone="warn">
          {delta.limitations.includes("duplicate_metric_match")
            ? text("No unique pairing", "无法唯一配对")
            : text("Meaning differs", "指标含义不同")}
        </StateBadge>
      );
    default:
      return <StateBadge>{localizedEnum(delta.percentage_state, text)}</StateBadge>;
  }
}

function StateBadge({ children, title, tone }: { children: ReactNode; title?: string; tone?: "critical" | "warn" }): JSX.Element {
  return (
    <span className={`state-stamp${tone ? ` state-stamp--${tone}` : ""}`} title={title}>
      {children}
    </span>
  );
}

function Dash({ title }: { title?: string }): JSX.Element {
  return <span className="cmp-dash" title={title}>—</span>;
}

/* ---------- Operator counters ---------- */

type OperatorGroup = { key: string; label: string | null; physical: string[]; deltas: MetricDelta[] };
type ReportPair = Record<Side, AnalysisSnapshotReport | null>;

function OperatorCounters({
  cells,
  deltas,
  comparisonReasons,
  reports,
  rows,
}: {
  cells: CellMaps;
  deltas: MetricDelta[];
  comparisonReasons: readonly string[];
  reports: ReportPair;
  rows: RowMaps;
}): JSX.Element {
  const { text } = useLocale();
  const groups = operatorGroups(deltas, rows, cells, reports);
  const labelTotals = new Map<string, number>();
  for (const group of groups) {
    if (group.label) labelTotals.set(group.label, (labelTotals.get(group.label) ?? 0) + 1);
  }
  const labelSeen = new Map<string, number>();

  return (
    <div className="compare-operators">
      {groups.map((group) => {
        const total = group.label ? labelTotals.get(group.label) ?? 1 : 1;
        const ordinal = group.label ? (labelSeen.get(group.label) ?? 0) + 1 : 1;
        if (group.label) labelSeen.set(group.label, ordinal);
        const notComparable = group.deltas.filter((delta) => delta.state !== "comparable").length;
        return (
          <details className="cmp-op-group" key={group.key}>
            <summary>
              <span className="cmp-op-summary-left">
                <span className="cmp-op-label mono">{group.label ?? text("Unlabelled subject", "未标注对象")}</span>
                {group.physical.length > 0 ? (
                  <span className="cmp-affected">{group.physical.join(" / ")}</span>
                ) : total > 1 ? (
                  <span
                    className="cmp-affected"
                    title={text(
                      "The same plan node is reported by more than one operator group, e.g. its sending and receiving fragments.",
                      "同一计划节点由多个算子组上报，例如发送端与接收端 Fragment。",
                    )}
                  >
                    {text(`group ${ordinal} of ${total}`, `第 ${ordinal}/${total} 组`)}
                  </span>
                ) : null}
              </span>
              <span className="cmp-op-summary-right">
                <span className="cmp-affected">{text(`${group.deltas.length} counters`, `${group.deltas.length} 个计数器`)}</span>
                {notComparable > 0 ? (
                  <span className="state-stamp">{text(`${notComparable} not comparable`, `${notComparable} 项不可比`)}</span>
                ) : null}
              </span>
            </summary>
            <MetricTable cells={cells} comparisonReasons={comparisonReasons} deltas={group.deltas} variant="operator" />
          </details>
        );
      })}
    </div>
  );
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

function operatorRowFor(delta: MetricDelta, rows: RowMaps): OperatorRow | null {
  const subject = delta.subject_match;
  if (subject) {
    const row = rows.baseline.byRef.get(subject) ?? rows.candidate.byRef.get(subject);
    if (row) return row;
  }
  return (
    (delta.baseline_metric_cell_ref ? rows.baseline.byCell.get(delta.baseline_metric_cell_ref) : undefined)
    ?? (delta.candidate_metric_cell_ref ? rows.candidate.byCell.get(delta.candidate_metric_cell_ref) : undefined)
    ?? null
  );
}

function operatorLabel(row: OperatorRow | null): string | null {
  if (!row) return null;
  return hasPlanNode(row.plan_node_id) ? `Plan ${row.plan_node_id} · ${row.operator_type}` : row.operator_type;
}

function hasPlanNode(planNodeId: string | null): planNodeId is string {
  return planNodeId !== null && planNodeId !== "-1";
}

/* ---------- Findings, limitations, structure ---------- */

type FindingKind = "resolved" | "added" | "persisting" | "changed" | "incomparable" | "algorithm_changed";

type FindingCountSlot = { kind: FindingKind; label: string; dotClass: string; countClass: string; count: number };
type FindingExtraSlot = { kind: FindingKind; label: string; dotClass: string; count: number };

type FindingChangeItem = {
  key: string;
  kind: FindingKind;
  label: string;
  dotClass: string;
  entry: string;
  title: string;
  subjectLabel: string | null;
  subjectTitle: string | undefined;
  fullObjectLabel: string | null;
  partialEvidence: boolean;
  leadText: string | undefined;
  measurementChanges: FindingMeasurementChange[];
  baselineFinding: Finding | null;
  candidateFinding: Finding | null;
  primaryFinding: Finding | null;
};

type FindingChangesView = {
  counts: FindingCountSlot[];
  extra: FindingExtraSlot[];
  items: FindingChangeItem[];
  partialCount: number;
};

function buildFindingChangesView(
  baseline: AnalysisSnapshotReport | null,
  candidate: AnalysisSnapshotReport | null,
  rows: RowMaps,
  comparison: Comparison,
  text: LocalizedText,
): FindingChangesView {
  const delta = comparison.finding_delta;
  const counts: FindingCountSlot[] = [
    { kind: "resolved", label: text("Resolved", "已消除"), dotClass: "status-dot status-dot--ok", countClass: "cmp-count-ok", count: delta.resolved.length },
    { kind: "added", label: text("New", "新增"), dotClass: "status-dot status-dot--warn", countClass: "cmp-count-warn", count: delta.added.length },
    { kind: "persisting", label: text("Persisting", "持续存在"), dotClass: "status-dot", countClass: "", count: delta.persisting.length },
    ...(delta.changed.length
      ? [{ kind: "changed" as const, label: text("Changed", "已变化"), dotClass: "status-dot status-dot--warn", countClass: "cmp-count-accent", count: delta.changed.length }]
      : []),
  ];
  const extra: FindingExtraSlot[] = [
    { kind: "incomparable", label: text("Not comparable", "不可比较"), dotClass: "status-dot status-dot--hollow", count: delta.incomparable.length },
    { kind: "algorithm_changed", label: text("Rule version changed", "规则版本变化"), dotClass: "status-dot status-dot--hollow", count: delta.algorithm_changed.length },
  ];
  const partial = new Set(delta.partial_evidence ?? []);
  const items: FindingChangeItem[] = [
    ...counts.map(({ kind, label, dotClass }) => ({ kind, label, dotClass })),
    ...extra.map(({ kind, label, dotClass }) => ({ kind, label, dotClass })),
  ].flatMap(({ kind, label, dotClass }) =>
    delta[kind].map((entry) => {
      const { baselineFinding, candidateFinding } = findingFor(baseline, candidate, kind, entry);
      const sides = kind === "resolved"
        ? [{ side: "baseline" as const, report: baseline, finding: baselineFinding }]
        : [
            { side: "candidate" as const, report: candidate, finding: candidateFinding },
            { side: "baseline" as const, report: baseline, finding: baselineFinding },
          ];
      const match = sides.find((entrySide) => entrySide.finding !== null) ?? null;
      const primaryFinding = match?.finding ?? null;
      const primaryReport = match?.report ?? null;
      const primarySide = match?.side ?? null;
      const measurementChanges = delta.measurement_changes.filter((change) => change.finding_entry === entry);
      const lead = measurementChanges[0] ?? null;
      const leadText = lead ? measurementChangeText(lead, text) : undefined;
      const title = primaryFinding ? findingTitleText(primaryFinding, text) : entry;
      let subjectLabel: string | null = null;
      let fullObjectLabel: string | null = null;
      let subjectDetail: string | null = null;
      if (primaryFinding) {
        if (!primaryFinding.subject_ref || primaryFinding.subject_ref === "query") {
          const wholeQuery = text("Whole query", "整条查询");
          subjectLabel = wholeQuery;
          fullObjectLabel = wholeQuery;
        } else {
          const graphLabel = primaryReport ? findingObjectLabel(primaryReport, primaryFinding) : null;
          const context = primaryReport ? findingContext(primaryReport, primaryFinding) : null;
          const planNodeId = context?.planNodeId ?? null;
          const planLabel = hasPlanNode(planNodeId) ? `Plan ${planNodeId}` : null;
          if (graphLabel !== null || planLabel !== null || context?.label) {
            subjectLabel = planLabel ?? context?.label ?? graphLabel;
            fullObjectLabel = graphLabel ?? context?.label ?? planLabel;
            subjectDetail = context?.label ?? graphLabel;
          } else {
            const otherSide: Side = primarySide === "candidate" ? "baseline" : "candidate";
            const row = primarySide
              ? rows[primarySide].byRef.get(primaryFinding.subject_ref)
                ?? (sides.some((entrySide) => entrySide.side === otherSide && entrySide.finding?.subject_ref === primaryFinding.subject_ref)
                  ? rows[otherSide].byRef.get(primaryFinding.subject_ref) ?? null
                  : null)
              : null;
            const fallbackLabel = operatorLabel(row);
            subjectLabel = fallbackLabel;
            fullObjectLabel = fallbackLabel;
            subjectDetail = fallbackLabel;
          }
        }
      }
      const subjectTitle = primaryFinding
        ? `${primaryFinding.rule_id}${subjectDetail ? ` · ${subjectDetail}` : ""}`
        : undefined;
      const partialEvidence = (kind === "resolved" || kind === "added") && partial.has(entry);
      return {
        key: `${kind}:${entry}`,
        kind,
        label,
        dotClass,
        entry,
        title,
        subjectLabel,
        subjectTitle,
        fullObjectLabel,
        partialEvidence,
        leadText,
        measurementChanges,
        baselineFinding,
        candidateFinding,
        primaryFinding,
      };
    }),
  );

  return { counts, extra, items, partialCount: partial.size };
}

function FindingChanges({
  activeItemKey,
  data,
  limitationsCount,
  onOpenFinding,
  onOpenLimitations,
}: {
  activeItemKey: string | null;
  data: FindingChangesView;
  limitationsCount: number;
  onOpenFinding(itemKey: string): void;
  onOpenLimitations(): void;
}): JSX.Element {
  const { text } = useLocale();
  const { counts, extra, items, partialCount } = data;

  return (
    <Card
      actions={
        limitationsCount > 0 ? (
          <Button
            aria-haspopup="dialog"
            onClick={onOpenLimitations}
            size="sm"
            variant="secondary"
          >
            {text(`Limitations ${limitationsCount}`, `比较限制 ${limitationsCount}`)}
          </Button>
        ) : undefined
      }
      className="cmp-findings-card"
      footer={
        partialCount > 0 ? (
          <span>
            {text(
              `${partialCount} resolved or new finding(s) rest on partial evidence`,
              `其中 ${partialCount} 项已消除或新增依据部分证据判定`,
            )}
          </span>
        ) : undefined
      }
      title={text("Finding changes", "发现项变化")}
      titleExtra={
        <InfoTip label={text("Finding change evaluation rules", "发现项变化判定规则")} side="left">
          {text(
            "A finding is called resolved or new only when both sides completed evaluating the same rule and object; entries with incomplete merged inputs are marked partial evidence.",
            "仅当两侧均完成同一规则与对象评估时才判定已消除或新增；部分合并输入不完整的条目显式标注“证据部分”。",
          )}
        </InfoTip>
      }
      variant="detail"
    >
      <div className="detail-stack">
        <div
          aria-label={text("Finding change counts", "发现变化统计")}
          className="cmp-stat-strip"
          role="group"
          style={{ gridTemplateColumns: `repeat(${counts.length}, minmax(0, 1fr))` }}
        >
          {counts.map(({ kind, label, countClass, count }) => (
            <div key={kind}>
              <span className="card-subtitle">{label}</span>
              <strong className={countClass || undefined}>{count}</strong>
            </div>
          ))}
        </div>

        {extra.some(({ count }) => count > 0) ? (
          <p className="card-subtitle cmp-extra-note">
            {extra
              .filter(({ count }) => count > 0)
              .map(({ label, count }) => `${label} ${count}`)
              .join(" · ")}
          </p>
        ) : null}

        {items.length ? (
          <ul aria-label={text("Finding change list", "发现项变化清单")} className="cmp-finding-list">
            {items.map((item) => {
              const active = activeItemKey === item.key;
              return (
                <li key={item.key}>
                  <button
                    aria-haspopup="dialog"
                    aria-label={`${item.label} · ${item.title}`}
                    className={`item-btn cmp-finding-item${active ? " is-selected" : ""}`}
                    onClick={() => onOpenFinding(item.key)}
                    title={item.leadText}
                    type="button"
                  >
                    <span className="item-row-top">
                      <span className="status-inline">
                        <span aria-hidden="true" className={item.dotClass} />
                        <strong>{item.label}</strong>
                        {" · "}
                        <span className="cmp-finding-cat">
                          {item.primaryFinding ? item.title : <code>{item.entry}</code>}
                        </span>
                      </span>
                      {item.subjectLabel ? (
                        <code className="cmp-finding-subject mono" title={item.subjectTitle}>
                          {item.subjectLabel}
                        </code>
                      ) : null}
                    </span>
                    {item.partialEvidence || item.leadText ? (
                      <span className="item-row-meta">
                        {item.partialEvidence ? (
                          <span
                            className="state-stamp state-stamp--warn"
                            title={text(
                              "Decided on partial evidence: the rule ran on both sides with no missing prerequisite, but some inputs were incomplete.",
                              "依据部分证据判定：两侧都完成了该规则评估且没有缺少前提，但部分输入不完整。",
                            )}
                          >
                            {text("Partial evidence", "证据部分")}
                          </span>
                        ) : null}
                        {item.leadText ? <span className="mono cmp-finding-lead">{item.leadText}</span> : null}
                      </span>
                    ) : null}
                  </button>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="card-subtitle cmp-empty-copy">{text("Neither Profile produced findings.", "两个 Profile 均没有发现项。")}</p>
        )}
      </div>
    </Card>
  );
}

function FindingChangeDrawer({
  baseline,
  candidate,
  item,
  onClose,
}: {
  baseline: AnalysisSnapshotReport | null;
  candidate: AnalysisSnapshotReport | null;
  item: FindingChangeItem | null;
  onClose(): void;
}): JSX.Element {
  const { text } = useLocale();
  const primary = item?.primaryFinding ?? null;
  const ruleLabel = item
    ? item.baselineFinding && item.candidateFinding && item.baselineFinding.rule_version !== item.candidateFinding.rule_version
      ? `${primary!.rule_id} (${item.baselineFinding.rule_version} → ${item.candidateFinding.rule_version})`
      : primary
        ? `${primary.rule_id}@${primary.rule_version}`
        : item.entry
    : "";
  const objectLabel = item?.fullObjectLabel ?? null;
  const findingLimitations = item
    ? Array.from(
        new Set(
          [item.baselineFinding, item.candidateFinding].flatMap((finding) =>
            finding ? finding.limitations.map((value) => localizedFindingText(finding.rule_id, "limitation", value, text)) : [],
          ),
        ),
      )
    : [];
  const expectedDirection = primary?.recommendation.expected_direction
    ? localizedFindingText(primary.rule_id, "expected_direction", primary.recommendation.expected_direction, text)
    : null;
  const unpairedSideMeasurements = item && item.measurementChanges.length === 0
    ? [
        ...(item.baselineFinding?.measurements.length
          ? [{ sideLabel: text("Baseline", "基线"), measurements: item.baselineFinding.measurements }]
          : []),
        ...(item.candidateFinding?.measurements.length
          ? [{ sideLabel: text("Candidate", "候选"), measurements: item.candidateFinding.measurements }]
          : []),
      ]
    : [];

  return (
    <Dialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      open={item !== null}
      placement="right"
      title={item ? item.title : ""}
      width="lg"
    >
      {item ? (
        <div className="detail-stack cmp-drawer-body" key={item.key}>
          <div className="cmp-drawer-meta">
            <div className="cmp-drawer-tags">
              <span className="status-inline">
                <span aria-hidden="true" className={item.dotClass} />
                <strong>{item.label}</strong>
              </span>
              {item.partialEvidence ? (
                <span className="state-stamp state-stamp--warn">{text("Partial evidence", "证据部分")}</span>
              ) : null}
              <code className="cmp-code">{ruleLabel}</code>
            </div>
            {objectLabel ? <div className="cmp-drawer-subject mono">{objectLabel}</div> : null}
          </div>

          {item.partialEvidence ? (
            <div className="notice-banner notice-banner--warn">
              <span>
                {text(
                  "Decided on partial evidence: the rule ran on both sides with no missing prerequisite, but some inputs were incomplete.",
                  "依据部分证据判定：两侧都完成了该规则评估且没有缺少前提，但部分输入不完整。",
                )}
              </span>
            </div>
          ) : null}

          <div className="cmp-dual-grid cmp-drawer-dual">
            <FindingSideSummary
              finding={item.baselineFinding}
              kind={item.kind}
              label={text("Baseline · before", "基线 · 优化前")}
              report={baseline}
              side="baseline"
            />
            <FindingSideSummary
              finding={item.candidateFinding}
              kind={item.kind}
              label={text("Candidate · after", "候选 · 调整后")}
              report={candidate}
              side="candidate"
            />
          </div>

          {item.measurementChanges.length > 0 ? (
            <section className="cmp-drawer-section">
              <h3 className="detail-sec-title">{text("Rule measurements", "规则度量对比")}</h3>
              <table className="data-table cmp-drawer-table">
                <thead>
                  <tr>
                    <th scope="col">{text("Measurement", "度量项")}</th>
                    <th className="num" scope="col">{text("Baseline", "基线")}</th>
                    <th className="num" scope="col">{text("Candidate", "候选")}</th>
                    <th scope="col">{text("Condition", "规则条件")}</th>
                  </tr>
                </thead>
                <tbody>
                  {item.measurementChanges.map((change) => {
                    const before = change.baseline_threshold_text;
                    const after = change.candidate_threshold_text;
                    const threshold = before === null || after === null || before === after ? after ?? before : `${before} → ${after}`;
                    return (
                      <tr key={change.measurement_id}>
                        <td>{measurementLabel(change.measurement_id, text)}</td>
                        <td className="num">
                          {change.baseline_display_text ?? <StateBadge>{text("absent", "无")}</StateBadge>}
                        </td>
                        <td className="num">
                          {change.candidate_display_text ?? <StateBadge>{text("absent", "无")}</StateBadge>}
                        </td>
                        <td className="mono cmp-threshold-cell">
                          {threshold ? measurementThreshold(threshold, text) : <Dash />}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </section>
          ) : unpairedSideMeasurements.length > 0 ? (
            <section className="cmp-drawer-section">
              <h3 className="detail-sec-title">{text("Observed rule measurements", "已观测规则度量")}</h3>
              {unpairedSideMeasurements.map(({ sideLabel, measurements }) => (
                <div className="cmp-provenance-box" key={sideLabel}>
                  <div className="field-label">{sideLabel}</div>
                  <dl className="kv-grid">
                    {measurements.map((measurement) => (
                      <Fragment key={measurement.measurement_id}>
                        <dt>
                          {measurementLabel(measurement.measurement_id, text)}
                          {measurement.operator_type ? ` · ${measurement.operator_type}` : ""}
                        </dt>
                        <dd className="mono">
                          {measurement.display_text}
                          {measurement.threshold_text ? ` · ${measurementThreshold(measurement.threshold_text, text)}` : ""}
                        </dd>
                      </Fragment>
                    ))}
                  </dl>
                </div>
              ))}
            </section>
          ) : null}

          {findingLimitations.length > 0 ? (
            <section className="cmp-drawer-section">
              <h3 className="detail-sec-title">{text("Rule limitations", "规则限制")}</h3>
              <ul className="cmp-reasons">
                {findingLimitations.map((limitation) => (
                  <li key={limitation}>{limitation}</li>
                ))}
              </ul>
            </section>
          ) : null}

          {expectedDirection ? (
            <section className="cmp-drawer-section">
              <h3 className="detail-sec-title">{text("Expected direction", "预期优化方向")}</h3>
              <p className="cmp-drawer-copy">{expectedDirection}</p>
            </section>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  );
}

function FindingSideSummary({
  finding,
  kind,
  label,
  report,
  side,
}: {
  finding: Finding | null;
  kind: FindingKind;
  label: string;
  report: AnalysisSnapshotReport | null;
  side: Side;
}): JSX.Element {
  const { text } = useLocale();
  if (!finding) {
    const provenNotTriggered =
      (kind === "resolved" && side === "candidate") || (kind === "added" && side === "baseline");
    const absentText = !report
      ? text("No analyzed Profile", "无已分析 Profile")
      : provenNotTriggered
        ? text("Not triggered", "未触发")
        : kind === "incomparable"
          ? text("Not comparable", "不可比较")
          : text("Unavailable", "不可用");
    return (
      <section className="cmp-provenance-box">
        <div className="field-label">{label}</div>
        <div>
          <StateBadge>{absentText}</StateBadge>
        </div>
      </section>
    );
  }

  const completenessDetail = finding.evidence_completeness_reasons
    .map((reason) => localizedFindingText(finding.rule_id, "evidence_completeness_reason", reason, text))
    .join(" · ") || undefined;
  const evidenceLabel = `${text("Evidence", "证据")} ${localizedEnum(finding.evidence_completeness, text)}`;

  return (
    <section className="cmp-provenance-box">
      <div className="field-label">{label}</div>
      <dl className="kv-grid">
        <dt>{text("Severity", "严重度")}</dt>
        <dd>{localizedEnum(finding.severity, text)}</dd>
        <dt>{text("Confidence", "置信度")}</dt>
        <dd>{localizedEnum(finding.confidence, text)}</dd>
        <dt>{text("Evidence", "证据完整度")}</dt>
        <dd title={completenessDetail}>
          <span className="cmp-slot-evidence">
            <EvidenceMeter
              detail={completenessDetail}
              label={evidenceLabel}
              level={toEvidenceLevel(finding.evidence_completeness)}
            />
            <span>{localizedEnum(finding.evidence_completeness, text)}</span>
          </span>
        </dd>
        <dt>{text("Rule version", "规则版本")}</dt>
        <dd className="mono">{finding.rule_version}</dd>
      </dl>
    </section>
  );
}

function measurementChangeText(change: FindingMeasurementChange, text: LocalizedText): string {
  const side = (value: string | null) => value ?? text("absent", "无");
  const before = change.baseline_threshold_text;
  const after = change.candidate_threshold_text;
  const threshold = before === null || after === null || before === after ? after ?? before : `${before} → ${after}`;
  return `${measurementLabel(change.measurement_id, text)} ${side(change.baseline_display_text)} → ${side(change.candidate_display_text)}${threshold ? ` · ${measurementThreshold(threshold, text)}` : ""}`;
}

function findingFor(
  baseline: AnalysisSnapshotReport | null,
  candidate: AnalysisSnapshotReport | null,
  kind: FindingKind,
  entry: string,
): { baselineFinding: Finding | null; candidateFinding: Finding | null } {
  if (kind === "algorithm_changed") {
    if (baseline && candidate) {
      for (const baseFinding of baseline.findings) {
        for (const candFinding of candidate.findings) {
          if (
            baseFinding.rule_id === candFinding.rule_id
            && (baseFinding.subject_ref ?? "") === (candFinding.subject_ref ?? "")
            && `${baseFinding.rule_id}:${baseFinding.subject_ref ?? ""}:${baseFinding.rule_version}->${candFinding.rule_version}` === entry
          ) {
            return { baselineFinding: baseFinding, candidateFinding: candFinding };
          }
        }
      }
    }
    const baselineFinding =
      baseline?.findings.find((finding) => {
        const prefix = `${finding.rule_id}:${finding.subject_ref ?? ""}:${finding.rule_version}->`;
        return entry.startsWith(prefix) && entry.length > prefix.length;
      }) ?? null;
    const candidateFinding =
      candidate?.findings.find((finding) => {
        const head = `${finding.rule_id}:${finding.subject_ref ?? ""}:`;
        const tail = `->${finding.rule_version}`;
        return entry.startsWith(head) && entry.endsWith(tail) && entry.length > head.length + tail.length;
      }) ?? null;
    return { baselineFinding, candidateFinding };
  }

  return {
    baselineFinding: baseline?.findings.find((finding) => `${finding.rule_id}:${finding.subject_ref ?? ""}` === entry) ?? null,
    candidateFinding: candidate?.findings.find((finding) => `${finding.rule_id}:${finding.subject_ref ?? ""}` === entry) ?? null,
  };
}

function extraLimitations(comparison: Comparison, connectedLimitations: readonly string[]): { code: string; count: number }[] {
  const shown = new Set(comparison.reasons);
  return countedCodes([...comparison.limitations, ...connectedLimitations].filter((code) => !shown.has(code)));
}

function LimitationsDrawer({
  limitations,
  onClose,
  open,
}: {
  limitations: { code: string; count: number }[];
  onClose(): void;
  open: boolean;
}): JSX.Element {
  const { text } = useLocale();
  return (
    <Dialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      open={open && limitations.length > 0}
      placement="right"
      title={text("Limitations", "比较限制")}
    >
      <ul className="cmp-reasons cmp-drawer-reasons" data-readable-text="comparison-limitations">
        {limitations.map(({ code, count }) => (
          <li key={code}>
            {codeText(code, text)}
            {count > 1 ? <span className="mono"> ×{count}</span> : null}
          </li>
        ))}
      </ul>
    </Dialog>
  );
}

type StructureRow = {
  key: string;
  node: ReactNode;
  baseline: ReactNode;
  candidate: ReactNode;
  change: string;
  stampClass: string;
  muted?: boolean;
};

function StructureDiff({
  baseline,
  candidate,
  comparison,
}: {
  baseline: AnalysisSnapshotReport | null;
  candidate: AnalysisSnapshotReport | null;
  comparison: Comparison;
}): JSX.Element {
  const { text } = useLocale();
  const delta = comparison.structure_delta;
  const nodeIn = (report: AnalysisSnapshotReport | null, id: string) =>
    report?.graph_view.nodes.find((node) => node.node_id === id) ?? null;
  const none = <span className="cmp-dash">—</span>;
  const edgeLabel = (id: string): ReactNode => {
    const edge = baseline?.graph_view.edges.find((item) => item.edge_id === id)
      ?? candidate?.graph_view.edges.find((item) => item.edge_id === id);
    if (!edge) return <code>{id}</code>;
    const from = nodeIn(baseline, edge.from) ?? nodeIn(candidate, edge.from);
    const to = nodeIn(baseline, edge.to) ?? nodeIn(candidate, edge.to);
    return <code>{nodeLabel(from, edge.from)} → {nodeLabel(to, edge.to)}</code>;
  };
  const inBaseline = (id: string) => baseline?.graph_view.edges.some((edge) => edge.edge_id === id) ?? false;
  const unconnected = Array.from(
    new Set([...(baseline?.graph_view.unconnected_node_ids ?? []), ...(candidate?.graph_view.unconnected_node_ids ?? [])]),
  );
  const unproven = text("Relation unproven", "关系未证实");

  const rows: StructureRow[] = [
    ...delta.changed.map((id) => {
      const before = nodeIn(baseline, id);
      const after = nodeIn(candidate, id);
      const kinds = changedFactKinds(before, after);
      return {
        key: `changed:${id}`,
        node: <code>{nodeLabel(after ?? before, id)}</code>,
        baseline: <code>{factText(before, kinds)}</code>,
        candidate: <code>{factText(after, kinds)}</code>,
        change: `${factChangeLabel(kinds, text)} · ${text("explicit", "显式")}`,
        stampClass: "badge-stamp",
      };
    }),
    ...delta.added.map((id) => ({
      key: `added:${id}`,
      node: <code>{nodeLabel(nodeIn(candidate, id), id)}</code>,
      baseline: none,
      candidate: <code>{factText(nodeIn(candidate, id), null)}</code>,
      change: text("Added in candidate", "候选新增"),
      stampClass: "badge-stamp",
    })),
    ...delta.removed.map((id) => ({
      key: `removed:${id}`,
      node: <code>{nodeLabel(nodeIn(baseline, id), id)}</code>,
      baseline: <code>{factText(nodeIn(baseline, id), null)}</code>,
      candidate: none,
      change: text("Removed in candidate", "候选中移除"),
      stampClass: "state-stamp state-stamp--warn",
    })),
    ...delta.edge_delta.map((id) => ({
      key: `edge:${id}`,
      node: edgeLabel(id),
      baseline: inBaseline(id) ? text("Present", "存在") : none,
      candidate: inBaseline(id) ? none : text("Present", "存在"),
      change: text("Relation differs", "关系不同"),
      stampClass: "badge-stamp",
    })),
    ...unconnected.map((id) => ({
      key: `unconnected:${id}`,
      node: <code>{nodeLabel(nodeIn(candidate, id) ?? nodeIn(baseline, id), id)}</code>,
      baseline: unproven,
      candidate: unproven,
      change: `≠ ${text("Not compared", "不参与比较")}`,
      stampClass: "state-stamp",
      muted: true,
    })),
    ...delta.unresolved.map((code) => ({
      key: `unresolved:${code}`,
      node: <span>{codeText(code, text) !== code ? codeText(code, text) : localizedComparisonText("detail", code, text)}</span>,
      baseline: none,
      candidate: none,
      change: `≠ ${text("Not compared", "不参与比较")}`,
      stampClass: "state-stamp",
      muted: true,
    })),
  ];

  return (
    <div className="compare-structure">
      <div className="cmp-structure-summary">
        <span>{localizedEnum(delta.state, text)}</span>
        {" · "}
        <span>
          {text("Matched", "已匹配")} <strong className="mono">{delta.matches.length}</strong>
        </span>
        {" · "}
        <span>{text("Only proven plan-node mappings are compared", "仅比较可证实的计划节点映射")}</span>
      </div>
      {rows.length ? (
        <table className="data-table compare-structure-table">
          <thead>
            <tr>
              <th scope="col">{text("Plan node", "计划节点")}</th>
              <th scope="col">{text("Baseline", "基线")}</th>
              <th scope="col">{text("Candidate", "候选")}</th>
              <th scope="col">{text("Change", "结构变化")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr className={row.muted ? "muted" : undefined} key={row.key}>
                <td>{row.node}</td>
                <td>{row.baseline}</td>
                <td>{row.candidate}</td>
                <td><span className={row.stampClass}>{row.change}</span></td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div className="state-box">
          <span>{text(`No structural change: ${delta.matches.length} plan nodes matched.`, `结构无变化：${delta.matches.length} 个计划节点已匹配。`)}</span>
        </div>
      )}
    </div>
  );
}

const FACT_LABELS: Record<GraphSqlFactKind, string> = {
  table: "TABLE",
  predicate: "PREDICATES",
  join_condition: "JOIN",
  fields: "FIELDS",
};

function changedFactKinds(before: GraphNode | null, after: GraphNode | null): GraphSqlFactKind[] {
  const values = (node: GraphNode | null, kind: GraphSqlFactKind) =>
    (node?.sql_facts ?? []).filter((fact) => fact.kind === kind).map((fact) => fact.value).sort().join("\u0000");
  return (Object.keys(FACT_LABELS) as GraphSqlFactKind[]).filter((kind) => values(before, kind) !== values(after, kind));
}

function factText(node: GraphNode | null, kinds: GraphSqlFactKind[] | null): string {
  const facts = node?.sql_facts ?? [];
  const shown = kinds ?? Array.from(new Set(facts.map((fact) => fact.kind)));
  if (!shown.length) return "—";
  return shown
    .map((kind) => `${FACT_LABELS[kind]}: ${facts.filter((fact) => fact.kind === kind).map((fact) => fact.value).join("; ") || "—"}`)
    .join(" · ");
}

function factChangeLabel(kinds: GraphSqlFactKind[], text: LocalizedText): string {
  if (kinds.length === 1 && kinds[0] === "predicate") return text("Predicate changed", "谓词变化");
  if (kinds.length === 1 && kinds[0] === "join_condition") return text("Join condition changed", "关联条件变化");
  if (kinds.length === 1 && kinds[0] === "table") return text("Table changed", "表变化");
  return text("Plan facts changed", "计划事实变化");
}

function nodeLabel(node: GraphNode | null, fallback: string): string {
  if (!node) return fallback;
  const name = node.plan_node_type ?? node.display_name ?? node.node_id;
  const table = node.sql_facts?.find((fact) => fact.kind === "table")?.value.split(".").pop();
  const operator = `${name}${table ? ` ${table}` : ""}`;
  return hasPlanNode(node.plan_node_id) ? `Plan ${node.plan_node_id} · ${operator}` : operator;
}

function PlanCheck({ planComparison }: { planComparison: PlanComparison }): JSX.Element {
  const { text } = useLocale();
  const corr = planComparison.correlation;
  return (
    <div className="cmp-tab-pad compare-plan-check">
      <div className="notice-banner">
        <div>
          <strong>{text("Plan check", "只读计划检查")}</strong>
          {" · "}
          <span>{text("Plan structure only; actual performance needs execution observations.", "仅比较计划结构；实际性能需结合执行观测。")}</span>
        </div>
        <span className="state-stamp state-stamp--warn">
          ≠ {localizedEnum(planComparison.runtime_delta_state, text)}
        </span>
      </div>
      <dl className="kv-grid">
        <dt>{text("Plan comparison state", "比较状态")}</dt>
        <dd>
          <span className="status-inline">
            <span aria-hidden="true" className={statusDotClass(planComparison.state)} />
            <span>{localizedEnum(planComparison.state, text)}</span>
          </span>
        </dd>
        <dt>{text("Baseline source", "基线来源")}</dt>
        <dd>
          {originLabel(corr.baseline_origin, text)} · {localizedEnum(corr.baseline_source_kind, text)} ·{" "}
          <code className="mono">{corr.baseline_source_hash.slice(0, 12)}…</code>
        </dd>
        <dt>{text("Candidate plan", "候选计划")}</dt>
        <dd>
          {originLabel(corr.candidate_origin, text)} · <code className="mono">{corr.candidate_plan_hash.slice(0, 12)}…</code>
        </dd>
        <dt>{text("Structure summary", "结构比对摘要")}</dt>
        <dd>
          {text("Matched", "已匹配")} <span className="mono">{planComparison.structure_delta.matches.length}</span>
          {" · "}
          {text("Changed", "变化")} <span className="mono">{planComparison.structure_delta.changed.length}</span>
          {" · "}
          {text("Added", "新增")} <span className="mono">{planComparison.structure_delta.added.length}</span>
          {" · "}
          {text("Removed", "移除")} <span className="mono">{planComparison.structure_delta.removed.length}</span>
        </dd>
      </dl>
      {planComparison.limitations.length > 0 ? (
        <ul className="cmp-reasons">
          {planComparison.limitations.map((reason) => (
            <li key={reason}>{codeText(reason, text)}</li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

/* ---------- Raw / Provenance ---------- */

export function ProvenancePanel({
  baseline,
  candidate,
  comparison = null,
}: {
  baseline: AnalysisSnapshotReport | null;
  candidate: AnalysisSnapshotReport | null;
  comparison?: Comparison | null;
}): JSX.Element {
  const { text } = useLocale();
  return (
    <section aria-labelledby="comparison-provenance-title" className="cmp-tab-pad compare-provenance-panel">
      <div className="cmp-provenance-head">
        <h3 className="detail-sec-title" id="comparison-provenance-title">{text("Raw / Provenance", "原始数据 / 溯源")}</h3>
        <span className="card-subtitle">
          {text(
            "Identity and source references of both analyses. The comparison never returns raw Profile text.",
            "两次分析的标识与来源引用；对比不返回 Profile 原文。",
          )}
        </span>
      </div>
      {comparison ? (
        <dl className="kv-grid">
          <dt>{text("Comparison digest", "对比摘要哈希")}</dt>
          <dd className="mono">{comparison.comparison_key}</dd>
          {comparison.comparison_receipt.components.length > 0 ? (
            <>
              <dt>{text("Comparator receipt", "对比引擎回执")}</dt>
              <dd className="mono">
                {comparison.comparison_receipt.components.map((item) => `${item.id}@${item.version}`).join(" · ")}
              </dd>
            </>
          ) : null}
        </dl>
      ) : null}
      <div className="cmp-dual-grid">
        <ProvenanceSide label={text("Baseline", "基线")} report={baseline} />
        <ProvenanceSide label={text("Candidate", "候选")} report={candidate} />
      </div>
    </section>
  );
}

function ProvenanceSide({ label, report }: { label: string; report: AnalysisSnapshotReport | null }): JSX.Element {
  const { text } = useLocale();
  if (!report) {
    return (
      <section className="cmp-provenance-box">
        <div className="field-label">{label}</div>
        <p className="card-subtitle cmp-empty-copy">{text("No analyzed Profile selected.", "未选择已分析的 Profile。")}</p>
      </section>
    );
  }
  const sourceRefs = Array.from(
    new Set([...report.provenance_index, ...report.profile_context.evidence_refs, ...report.graph_view.source_refs]),
  );
  const versionEvidence = report.version.reported_version ?? report.version.detection_evidence[0] ?? null;

  return (
    <section className="cmp-provenance-box">
      <div className="field-label">{label}</div>
      <dl className="kv-grid">
        <dt>{text("Analysis key", "分析摘要哈希")}</dt>
        <dd><code className="mono">{report.analysis_key}</code></dd>
        <dt>{text("Facts hash", "事实表哈希")}</dt>
        <dd><code className="mono">{report.facts_hash}</code></dd>
        <dt>{text("Adapter", "解析器")}</dt>
        <dd>
          {localizedEnum(report.input.detected_kind, text)}
          {report.input.adapter_evidence.length ? (
            <> · <code className="mono">{report.input.adapter_evidence.join(" / ")}</code></>
          ) : null}
        </dd>
        <dt>{text("Version evidence", "版本证据")}</dt>
        <dd>
          <code className="mono">
            {versionEvidence
              ? `${versionEvidence.value} · ${versionEvidence.source_ref}`
              : text("No version evidence", "无版本证据")}
          </code>
        </dd>
      </dl>
      <details className="disclosure cmp-provenance-refs">
        <summary>{text(`Source refs (${sourceRefs.length})`, `来源引用（${sourceRefs.length}）`)}</summary>
        {sourceRefs.length ? (
          <ul className="cmp-ref-list">
            {sourceRefs.map((reference) => (
              <li key={reference}><code className="mono">{reference}</code></li>
            ))}
          </ul>
        ) : (
          <p className="card-subtitle">{text("No source refs supplied.", "未提供来源引用。")}</p>
        )}
      </details>
    </section>
  );
}

/* ---------- Helpers ---------- */

function reportOf(attempt: AnalysisAttempt): AnalysisSnapshotReport | null {
  return "outcome" in attempt ? attempt.outcome?.report ?? null : null;
}

function cellsOf(report: AnalysisSnapshotReport | null): Map<string, MetricCell> {
  const cells = new Map<string, MetricCell>();
  if (!report) return cells;
  for (const cell of report.derived_metric_cells) cells.set(cell.metric_cell_ref, cell);
  for (const row of report.operator_rows) {
    for (const cell of row.metric_cells) cells.set(cell.metric_cell_ref, cell);
  }
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

function countedCodes(codes: readonly string[]): { code: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const code of codes) counts.set(code, (counts.get(code) ?? 0) + 1);
  return [...counts].map(([code, count]) => ({ code, count }));
}

function codeText(code: string, text: LocalizedText): string {
  if (Object.prototype.hasOwnProperty.call(LOCAL_REASON_LABELS, code)) {
    const pair = LOCAL_REASON_LABELS[code];
    return text(pair[0], pair[1]);
  }
  if (code.startsWith("registry_equivalence:")) {
    return text("Metric registries matched via audited equivalence recipe", "指标注册表经已审计等价规则匹配");
  }
  const limitation = localizedComparisonText("limitation", code, text);
  if (limitation !== code) return limitation;
  const reason = localizedComparisonText("reason", code, text);
  if (reason !== code) return reason;
  const detail = localizedComparisonText("detail", code, text);
  if (detail !== code) return detail;
  return localizedEnum(code, text);
}

function statusDotClass(state: string): string {
  if (state === "comparable") return "status-dot status-dot--ok";
  if (state === "partially_comparable") return "status-dot status-dot--warn";
  return "status-dot status-dot--critical";
}

function toEvidenceLevel(state: string): EvidenceLevel {
  if (state === "complete") return "complete";
  if (state === "partial") return "partial";
  if (state === "sparse") return "sparse";
  return "missing";
}
