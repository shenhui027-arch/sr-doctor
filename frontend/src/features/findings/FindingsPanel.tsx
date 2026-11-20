import { useRef, useState, type JSX, type ReactNode } from "react";
import type {
  AnalysisSnapshotReport,
  Finding,
  FindingMeasurement,
  PlanNodeSummary,
  RuleEvaluation,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, EvidenceMeter, MetricName, SegTabs, type EvidenceLevel } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import { localizedFindingText, type FindingTextField } from "../../i18n/finding-text";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { measurementLabel, measurementThreshold } from "../../i18n/measurement-label";
import { metricLabel } from "../../i18n/metric-label";
import { durationNanosecondsText } from "../../lib/duration-display";
import { evidenceRowsForReferences, FindingRecommendation, recommendedFindings } from "../recommendations/RecommendationsPanel";
import { physicalOperatorOf, resolveSubjectLocation } from "../recommendations/recommendation-location";
import { useSelection, type SelectionStore } from "../selection/selection-store";
import "./findings.css";

type SubjectContext = {
  nodeId: string | null;
  planNodeId: string | null;
  label: string | null;
  summary: PlanNodeSummary | null;
};

/** The located plan node of a finding and its backend summary; never inferred from identifier text. */
export function findingContext(report: AnalysisSnapshotReport, finding: Finding): SubjectContext {
  return subjectContext(report, finding.subject_ref);
}

function subjectContext(report: AnalysisSnapshotReport, subjectRef: string | null): SubjectContext {
  const location = resolveSubjectLocation(report, subjectRef);
  if (location.state !== "located" || !location.node) return { nodeId: null, planNodeId: null, label: null, summary: null };
  const table = location.facts.find((fact) => fact.kind === "table")?.value;
  const summary = report.execution_summary.state === "available"
    ? report.execution_summary.nodes.find((node) => node.node_id === location.node!.node_id) ?? null
    : null;
  const planNodeId = location.node.plan_node_id;
  const operator = [location.operator, planNodeId === null ? null : `#${planNodeId}`].filter(Boolean).join(" ");
  return { nodeId: location.node.node_id, planNodeId, label: [operator, table].filter(Boolean).join(" · ") || null, summary };
}

/** Whether any product rule was evaluated at all; a Profile no rule could read is not a clean result. */
export function rulesEvaluated(report: AnalysisSnapshotReport): boolean {
  return report.rule_evaluations.some((evaluation) => evaluation.status !== "not_applicable");
}

const SEVERITY_ORDER = ["critical", "severe", "high", "moderate", "medium", "low", "info"];

function severityRank(severity: string): number {
  const index = SEVERITY_ORDER.indexOf(severity.toLowerCase());
  return index < 0 ? SEVERITY_ORDER.length : index;
}

function severityTone(severity: string): "critical" | "warn" | "neutral" {
  const rank = severityRank(severity);
  return rank <= 2 ? "critical" : rank <= 4 ? "warn" : "neutral";
}

function severityDot(severity: string): "critical" | "warn" | "hollow" {
  const tone = severityTone(severity);
  return tone === "critical" ? "critical" : tone === "warn" ? "warn" : "hollow";
}

/** The selected finding, or the most severe one so the inspector is never empty while findings exist. */
export function selectedFinding(report: AnalysisSnapshotReport, findingId: string | null): Finding | null {
  return report.findings.find((finding) => finding.finding_id === findingId)
    ?? [...report.findings].sort((left, right) => severityRank(left.severity) - severityRank(right.severity))[0]
    ?? null;
}

/** Findings list card in Doctor's right column: findings ordered as delivered, filterable by severity. */
export function FindingsPanel({ report, children }: { report: AnalysisSnapshotReport; children?: ReactNode }): JSX.Element {
  const { text } = useLocale();
  const { findingId, selectFinding } = useSelection();
  const [severity, setSeverity] = useState<string>("all");
  const severities = Array.from(new Set(report.findings.map((finding) => finding.severity))).sort((left, right) => severityRank(left) - severityRank(right));
  const visible = report.findings.filter((finding) => severity === "all" || finding.severity === severity);

  const filterItems = [
    { value: "all", label: text("All", "全部"), count: report.findings.length },
    ...severities.map((value) => ({
      value,
      label: localizedEnum(value, text),
      count: report.findings.filter((finding) => finding.severity === value).length,
    })),
  ];

  return (
    <section aria-labelledby="findings-title" className="card card--list findings-panel">
      <div className="card-hd findings-heading">
        <div className="card-hd-main">
          <h2 className="card-title" id="findings-title">
            {text("Findings", "发现")} <span className="findings-count">{report.findings.length}</span>
          </h2>
        </div>
        {severities.length > 1 ? (
          <div className="card-hd-actions">
            <SegTabs
              aria-label={text("Filter by severity", "按严重度筛选")}
              items={filterItems}
              onValueChange={setSeverity}
              size="sm"
              value={severity}
            />
          </div>
        ) : null}
      </div>
      <div className="card-bd card-bd--compact findings-body">
        {report.findings.length === 0 ? (
          <div className="state-box findings-empty">
            <p>
              {rulesEvaluated(report)
                ? text("No reportable issues were found in this Profile.", "当前 Profile 暂未发现可报告的问题。")
                : text("Rules were not evaluated for this Profile, so an empty list does not mean the query is healthy. The Profile notice above says what to capture instead.", "此 Profile 未评估诊断规则，列表为空不代表查询正常。应改为采集哪种 Profile，见上方的 Profile 提示。")}
            </p>
          </div>
        ) : (
          <ul className="item-list finding-list">
            {groupByNode(report, visible).map((group) =>
              group.findings.length > 1 ? (
                <li aria-label={group.label ?? undefined} className="finding-group" key={`node:${group.key}`} role="group">
                  <div className="finding-group-head">
                    <span className="finding-subject">{group.label}</span>
                    <span className="finding-group-count">{text(`${group.findings.length} findings`, `${group.findings.length} 条发现`)}</span>
                  </div>
                  <ul className="finding-group-list">
                    {group.findings.map(({ finding, context }) => (
                      <FindingCard
                        active={findingId === finding.finding_id}
                        context={context}
                        finding={finding}
                        grouped
                        key={finding.finding_id}
                        onSelect={() => selectFinding(finding.finding_id, finding.subject_ref)}
                        report={report}
                      />
                    ))}
                  </ul>
                </li>
              ) : (
                group.findings.map(({ finding, context }) => (
                  <FindingCard
                    active={findingId === finding.finding_id}
                    context={context}
                    finding={finding}
                    key={finding.finding_id}
                    onSelect={() => selectFinding(finding.finding_id, finding.subject_ref)}
                    report={report}
                  />
                ))
              ),
            )}
          </ul>
        )}
        {children}
      </div>
    </section>
  );
}

type FindingGroup = { key: string; label: string | null; findings: { finding: Finding; context: SubjectContext }[] };

function groupByNode(report: AnalysisSnapshotReport, findings: readonly Finding[]): FindingGroup[] {
  const groups: FindingGroup[] = [];
  const byNode = new Map<string, FindingGroup>();
  for (const finding of findings) {
    const context = findingContext(report, finding);
    const existing = context.nodeId ? byNode.get(context.nodeId) : undefined;
    if (existing) {
      existing.findings.push({ finding, context });
      continue;
    }
    const group = { key: context.nodeId ?? `finding:${finding.finding_id}`, label: context.label, findings: [{ finding, context }] };
    groups.push(group);
    if (context.nodeId) byNode.set(context.nodeId, group);
  }
  return groups;
}

/** "OPERATOR table · Plan N": the located node, its table's own name, and its plan node id. */
export function findingObjectLabel(report: AnalysisSnapshotReport, finding: Finding): string | null {
  const location = resolveSubjectLocation(report, finding.subject_ref);
  if (location.state !== "located" || !location.node) return null;
  const table = location.facts.find((fact) => fact.kind === "table")?.value.split(".").pop();
  const plan = location.node.plan_node_id === null ? "" : ` · Plan ${location.node.plan_node_id}`;
  return `${location.operator ?? location.node.node_id}${table ? ` ${table}` : ""}${plan}`;
}

export function findingTitleText(finding: Finding, text: LocalizedText): string {
  return localizedFindingText(finding.rule_id, "category", finding.category, text);
}

function FindingCard({
  active,
  context,
  finding,
  grouped = false,
  onSelect,
  report,
}: {
  active: boolean;
  context: SubjectContext;
  finding: Finding;
  grouped?: boolean;
  onSelect(): void;
  report: AnalysisSnapshotReport;
}): JSX.Element {
  const { text } = useLocale();
  const tone = severityTone(finding.severity);
  const severityText = localizedEnum(finding.severity, text);
  const titleText = findingTitleText(finding, text);
  const objectLabel = context.label && !grouped ? findingObjectLabel(report, finding) : null;
  const accessibleName = [severityText, titleText, objectLabel].filter(Boolean).join(" · ");
  return (
    <li>
      <button
        aria-haspopup="dialog"
        aria-label={accessibleName}
        className={`item-btn finding-item${active ? " is-selected selected" : ""}`}
        onClick={onSelect}
        type="button"
      >
        <span className="item-btn-top finding-item-title">
          <span className="status-inline">
            <span aria-hidden="true" className={`status-dot status-dot--${severityDot(finding.severity)}`} />
            <span className={`severity-tag ${tone}`}>{severityText}</span>
            <strong>{titleText}</strong>
          </span>
          {objectLabel ? <span className="finding-subject">{objectLabel}</span> : null}
        </span>
        {finding.measurements.length ? (
          <span className="finding-cost">
            {finding.measurements.slice(0, 2).map((measurement) => `${measurementLabel(measurement.measurement_id, text)} ${measurement.display_text}`).join(" · ")}
          </span>
        ) : context.summary ? (
          <span className="finding-cost">
            {text("Share of operator time", "占算子总耗时")} {context.summary.time_share_display ?? "—"} · {context.summary.total_time.display_text ?? "—"}
          </span>
        ) : null}
        <span className="item-btn-meta finding-meta">
          <span>{text("Confidence", "置信度")} {localizedEnum(finding.confidence, text)}</span>
          <EvidenceMeter
            label={`${text("Evidence", "证据")} ${localizedEnum(finding.evidence_completeness, text)}`}
            level={evidenceLevel(finding.evidence_completeness)}
          />
        </span>
      </button>
    </li>
  );
}

/** One concrete gap the analysis could not close; every field comes from the report DTO. */
export type EvidenceGap = { key: string; subject: string; state: string; detail: string };

export function evidenceGapItems(report: AnalysisSnapshotReport, text: LocalizedText): EvidenceGap[] {
  const gaps: EvidenceGap[] = [];
  const unconnected = new Set(report.graph_view.unconnected_node_ids);
  for (const node of report.graph_view.nodes) {
    if (!unconnected.has(node.node_id) && node.source_quality !== "partial") continue;
    const label = `${node.display_name ?? node.plan_node_type ?? node.node_id}${node.plan_node_id === null ? "" : ` · Plan ${node.plan_node_id}`}`;
    const measured = report.execution_summary.state === "available" && report.execution_summary.nodes.some((summary) => summary.node_id === node.node_id);
    gaps.push({
      key: `node:${node.node_id}`,
      subject: label,
      state: text("Unavailable", "不可用"),
      detail: `${measured ? "" : text("No runtime metrics; ", "无运行时指标，")}${unconnected.has(node.node_id) ? text("its upstream and downstream are unproven, so it is not connected", "上下游关系未证实，未连线") : text("its relation is only partially proven", "关系仅部分证实")}`,
    });
  }
  if (report.graph_view.state !== "unavailable" && report.graph_view.unresolved_relations.length > 0) {
    gaps.push({
      key: "topology",
      subject: text("Topology checks", "拓扑校验"),
      state: text("Contradiction", "不符"),
      detail: text(
        `${report.graph_view.unresolved_relations.length} structural check(s) failed against the official StarRocks topology; edges stay as printed but the graph is not complete.`,
        `${report.graph_view.unresolved_relations.length} 处与官方 StarRocks 拓扑不变量不符；边按原文绘制，但图不视为完整。`,
      ),
    });
  }
  if (report.execution_summary.result_rows.state !== "exact") {
    gaps.push({
      key: "result_rows",
      subject: "ResultRows",
      state: text("Missing", "缺失"),
      detail: text("Rows returned were not printed in the Profile; not counted as 0", "返回行数未在 Profile 中出现，不按 0 计"),
    });
  }
  if (gaps.length) return gaps;
  return (["metrics", "graph", "provenance", "version"] as const)
    .filter((key) => report.completeness[key].state !== "complete")
    .map((key) => ({
      key,
      subject: dimensionLabel(key, text),
      state: localizedEnum(report.completeness[key].state, text),
      detail: report.completeness[key].reasons.map((reason) => localizedEnum(reason, text)).join(" · "),
    }));
}

const PLAN_SHAPE_ABSENT = new Set([
  "join_operator_missing",
  "join_probe_operator_missing",
  "broadcast_exchange_missing",
  "streaming_preaggregation_missing",
  "matching_plan_node_missing",
  "matching_operator_group_missing",
]);

/** Checks that did not run because a prerequisite counter was absent (backend `missing_prerequisites`). */
export function heldBackEvaluations(report: AnalysisSnapshotReport): RuleEvaluation[] {
  const seen = new Set<string>();
  return report.rule_evaluations.filter((evaluation) => {
    if (evaluation.status !== "not_applicable" || evaluation.missing_prerequisites.length === 0 || seen.has(evaluation.rule_id)) return false;
    if (evaluation.missing_prerequisites.every((item) => PLAN_SHAPE_ABSENT.has(item))) return false;
    seen.add(evaluation.rule_id);
    return true;
  });
}

const HELD_BACK_LABELS: Record<string, [string, string]> = {
  "SR3514.SPILL_OBSERVED": ["Spill check", "溢写检查"],
  "SR3514.OPERATOR_TIME_SKEW": ["Data skew check", "数据倾斜检查"],
  "SR3514.JOIN_PROBE_FANOUT": ["Join fan-out check", "Join 扇出检查"],
  "SR3514.BROADCAST_EXCHANGE_BYTES": ["Broadcast volume check", "广播数据量检查"],
  "SR3514.STREAMING_PREAGG_INEFFECTIVE": ["Pre-aggregation check", "预聚合检查"],
  "SR3514.ROW_FLOW_AMPLIFICATION": ["Row amplification check", "行数放大检查"],
  "SR3514.SCAN_TIME_SHARE": ["Scan share check", "扫描占比检查"],
  "SR3514.JOIN_TIME_SHARE": ["Join share check", "Join 占比检查"],
  "SR3514.AGGREGATE_TIME_SHARE": ["Aggregate share check", "聚合占比检查"],
  "SR3514.EXCHANGE_NETWORK_TIME_SHARE": ["Exchange share check", "Exchange 占比检查"],
  "SR3514.NODE_TIME_SHARE": ["Node share check", "节点占比检查"],
  "SR3514.PIPELINE_DRIVER_TIME_SKEW": ["Driver skew check", "Driver 倾斜检查"],
};

const PREREQUISITE_LABELS: Record<string, [string, string]> = {
  per_instance_metrics: ["per-instance metrics", "逐实例指标"],
  spill_observation_missing: ["spill counters", "溢写计数"],
  runtime_operator_missing: ["runtime operators", "运行时算子"],
  operator_total_time_missing: ["OperatorTotalTime", "OperatorTotalTime"],
};

/** What the analysis could not establish, and the checks it held back. */
export function EvidenceGaps({ report, embedded = false }: { report: AnalysisSnapshotReport; embedded?: boolean }): JSX.Element | null {
  const { text } = useLocale();
  const gaps = evidenceGapItems(report, text);
  const heldBack = heldBackEvaluations(report);
  const suppressed = suppressedEvaluations(report);
  const recapturable = heldBack.length > 0 || gaps.some((gap) => gap.key === "result_rows");
  if (gaps.length === 0 && heldBack.length === 0 && suppressed.length === 0) return null;

  const content = (
    <>
      <div className="evidence-gaps-heading">
        <h2 className="card-title" id="evidence-gaps-title">
          {text("Evidence gaps", "证据缺口")} <span className="evidence-gaps-count">{gaps.length}</span>
        </h2>
        <span className="card-subtitle">{text("Bounds what the findings can claim", "影响结论范围")}</span>
      </div>
      {gaps.length > 0 ? (
        <ul className="evidence-gap-list">
          {gaps.map((gap) => (
            <li key={gap.key}>
              <span className="evidence-gap-subject">{gap.subject}</span>
              <span className="state-stamp evidence-gap-state">{gap.state}</span>
              {gap.detail ? <span className="evidence-gap-detail">{gap.detail}</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {heldBack.length > 0 ? (
        <div className="evidence-held-back">
          <h3>{text(`${heldBack.length} check(s) held back · prerequisite missing`, `已抑制的检查 ${heldBack.length} · 前置条件缺失`)}</h3>
          <ul>
            {heldBack.map((evaluation) => (
              <li key={evaluation.rule_id}>
                <span>{HELD_BACK_LABELS[evaluation.rule_id] ? text(...HELD_BACK_LABELS[evaluation.rule_id]) : evaluation.rule_id}</span>
                <span className="evidence-held-back-missing">
                  {text("missing", "缺")} {evaluation.missing_prerequisites.map((item) => (PREREQUISITE_LABELS[item] ? text(...PREREQUISITE_LABELS[item]) : item)).join(" / ")}
                </span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <SuppressedEvaluations evaluations={suppressed} report={report} />
      {recapturable ? (
        <p className="evidence-gaps-hint">
          {text(
            "To close them: capture again with “Runtime Top · run once” in the SQL workspace, then come back here.",
            "补齐方式：在 SQL 工作区用「运行时 Top · 执行一次」重新采集，再回到这里复核。",
          )}
        </p>
      ) : null}
    </>
  );

  if (embedded) {
    return (
      <details aria-labelledby="evidence-gaps-title" className="disclosure evidence-gaps evidence-gaps--embedded">
        <summary>{text(`Evidence gaps (${gaps.length})`, `证据缺口 (${gaps.length})`)}</summary>
        {content}
      </details>
    );
  }

  return (
    <section aria-labelledby="evidence-gaps-title" className="card evidence-gaps">
      <div className="card-bd">{content}</div>
    </section>
  );
}

export function dimensionLabel(key: "summary" | "metrics" | "graph" | "provenance" | "version", text: LocalizedText): string {
  if (key === "summary") return text("Query summary", "查询概要");
  if (key === "metrics") return text("Metrics", "指标");
  if (key === "graph") return text("Execution structure", "执行结构");
  if (key === "provenance") return text("Source provenance", "原文溯源");
  return text("Version", "版本");
}

/** The signature meter's level for a backend completeness state; the meter never invents a level. */
export function evidenceLevel(state: string): EvidenceLevel {
  return state === "complete" ? "complete" : state === "partial" ? "partial" : "missing";
}

function findingFooterTarget(report: AnalysisSnapshotReport, finding: Finding): {
  primarySourceRef: string | null;
  primaryLine: number | undefined;
  nodeId: string | null;
} {
  const evaluation = report.rule_evaluations.find((candidate) => candidate.finding_id === finding.finding_id) ?? null;
  const context = findingContext(report, finding);
  const references = Array.from(new Set(evaluation?.input_observation_refs.length ? evaluation.input_observation_refs : finding.evidence));
  const evidence = evidenceRowsForReferences(report, references).filter((row) => row.cell);
  const lines = sourceLineNumbers(report);
  const primarySourceRef = evidence[0]?.cell?.source_refs[0] ?? finding.evidence[0] ?? null;
  const primaryLine = primarySourceRef ? lines.get(primarySourceRef) : undefined;
  return { primarySourceRef, primaryLine, nodeId: context.nodeId };
}

export function findingHasFooterActions(report: AnalysisSnapshotReport, finding: Finding): boolean {
  const { primarySourceRef, nodeId } = findingFooterTarget(report, finding);
  return Boolean(primarySourceRef || nodeId);
}

/** Selected finding detail body: problem statement, evidence with MetricName, limitations, recommendation, and rule trace. */
export function FindingInspectorBody({
  finding,
  report,
  includeEvidenceGaps = false,
}: {
  finding: Finding;
  report: AnalysisSnapshotReport;
  includeEvidenceGaps?: boolean;
}): JSX.Element {
  const { text } = useLocale();
  const { selectSourceRef } = useSelection();
  const technical = useRef<HTMLDetailsElement>(null);

  const evaluation = report.rule_evaluations.find((candidate) => candidate.finding_id === finding.finding_id) ?? null;
  const context = findingContext(report, finding);
  const references = Array.from(new Set(evaluation?.input_observation_refs.length ? evaluation.input_observation_refs : finding.evidence));
  const evidence = evidenceRowsForReferences(report, references).filter((row) => row.cell);
  const measuredBy = new Map(finding.measurements.filter((item) => item.input_refs.length === 1).map((item) => [item.input_refs[0], item] as const));
  const headline = finding.measurements[0] ?? null;
  const derived = headline && headline.input_refs.length !== 1 ? headline : null;
  const nodeShare = finding.measurements.some((item) => item.measurement_id === "node_time_share");
  const queryTotal = report.execution_summary.query_total_time.display_text;
  const category = findingTitleText(finding, text);
  const lines = sourceLineNumbers(report);
  const family = context.nodeId ? operatorFamily(report.graph_view.nodes.find((node) => node.node_id === context.nodeId)?.plan_node_type ?? null) : null;
  const rule = finding.rule_id.includes("@") ? finding.rule_id : `${finding.rule_id}@${finding.rule_version}`;
  const limitations = finding.limitations.map((value) => localizedFindingText(finding.rule_id, "limitation", value, text));
  const completenessDetail = finding.evidence_completeness_reasons
    .map((reason) => localizedFindingText(finding.rule_id, "evidence_completeness_reason", reason, text))
    .join(" · ") || undefined;

  return (
    <>
      <div className="inspector-drawer-meta">
        <div className="inspector-tags">
          <span className="status-inline">
            <span aria-hidden="true" className={`status-dot status-dot--${severityDot(finding.severity)}`} />
            <span className={`severity-tag ${severityTone(finding.severity)}`}>{localizedEnum(finding.severity, text)}</span>
          </span>
          <span className="inspector-rule-meta">{family ? `${family} · ` : ""}{text("Rule", "规则")} {rule}</span>
        </div>
        <div className="inspector-subject">
          <span>{findingObjectLabel(report, finding) ?? text("Whole query", "整条查询")}</span>
        </div>
      </div>

      <dl className="kv-grid inspector-facts">
        <dt>{text("Confidence", "置信度")}</dt>
        <dd>{localizedEnum(finding.confidence, text)}</dd>
        <dt>{text("Evidence completeness", "证据完整度")}</dt>
        <dd className={`completeness-${finding.evidence_completeness}`} title={completenessDetail}>
          <EvidenceMeter
            detail={completenessDetail}
            label={`${text("Evidence", "证据")} ${localizedEnum(finding.evidence_completeness, text)}`}
            level={evidenceLevel(finding.evidence_completeness)}
          />{" "}
          {localizedEnum(finding.evidence_completeness, text)}
        </dd>
        <dt>{text("Comparable dimension", "可比维度")}</dt>
        <dd>{headline ? dimensionLabelOf(headline.measurement_id, text) : "—"}</dd>
      </dl>

      <section className="inspector-section">
        <h3>{text("Problem statement", "问题陈述")}</h3>
        <p>
          {context.summary && !nodeShare ? (
            <span className="finding-cost">
              {text("This node", "该节点")}{text(": ", "：")}{text("share of operator time", "占算子总耗时")} {context.summary.time_share_display ?? "—"} · {context.summary.total_time.display_text ?? "—"}
              {queryTotal ? ` · ${text("query total", "查询总耗时")} ${queryTotal}` : ""}{text(". ", "。")}
            </span>
          ) : nodeShare && queryTotal ? (
            <span className="finding-cost">{text("Query total", "查询总耗时")} {queryTotal}{text(". ", "。")}</span>
          ) : null}
          {text(`${category}; the root cause remains unproven.`, `${category}；根因仍待验证。`)}
        </p>
      </section>

      <section className="inspector-section">
        <h3>{text("Evidence", "证据")}</h3>
        {evidence.length || derived ? (
          <ul className="inspector-evidence">
            {evidence.map((row) => {
              const reference = row.cell!.source_refs[0];
              const line = reference ? lines.get(reference) : undefined;
              const localizedName = measuredBy.has(row.reference)
                ? measurementLabel(measuredBy.get(row.reference)!.measurement_id, text)
                : metricLabel(row.cell!.metric_id, text);
              return (
                <li
                  key={row.reference}
                  title={
                    row.location?.state === "query"
                      ? text("Whole query", "整条查询")
                      : measuredOperator(measuredBy.get(row.reference), context.planNodeId)
                        ?? physicalOperatorOf(report, row.cell!, context.nodeId)
                        ?? (row.subject || undefined)
                  }
                >
                  <MetricName
                    label={<span className="inspector-metric">{localizedName}</span>}
                    official={localizedName !== row.cell!.metric_id ? row.cell!.metric_id : undefined}
                  />
                  <strong>{row.cell!.display_text}</strong>
                  {reference ? (
                    <button className="reference-button" onClick={() => selectSourceRef(reference)} title={reference} type="button">
                      {line === undefined ? text("Source line", "原文行") : `${text("Source", "原文")} L${line}`}
                    </button>
                  ) : (
                    <span />
                  )}
                </li>
              );
            })}
            {derived ? (
              <li key="derived">
                <span className="inspector-metric inspector-derived">{measurementLabel(derived.measurement_id, text)} · {text("derived", "派生")}</span>
                <strong>{derived.display_text}</strong>
                <button
                  className="reference-button"
                  onClick={() => {
                    if (technical.current) {
                      technical.current.open = true;
                      technical.current.scrollIntoView({ block: "nearest" });
                    }
                  }}
                  type="button"
                >
                  {text("Derivation", "计算链")}
                </button>
              </li>
            ) : null}
          </ul>
        ) : (
          <p className="inspector-empty-copy">{text("No referenced metric cell is available in this report.", "此报告中没有可用的引用指标单元。")}</p>
        )}
      </section>

      <section className="inspector-section">
        <h3>{text("Limitations", "限制")}</h3>
        <p className="inspector-limitations">
          {limitations.length ? `${limitations.join(text("; ", "；"))}${text(".", "。")}` : text("None supplied by the report.", "报告未提供。")}
        </p>
      </section>

      {recommendedFindings(report).some((candidate) => candidate.finding_id === finding.finding_id) ? (
        <FindingRecommendation finding={finding} report={report} />
      ) : (
        <section className="inspector-direction">
          <h3>{text("Recommended direction", "建议方向")}</h3>
          <p>{localizedFindingText(finding.rule_id, "expected_direction", finding.recommendation.expected_direction, text)}</p>
        </section>
      )}

      <details className="disclosure technical-disclosure" ref={technical}>
        <summary>{text("Evidence and rule details", "证据与规则详情")}</summary>
        <dl className="kv-grid finding-facts">
          <dt>{text("Rule", "规则")}</dt><dd><code>{finding.rule_id}</code></dd>
          <dt>{text("Rule version", "规则版本")}</dt><dd>{finding.rule_version}</dd>
          <dt>{text("Category", "类别")}</dt><dd>{category}</dd>
          <dt>{text("Subject", "对象")}</dt><dd>{finding.subject_ref ? <code>{finding.subject_ref}</code> : "unavailable"}</dd>
          <dt>{text("Rule trace", "规则轨迹")}</dt><dd><code>{finding.rule_trace_ref}</code></dd>
        </dl>
        {finding.measurements.length ? (
          <ul className="inspector-measurements">
            {finding.measurements.map((item) => (
              <li key={item.measurement_id}>
                <span>{measurementLabel(item.measurement_id, text)}{item.operator_type ? <small> · {item.operator_type}</small> : null}</span>
                <strong>{item.display_text}</strong>
                {item.threshold_text ? <small className="inspector-threshold">{measurementThreshold(item.threshold_text, text)}</small> : null}
              </li>
            ))}
          </ul>
        ) : null}
        <FindingList field="validation_step" label={text("External validation steps", "外部验证步骤")} ruleId={finding.rule_id} values={finding.recommendation.validation_steps} />
        {finding.recommendation.risk ? (
          <p className="inspector-empty-copy">{text("Risk", "风险")}: {localizedFindingText(finding.rule_id, "risk", finding.recommendation.risk, text)}</p>
        ) : null}
        <FindingList field="confidence_reason" label={text("Confidence reasons", "置信依据")} ruleId={finding.rule_id} values={finding.confidence_reasons} />
        <FindingList field="evidence_completeness_reason" label={text("Completeness reasons", "完整度依据")} ruleId={finding.rule_id} values={finding.evidence_completeness_reasons} />
        <ReferenceList label={text("Source evidence", "原文依据")} onSelect={selectSourceRef} values={finding.evidence} />
        <FindingList label={text("Related findings", "相关发现")} values={finding.related_finding_refs} />
        {evaluation ? (
          <EvaluationTrace evaluation={evaluation} finding={finding} onSelect={selectSourceRef} />
        ) : (
          <p className="inspector-empty-copy">{text("No exact RuleEvaluation was supplied for this finding.", "此发现没有提供精确的 RuleEvaluation。")}</p>
        )}
      </details>

      {includeEvidenceGaps ? <EvidenceGaps embedded report={report} /> : null}
    </>
  );
}

/** Finding-specific footer actions for the right-side detail drawer. */
export function FindingInspectorFooter({
  finding,
  report,
}: {
  finding: Finding;
  report: AnalysisSnapshotReport;
}): JSX.Element | null {
  const { text } = useLocale();
  const { showFindingNode, selectSourceRef } = useSelection();
  const { primarySourceRef, primaryLine, nodeId } = findingFooterTarget(report, finding);
  if (!primarySourceRef && !nodeId) return null;

  return (
    <>
      <span className="inspector-footer-meta">
        {primaryLine !== undefined ? `${text("Source", "原文")} L${primaryLine}` : ""}
      </span>
      <div className="inspector-footer-actions">
        {primarySourceRef ? (
          <Button
            onClick={() => selectSourceRef(primarySourceRef)}
            size="sm"
            variant="secondary"
          >
            {primaryLine === undefined ? text("View source", "查看原文") : text(`View source L${primaryLine}`, `查看原文 L${primaryLine}`)}
          </Button>
        ) : null}
        {nodeId ? (
          <Button
            onClick={() => showFindingNode(nodeId)}
            size="sm"
            variant="secondary"
          >
            {text("Show in execution graph", "在图中定位")}
          </Button>
        ) : null}
      </div>
    </>
  );
}

function sourceLineNumbers(report: AnalysisSnapshotReport): Map<string, number> {
  const lines = new Map<string, number>();
  for (const line of report.source_view.plain_text_lines) if (!lines.has(line.source_ref)) lines.set(line.source_ref, line.line_no);
  for (const line of report.source_view.line_refs ?? []) if (!lines.has(line.source_ref)) lines.set(line.source_ref, line.line_no);
  return lines;
}

function operatorFamily(type: string | null): string | null {
  if (!type) return null;
  if (type.includes("JOIN")) return "Join";
  if (type.includes("SCAN")) return "Scan";
  if (type.includes("AGG")) return "Aggregate";
  if (type.includes("EXCHANGE")) return "Exchange";
  if (type.includes("SORT") || type.includes("TOP_N")) return "Sort";
  return null;
}

function dimensionLabelOf(measurementId: string, text: LocalizedText): string {
  if (measurementId === "node_time_share") return text("Time share", "耗时占比");
  return measurementLabel(measurementId, text);
}

function measuredOperator(measurement: FindingMeasurement | undefined, planNodeId: string | null): string | null {
  return measurement?.operator_type ? `${measurement.operator_type}${planNodeId ? ` #${planNodeId}` : ""}` : null;
}

function EvaluationTrace({ evaluation, finding, onSelect }: { evaluation: RuleEvaluation; finding: Finding; onSelect(value: string): void }): JSX.Element {
  const { text } = useLocale();
  return (
    <div className="rule-evaluation">
      <p className="field-label">{text("Rule evaluation trace", "规则评估轨迹")}</p>
      <dl className="kv-grid finding-facts">
        <dt>{text("Status", "状态")}</dt><dd>{localizedEnum(evaluation.status, text)}</dd>
        <dt>{text("Evidence completeness", "证据完整度")}</dt><dd>{localizedEnum(evaluation.evidence_completeness, text)}</dd>
        <dt>{text("Evaluation rule", "评估规则")}</dt><dd>{evaluation.rule_id}@{evaluation.rule_version}</dd>
      </dl>
      <FindingList field="evidence_completeness_reason" label={text("Evaluation completeness reasons", "评估完整度依据")} ruleId={finding.rule_id} values={evaluation.evidence_completeness_reasons} />
      <FindingList field="applicability" label={text("Applicability checks", "适用性检查")} ruleId={finding.rule_id} values={evaluation.applicability_checks} />
      <ReferenceList label={text("Input observation refs", "输入观测引用")} onSelect={onSelect} values={evaluation.input_observation_refs} />
      <FindingList label={text("Comparisons", "比较")} values={evaluation.comparisons} />
      <FindingList label={text("Thresholds used", "使用的阈值")} values={evaluation.threshold_receipts} />
      <FindingList label={text("Missing prerequisites", "缺失前置条件")} values={evaluation.missing_prerequisites} />
      <FindingList field="limitation" label={text("Evaluation limitations", "评估限制")} ruleId={finding.rule_id} values={evaluation.limitations} />
      <ReferenceList label={text("Evaluation evidence refs", "评估证据引用")} onSelect={onSelect} values={evaluation.evidence_refs} />
    </div>
  );
}

function ReferenceList({ label, onSelect, values }: { label: string; onSelect(value: string): void; values: string[] }): JSX.Element {
  const { text } = useLocale();
  return (
    <div className="finding-values">
      <p className="field-label">{label}</p>
      {values.length === 0 ? (
        <p className="inspector-empty-copy">{text("Not available.", "暂无资料。")}</p>
      ) : (
        <div className="source-ref-list">
          {values.map((value, index) => (
            <button className="reference-button" key={`${value}-${index}`} onClick={() => onSelect(value)} title={value} type="button">
              {text(`View evidence ${index + 1}`, `查看依据 ${index + 1}`)}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function FindingList({ field, label, ruleId, values }: { field?: FindingTextField; label: string; ruleId?: string; values: string[] }): JSX.Element {
  const { text } = useLocale();
  return (
    <div className="finding-values">
      <h3>{label}</h3>
      {values.length === 0 ? (
        <p className="inspector-empty-copy">{text("None supplied by the report.", "报告未提供。")}</p>
      ) : (
        <ul>
          {values.map((value) => (
            <li key={value}>{field ? localizedFindingText(ruleId, field, value, text) : value}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

const SHARE_RULE_LABELS: Record<string, [string, string]> = {
  "SR3514.SCAN_TIME_SHARE": ["Scan time share", "扫描耗时占比"],
  "SR3514.JOIN_TIME_SHARE": ["Join time share", "关联耗时占比"],
  "SR3514.AGGREGATE_TIME_SHARE": ["Aggregate time share", "聚合耗时占比"],
  "SR3514.EXCHANGE_NETWORK_TIME_SHARE": ["Exchange network time share", "数据交换网络耗时占比"],
};

export function suppressedFloorNs(evaluation: RuleEvaluation): string | null {
  for (const comparison of evaluation.comparisons) {
    const match = /^subject_time_all_below_absolute_floor_(\d+)ns$/.exec(comparison);
    if (match) return match[1];
  }
  return null;
}

function suppressedEvaluations(report: AnalysisSnapshotReport): RuleEvaluation[] {
  return report.rule_evaluations.filter((evaluation) => evaluation.status === "suppressed" && suppressedFloorNs(evaluation) !== null);
}

export function locateSuppressedNode(selection: Pick<SelectionStore, "revealNode">, nodeId: string): void {
  selection.revealNode(nodeId);
}

function SuppressedEvaluations({ evaluations, report }: { evaluations: RuleEvaluation[]; report: AnalysisSnapshotReport }): JSX.Element | null {
  const { text } = useLocale();
  const selection = useSelection();
  if (!evaluations.length) return null;
  const floor = durationNanosecondsText(suppressedFloorNs(evaluations[0])!) ?? "—";
  return (
    <details className="disclosure findings-suppressed">
      <summary>{text(`${evaluations.length} share signal(s) below the ${floor} floor, not reported`, `${evaluations.length} 处耗时占比超过阈值，但节点耗时低于 ${floor}，未作为发现报告`)}</summary>
      <p className="inspector-empty-copy">
        {text(
          "Product rule: an operator that takes less than this in total is rarely worth investigating, even when it dominates a short query.",
          "产品规则：节点总耗时低于该下限时，即使在短查询中占比很高，通常也不值得专门排查。",
        )}
      </p>
      <ul>
        {evaluations.map((evaluation) => {
          const context = subjectContext(report, evaluation.subject_ref);
          const label = SHARE_RULE_LABELS[evaluation.rule_id];
          return (
            <li key={`${evaluation.rule_id}:${evaluation.subject_ref ?? ""}`}>
              <span>{label ? text(...label) : evaluation.rule_id}{context.label ? ` · ${context.label}` : ""}</span>
              {context.summary ? <span className="finding-cost">{context.summary.time_share_display ?? "—"} · {context.summary.total_time.display_text ?? "—"}</span> : null}
              {context.nodeId ? (
                <button className="reference-button" onClick={() => locateSuppressedNode(selection, context.nodeId!)} type="button">
                  {text("Show in graph", "在图中定位")}
                </button>
              ) : null}
            </li>
          );
        })}
      </ul>
    </details>
  );
}
