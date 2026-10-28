import { metricLabel } from "../../i18n/metric-label";
import { physicalOperatorOf, resolveMetricLocation, resolveSubjectLocation, type MetricLocation } from "./recommendation-location";
import { useMemo, useState } from "react";
import type { JSX } from "react";
import type {
  AnalysisSnapshotReport,
  ConnectedRecommendation,
  ExpectedMetricChange,
  Finding,
  MetricCell,
  PlanAdvisorResponse,
  Recommendation,
  RuleEvaluation,
} from "../../../../contracts/generated/doctor-contracts";
import { localizedEnum } from "../../i18n/enum-label";
import { localizedAdvisorText, type AdvisorTextField } from "../../i18n/advisor-text";
import { localizedFindingText, type FindingTextField } from "../../i18n/finding-text";
import { measurementLabel } from "../../i18n/measurement-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { compactCountText } from "../../lib/duration-display";
import "./recommendations.css";

type RecommendationItem =
  | { kind: "product"; finding: Finding; evaluation: RuleEvaluation | null }
  | { kind: "advisor"; recommendation: ConnectedRecommendation };

type SourceFilter = "all" | RecommendationItem["kind"];

export type MetricEvidenceRow = {
  reference: string;
  cell: MetricCell | null;
  subject: string;
  location?: ReturnType<typeof resolveMetricLocation>;
  /** Physical operator the cell was printed under (e.g. `AGGREGATE_BLOCKING_SINK #5`), only when exactly one matches. */
  physicalOperator?: string | null;
};

/** Exact-only evidence lookup: never derive a metric-cell reference in the client. */
export function evidenceRowsForReferences(report: AnalysisSnapshotReport | null, references: readonly string[]): MetricEvidenceRow[] {
  const cells = report ? [...report.operator_rows.flatMap((row) => row.metric_cells), ...report.derived_metric_cells] : [];
  const byReference = new Map<string, MetricCell[]>();
  for (const cell of cells) byReference.set(cell.metric_cell_ref, [...(byReference.get(cell.metric_cell_ref) ?? []), cell]);

  return orderedUnique(references).map((reference) => {
    const matches = byReference.get(reference) ?? [];
    const cell = matches.length === 1 ? matches[0] : null;
    if (!cell || !report) return { reference, cell, subject: "" };
    const location = resolveMetricLocation(report, cell);
    return { reference, cell, subject: location.operator ?? "", location, physicalOperator: physicalOperatorOf(report, cell, location.node?.node_id ?? null) };
  });
}

/**
 * Product decision (v0.7-03): a suspected finding — confidence `low`, its
 * measurement straddling the rule threshold — stays on the Doctor page but is
 * not turned into an optimization recommendation.
 */
export function recommendedFindings(report: AnalysisSnapshotReport | null): Finding[] {
  return (report?.findings ?? []).filter((finding) => finding.confidence !== "low");
}

export function RecommendationsPanel({
  report,
  advisorResponse, onLocateNode, onOpenQueries, onImportProfile, onOpenCompare,
}: {
  report: AnalysisSnapshotReport | null;
  advisorResponse?: PlanAdvisorResponse | null; onLocateNode?(nodeId: string): void;
  onOpenQueries?(): void; onImportProfile?(): void; onOpenCompare?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const items = useMemo<RecommendationItem[]>(() => {
    const product = recommendedFindings(report).map<RecommendationItem>((finding) => ({
      kind: "product",
      finding,
      evaluation: report?.rule_evaluations.find((candidate) => candidate.finding_id === finding.finding_id) ?? null,
    }));
    const advisor = (advisorResponse?.recommendations ?? []).map<RecommendationItem>((recommendation) => ({ kind: "advisor", recommendation }));
    return [...product, ...advisor];
  }, [advisorResponse, report]);
  const heldBack = (report?.findings.length ?? 0) - recommendedFindings(report).length;

  const [source, setSource] = useState<SourceFilter>("all");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const filtered = items.filter((item) => source === "all" || item.kind === source);
  const selected = filtered.find((item) => itemKey(item) === selectedKey) ?? filtered[0] ?? null;

  // Before any analysis there is nothing to list, select or compare: one short hint with the two ways in.
  if (!report && !advisorResponse) return <section className="recommendations-workspace recommendations-start" aria-labelledby="recommendations-title">
    <div className="recommendation-empty">
      <h2 id="recommendations-title">{text("No analysis loaded", "尚未载入分析")}</h2>
      <p>{text("Recommendations come from a diagnosed Profile. Pick a query to diagnose, or import a Profile file.", "建议来自已诊断的 Profile。请先选一条查询诊断，或导入 Profile 文件。")}</p>
      <p>{text("Evidence-backed guidance · nothing is auto-applied", "基于证据的建议 · 不会自动应用任何修改")}</p>
      <div className="row actions">
        {onOpenQueries && <button className="button accent" onClick={onOpenQueries} type="button">{text("Pick a query", "选择查询")}</button>}
        {onImportProfile && <button className="button secondary" onClick={onImportProfile} type="button">{text("Import profile", "导入 Profile")}</button>}
      </div>
    </div>
  </section>;

  const counts: Record<SourceFilter, number> = {
    all: items.length,
    product: items.filter((item) => item.kind === "product").length,
    advisor: items.filter((item) => item.kind === "advisor").length,
  };
  const pills: Array<{ value: SourceFilter; label: string }> = [
    { value: "all", label: text("All", "全部来源") },
    { value: "product", label: text("Product rules", "产品规则") },
    { value: "advisor", label: text("Official Advisor · experimental", "官方 Advisor · 实验性") },
  ];
  const heldBackNote = heldBack > 0 ? <p className="recommendation-held-back" title={text("Low confidence means the measurement straddles the rule threshold; such findings stay in Doctor.", "低置信度表示测量值跨越规则阈值，仅属疑似；这类发现只在诊断页列出。")}>{text(
    `${heldBack} low-confidence finding(s) not turned into a recommendation`,
    `另有 ${heldBack} 条低置信度发现未生成建议`,
  )}</p> : null;

  return <section className="recommendations-workspace" aria-labelledby="recommendations-title">
    <header className="recommendations-header">
      <div>
        <h1 id="recommendations-title">{text("Optimization recommendations", "优化建议")}</h1>
        <p>{text("Each recommendation links to a finding, operator and source evidence; investigate first, then verify with before/after comparison.", "每条建议都关联到发现、算子和原文证据；先排查，再用前后对比验证收益。")}</p>
      </div>
      <div aria-controls="recommendation-list" aria-label={text("Recommendation source", "建议来源")} className="recommendation-source-pills" role="group">
        {pills.map((pill) => <button aria-pressed={source === pill.value} key={pill.value} onClick={() => setSource(pill.value)} type="button">{pill.label} <span className="recommendation-pill-count">{counts[pill.value]}</span></button>)}
      </div>
    </header>

    {items.length === 0 ? <div className="recommendation-none-layout">
      <div className="recommendation-none" role="status">
        <strong>{report ? text("No recommendations for this Profile", "此 Profile 没有优化建议") : text("No recommendations", "暂无建议")}</strong>
        <p>{report
          ? text("No product rule found an evidence-backed issue in this Profile. This is not a health score, and nothing is auto-applied.", "产品规则未在此 Profile 中发现有证据支撑的问题。这不是健康评分，也不会自动应用任何修改。")
          : text("The Official Advisor responded without guidance.", "官方 Advisor 已响应，但未提供指引。")}</p>
      </div>
      {heldBackNote}
      {advisorResponse ? <AdvisorGuide advisorResponse={advisorResponse} /> : null}
    </div> : <div className="recommendation-layout">
      <section aria-label={text("Recommendation list", "建议列表")} className="recommendation-list-panel" id="recommendation-list">
        {filtered.length === 0 ? <div className="recommendation-empty"><strong>{text("No recommendation from this source", "该来源暂无建议")}</strong></div> : <ol className="recommendation-cards">
          {filtered.map((item) => {
            const key = itemKey(item);
            const active = key === itemKey(selected);
            const location = locationOf(item, report);
            return <li key={key}>
              <button aria-controls="recommendation-detail" aria-pressed={active} className={`recommendation-card${active ? " selected" : ""}`} onClick={() => setSelectedKey(key)} type="button">
                <span className="recommendation-card-top"><span className={`recommendation-source ${item.kind}`}>{kindLabel(item, location, text)}</span><StateChip item={item} location={location} /></span>
                <strong className="recommendation-card-title">{titleOf(item, text)}</strong>
                <span className="recommendation-card-object"><SeverityTag item={item} /><ObjectLabel item={item} location={location} /></span>
                <span className="recommendation-card-meta">
                  {expectedSummary(item, location, text) ? <span>{text("Expected", "预期")} {expectedSummary(item, location, text)}</span> : null}
                  <span>{text("Risk", "风险")} {riskLevelLabel(item, text)}</span>
                </span>
              </button>
            </li>;
          })}
        </ol>}
        {advisorResponse ? <AdvisorGuide advisorResponse={advisorResponse} /> : null}
        {heldBackNote}
      </section>

      {selected
        ? <RecommendationDetail item={selected} key={itemKey(selected)} onLocateNode={onLocateNode} onOpenCompare={onOpenCompare} report={report} />
        : <article aria-label={text("Recommendation detail", "建议详情")} className="recommendation-detail-panel" id="recommendation-detail"><div className="recommendation-empty"><strong>{text("No recommendation selected", "未选择建议")}</strong><p>{text("Choose another source to see its recommendations.", "请切换来源查看其建议。")}</p></div></article>}
    </div>}
  </section>;
}

type StepView = { title: string; detail: string | null; evidence: MetricEvidenceRow[] };

function RecommendationDetail({ item, onLocateNode, onOpenCompare, report }: {
  item: RecommendationItem; onLocateNode?(nodeId: string): void; onOpenCompare?(): void;
  report: AnalysisSnapshotReport | null;
}): JSX.Element {
  const { text } = useLocale();
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const recommendation = recommendationOf(item);
  const ruleId = ruleIdOf(item);
  const location = locationOf(item, report);
  const steps = stepsOf(item, report, text);
  const { shown: evidence, more: moreEvidence } = recommendationEvidence(item, report, steps);
  const derived = item.kind === "product" ? item.finding.measurements.find((measurement) => measurement.input_refs.length > 1) ?? null : null;
  // What the recommendation did not check; the finding's measurement caveats stay on the Doctor page.
  const limitations = item.kind === "product"
    ? item.finding.recommendation.limitations.map((value) => localizedFindingText(ruleId, "recommendation_limitation", value, text))
    : item.recommendation.limitations.map((value) => localizedAdvisorText("limitation", value, text));
  const locatable = location.node && onLocateNode ? location.node.node_id : null;
  const changes = recommendation.expected_changes;
  const lines = sourceLineNumbers(report);
  // A chip names its operator once; later chips on the same node keep only the plan id.
  const chipNamed = new Set<string>();

  async function copySteps(): Promise<void> {
    try {
      await navigator.clipboard.writeText(steps.map((step, index) => `${index + 1}. ${step.title}${step.detail ? `${text(": ", "：")}${step.detail}` : ""}`).join("\n"));
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
  }

  return <article aria-label={text("Recommendation detail", "建议详情")} className="recommendation-detail-panel" id="recommendation-detail">
    <header className="recommendation-detail-header">
      <div className="recommendation-detail-chips">
        <span className="chip">{sourceLabel(item, text)} · <code title={item.kind === "product" ? `${item.finding.rule_id}@${item.finding.rule_version}` : undefined}>{recommendation.recommendation_id}</code></span>
        {item.kind === "product" ? <span className={`chip ${severityTone(item.finding.severity)}`} title={findingTitle(item, text)}>{`${text("Linked finding", "关联发现")} · ${localizedEnum(item.finding.severity, text)}`}</span> : <span className="chip dashed">{localizedEnum("experimental", text)}</span>}
        <StateChip item={item} location={location} />
      </div>
      <h2>{titleOf(item, text)}</h2>
      <p className="problem-statement-copy">{summaryOf(item, location, text)}</p>
      <div className="row actions recommendation-detail-actions">
        {onOpenCompare && <button className="button accent" onClick={onOpenCompare} type="button">{text("Start before/after comparison", "开始前后对比")}</button>}
        {item.kind === "product" && <button className="button secondary" disabled={!locatable} onClick={() => { if (locatable) onLocateNode?.(locatable); }} title={locatable ? undefined : text("Operator not located", "尚未定位算子")} type="button">{text("Locate operator", "定位算子")}</button>}
        <button className="button secondary" disabled={steps.length === 0} onClick={() => { void copySteps(); }} type="button">{text("Copy investigation steps", "复制排查步骤")}</button>
        <span aria-live="polite" className="recommendation-copy-status" role="status">{copyState === "copied" ? text("Steps copied", "已复制排查步骤") : copyState === "failed" ? text("Copy failed: clipboard unavailable", "复制失败：剪贴板不可用") : ""}</span>
      </div>
    </header>

    <div className="recommendation-detail-body">
      <div className="recommendation-detail-main">
        <section>
          <h3>{text("What to check", "具体排查")}</h3>
          {steps.length ? <ol className="recommendation-steps">{steps.map((step, index) => <li key={`${index}-${step.title}`}>
            <span aria-hidden="true" className="recommendation-step-number">{index + 1}</span>
            <div>
              <div className="recommendation-step-title">{step.title}</div>
              {step.detail ? <div className="recommendation-step-detail">{step.detail}</div> : null}
              {step.evidence.map((row) => <code className="recommendation-step-evidence" key={row.reference} title={row.reference}>{evidenceChip(row, chipNamed, text)}</code>)}
            </div>
          </li>)}</ol> : <p className="recommendation-empty-copy">{text("No investigation steps were supplied.", "未提供排查步骤。")}</p>}
        </section>
        <section>
          <h3>{text("Evidence", "证据")}</h3>
          {evidence.length || derived ? <ul className="recommendation-evidence-grid">
            {evidence.map((row) => {
              const cell = row.cell!;
              const nodeId = row.location?.node?.node_id;
              const plan = row.location?.node?.plan_node_id;
              const line = cell.source_refs[0] ? lines.get(cell.source_refs[0]) : undefined;
              const where = [line === undefined ? text("Source", "原文") : `${text("Source", "原文")} L${line}`, row.location?.state === "query" ? text("Whole query", "整条查询") : plan != null ? `Plan ${plan}` : null].filter(Boolean).join(" · ");
              return <li className="recommendation-evidence-cell" key={row.reference}>
                <span className="recommendation-evidence-label" title={metricLabel(cell.metric_id, text)}>{cell.metric_id}</span>
                <strong className="recommendation-evidence-value" title={cell.display_text || undefined}>{cardValue(cell) ?? (cell.display_text || localizedEnum(cell.state, text))}</strong>
                {nodeId && onLocateNode ? <button className="recommendation-evidence-link" onClick={() => onLocateNode(nodeId)} title={evidenceTitle(row, text)} type="button">{where}</button> : <span className="recommendation-evidence-link" title={evidenceTitle(row, text)}>{where}</span>}
              </li>;
            })}
            {derived ? <li className="recommendation-evidence-cell" key="derived">
              <span className="recommendation-evidence-label">{measurementLabel(derived.measurement_id, text)} · {text("derived", "派生")}</span>
              <strong className="recommendation-evidence-value">{derived.display_text}</strong>
              <span className="recommendation-evidence-link" title={derived.input_refs.join("\n")}>{text(`Derivation · ${derived.input_refs.length} inputs`, `计算链 · ${derived.input_refs.length} 个输入`)}</span>
            </li> : null}
          </ul> : <p className="recommendation-empty-copy">{text("No referenced metric cell is available in this report.", "此报告中没有可用的引用指标单元。")}</p>}
          {moreEvidence > 0 ? <p className="recommendation-more-evidence">{locatable
            ? <button className="recommendation-evidence-link" onClick={() => onLocateNode?.(locatable)} type="button">{text(`${moreEvidence} more rule input(s) in Doctor`, `另有 ${moreEvidence} 项规则输入，见诊断页`)}</button>
            : text(`${moreEvidence} more rule input(s) in Doctor`, `另有 ${moreEvidence} 项规则输入，见诊断页`)}</p> : null}
        </section>
      </div>

      <aside aria-label={text("Expected outcome and verification", "预期与验证")} className="recommendation-rail">
        <section>
          <h3>{text("Expected direction", "预期方向")}</h3>
          {changes.length ? <ul className="recommendation-expected">
            {changes.map((change) => <li key={change.metric_id}><span>{changeLabel(change, location, false, text)}</span><span className="recommendation-direction">{directionText(change, text)}</span></li>)}
            <li><span>{text("Query result", "返回结果")}</span><span className={`recommendation-direction ${recommendation.result_effect}`}>{recommendation.result_effect === "unchanged" ? text("Unchanged", "不变") : text("May change", "可能变化")}</span></li>
          </ul> : <p>{expectedDirectionOf(item, text)}</p>}
          <p className="recommendation-rail-note">{text("Direction only; no magnitude is promised.", "方向判断，不承诺幅度。")}</p>
        </section>
        <section>
          <h3>{text("Risk", "风险")}</h3>
          <p className="risk-value"><strong>{riskLevelLabel(item, text)}</strong>{riskNote(item, text) ? ` · ${riskNote(item, text)}${text(".", "。")}` : ""}</p>
        </section>
        <section>
          <h3>{text("Verification plan", "验证计划")}</h3>
          <ol className="recommendation-verification">
            <li>{text("Capture a Profile of the changed query with “Runtime Top · run once” in the SQL workspace", "在 SQL 工作区以「运行时 Top · 执行一次」采集修改后的 Profile")}</li>
            <li>{text("Set the current Profile as baseline and the changed one as candidate", "将当前 Profile 设为基线，修改后为候选")}</li>
            <li>{changes.length
              ? text(`In Compare, review ${changes.map((change) => reviewLabel(change, location, text, changes)).join(" and ")}; a metric whose baseline is 0 shows only its absolute change`, `在对比中复核 ${changes.map((change) => reviewLabel(change, location, text, changes)).join(" 与 ")}；基线为 0 的指标只看绝对变化`)
              : text("In Compare, review the evidence metrics above; a metric whose baseline is 0 shows only its absolute change", "在对比中复核上方证据指标；基线为 0 的指标只看绝对变化")}</li>
          </ol>
        </section>
        <section>
          <h3>{text("Limitations", "限制")}</h3>
          <p className="recommendation-limitations">{limitations.length ? `${limitations.join(text("; ", "；"))}${text(".", "。")}` : text("None supplied by the report.", "报告未提供。")}</p>
        </section>
      </aside>
    </div>
  </article>;
}

function AdvisorGuide({ advisorResponse }: { advisorResponse: PlanAdvisorResponse }): JSX.Element {
  const { text } = useLocale();
  const recommendations = advisorResponse.recommendations;
  const provenances = orderedUnique(recommendations.flatMap((recommendation) => recommendation.advisor_provenance ? [recommendation.advisor_provenance] : []));
  return <details className="advisor-guide-panel" aria-label={text("Official Advisor status", "官方 Advisor 状态")}>
    <summary>{text("Official Advisor · experimental · FE-local · volatile", "官方 Advisor · 实验性 · FE 本地 · 易变")}</summary>
    <dl><div><dt>{text("Capability", "能力")}</dt><dd>{localizedEnum(advisorResponse.capability, text)}</dd></div><div><dt>{text("Recommendations", "建议")}</dt><dd>{recommendations.length}</dd></div>{recommendations[0] ? <div className="advisor-source"><dt>{text("Source", "来源")}</dt><dd><code>{recommendations[0].source}</code></dd></div> : null}</dl>
    {provenances.length ? <div className="advisor-tags" aria-label={text("Advisor provenance tags", "Advisor 溯源标签")}>{provenances.flatMap((provenance) => advisorProvenanceBadges(provenance, text).map((badge) => <span className={badge.className} key={`${provenance}:${badge.key}`}>{badge.label}</span>))}</div> : null}
    {recommendations.length ? <section className="advisor-guidance" data-readable-text="advisor-limitations"><p className="advisor-copy-label">{text("Guidance", "指引")}</p><p className="advisor-copy">{recommendations.map((recommendation) => localizedAdvisorText("suggested_action", recommendation.suggested_action, text)).join(" ")}</p><p className="advisor-copy-label">{text("Validation", "验证")}</p><List advisor field="validation_step" values={recommendations.flatMap((recommendation) => recommendation.recommendation.validation_steps)} /><p className="advisor-copy-label">{text("Limitations", "限制")}</p><List advisor empty={text("No limitations were supplied.", "未提供限制项。")} field="limitation" values={advisorResponse.limitations} /></section> : <p className="advisor-unavailable">{text("Advisor responded but supplied no guidance. Results are for reference only and do not take part in product rules.", "Advisor 已响应，但未提供指引。结果仅供参考，不参与产品规则判定。")}</p>}
    <p className="advisor-boundary">{text("No SQL execution · no auto-apply", "不执行 SQL · 不自动应用")}</p>
  </details>;
}

/** Actionable: the operator is located and the rule's evidence is complete; anything else is a lead. */
function actionable(item: RecommendationItem, location: MetricLocation): boolean {
  return item.kind === "product" && location.state === "located" && item.finding.evidence_completeness === "complete";
}

function StateChip({ item, location }: { item: RecommendationItem; location: MetricLocation }): JSX.Element {
  const { text } = useLocale();
  const sqlUnlocated = location.state === "located" && location.node?.sql_location?.state !== "located"
    ? text("The exact SQL or subquery location has not been established; a table name alone does not identify one reference in a self-join or repeated subquery.", "SQL / 子查询位置尚未定位；仅凭表名无法确定自关联或重复子查询中的具体引用。")
    : undefined;
  return actionable(item, location)
    ? <span className="chip accent recommendation-state actionable" title={sqlUnlocated}>{text("Actionable investigation", "可执行排查")}</span>
    : <span className="chip recommendation-state lead" title={text("A lead, not a proven bottleneck.", "线索，不是已证实的瓶颈。")}>{text("Lead to investigate", "待排查线索")}</span>;
}

function SeverityTag({ item }: { item: RecommendationItem }): JSX.Element {
  const { text } = useLocale();
  const severity = item.kind === "product" ? item.finding.severity : "experimental";
  return <span className={`recommendation-severity ${item.kind === "product" ? severityTone(severity) : "neutral"} severity-${severity.toLowerCase()}`}>{localizedEnum(severity, text)}</span>;
}

function ObjectLabel({ item, location }: { item: RecommendationItem; location: MetricLocation }): JSX.Element {
  const { text } = useLocale();
  if (item.kind === "advisor") return <code>{item.recommendation.action_kind}</code>;
  if (location.state === "query") return <span>{text("Whole query", "整条查询")}</span>;
  if (location.state !== "located") return <span>{text("Operator not located", "尚未定位算子")}</span>;
  const label = objectLabel(location);
  return label ? <code>{label}</code> : <span>{text("Operator name unavailable", "算子名称不可用")}</span>;
}

/** "HASH_JOIN · Plan 4", from the located node; null when neither is known. */
function objectLabel(location: MetricLocation): string | null {
  const plan = location.node?.plan_node_id != null ? `Plan ${location.node.plan_node_id}` : null;
  const parts = [location.operator, plan].filter(Boolean);
  return parts.length ? parts.join(" · ") : null;
}

function List({ advisor = false, field, ruleId, values, empty = "None supplied." }: { advisor?: boolean; field?: FindingTextField | AdvisorTextField; ruleId?: string; values: string[]; empty?: string }): JSX.Element {
  const { text } = useLocale();
  return values.length ? <ul>{values.map((value, index) => <li key={`${value}-${index}`}>{field ? advisor ? localizedAdvisorText(field as AdvisorTextField, value, text) : localizedFindingText(ruleId, field as FindingTextField, value, text) : value}</li>)}</ul> : <p className="recommendation-empty-copy">{empty}</p>;
}

const SEVERITY_ORDER = ["critical", "severe", "high", "moderate", "medium", "low", "info"];

/** Display tone only; the severity value itself comes from the finding DTO. */
function severityTone(severity: string): "critical" | "warn" | "neutral" {
  const index = SEVERITY_ORDER.indexOf(severity.toLowerCase());
  if (index < 0) return "neutral";
  return index <= 2 ? "critical" : index <= 4 ? "warn" : "neutral";
}

/** The rule's own steps with their bound evidence; an Advisor without steps lists its validation steps. */
function stepsOf(item: RecommendationItem, report: AnalysisSnapshotReport | null, text: LocalizedText): StepView[] {
  const recommendation = recommendationOf(item);
  const ruleId = ruleIdOf(item);
  if (item.kind === "advisor" && recommendation.steps.length === 0) {
    return recommendation.validation_steps.map((value) => ({ title: localizedAdvisorText("validation_step", value, text), detail: null, evidence: [] }));
  }
  const location = locationOf(item, report);
  const mapping: StepView[] = location.state === "unavailable" || location.state === "ambiguous"
    ? [{ title: text("Resolve the operator mapping before changing SQL", "先确认对应算子，再决定修改哪段 SQL"), detail: text("The finding's subject is not mapped to one plan node yet.", "该发现的对象尚未对应到唯一的计划节点。"), evidence: [] }]
    : [];
  return [...mapping, ...recommendation.steps.map((step) => ({
    title: localizedFindingText(ruleId, "step_title", step.title, text),
    detail: step.detail ? localizedFindingText(ruleId, "step_detail", step.detail, text) : null,
    evidence: evidenceRowsForReferences(report, step.evidence_refs).filter((row) => row.cell),
  }))];
}

const EVIDENCE_CARDS = 3;

/**
 * The evidence the recommendation acts on: cells of the metrics it expects to
 * change, then the cells its steps check, then the rule's other inputs. Three
 * cards are shown; the rest stay listed in full on the Doctor page.
 */
function recommendationEvidence(item: RecommendationItem, report: AnalysisSnapshotReport | null, steps: StepView[]): { shown: MetricEvidenceRow[]; more: number } {
  const all = evidenceRowsForReferences(report, exactMetricReferencesFor(item));
  const inputs = all.filter((row) => row.cell);
  const unresolved = all.length - inputs.length;
  const expected = recommendationOf(item).expected_changes.flatMap((change) => inputs.filter((row) => row.cell!.metric_id === change.metric_id));
  const ranked: MetricEvidenceRow[] = [];
  for (const row of [...expected, ...steps.flatMap((step) => step.evidence), ...inputs]) {
    if (!ranked.some((candidate) => candidate.reference === row.reference)) ranked.push(row);
  }
  return { shown: ranked.slice(0, EVIDENCE_CARDS), more: Math.max(0, ranked.length - EVIDENCE_CARDS) + unresolved };
}

/** Rule identity, confidence and completeness with their backend reasons. */
function findingTitle(item: Extract<RecommendationItem, { kind: "product" }>, text: LocalizedText): string {
  const finding = item.finding;
  const reasons = (item.evaluation?.evidence_completeness_reasons ?? finding.evidence_completeness_reasons)
    .map((value) => localizedFindingText(finding.rule_id, "evidence_completeness_reason", value, text));
  return [
    `${finding.rule_id}@${finding.rule_version}`,
    `${text("Confidence", "置信度")} ${localizedEnum(finding.confidence, text)}`,
    `${text("Evidence completeness", "证据完整度")} ${localizedEnum(item.evaluation?.evidence_completeness ?? finding.evidence_completeness, text)}${reasons.length ? ` (${reasons.join(" · ")})` : ""}`,
  ].join(" · ");
}

/** The physical operator a counter was printed under (when unique), then its printed precision. */
function evidenceTitle(row: MetricEvidenceRow, text: LocalizedText): string {
  return [row.physicalOperator ?? row.subject, precisionText(row.cell!, text)].filter(Boolean).join(" · ");
}

/** "OLAP_SCAN Plan 0 · PartitionsHit = 412 / 412". */
function evidenceChip(row: MetricEvidenceRow, named: Set<string>, text: LocalizedText): string {
  const cell = row.cell!;
  const plan = row.location?.node?.plan_node_id;
  const nodeId = row.location?.node?.node_id;
  const operator = nodeId && named.has(nodeId) ? null : row.subject;
  if (nodeId) named.add(nodeId);
  const where = row.location?.state === "query" ? text("Query", "整条查询") : [operator, plan != null ? `Plan ${plan}` : null].filter(Boolean).join(" ");
  return `${where ? `${where} · ` : ""}${cell.metric_id} = ${cell.display_text || localizedEnum(cell.state, text)}`;
}

/** Source reference → printed line number, by exact equality with the backend source view. */
function sourceLineNumbers(report: AnalysisSnapshotReport | null): Map<string, number> {
  const lines = new Map<string, number>();
  for (const line of report?.source_view.plain_text_lines ?? []) if (!lines.has(line.source_ref)) lines.set(line.source_ref, line.line_no);
  for (const line of report?.source_view.line_refs ?? []) if (!lines.has(line.source_ref)) lines.set(line.source_ref, line.line_no);
  return lines;
}

/** Operator family of the located node, for "Join time"-style short labels. */
function familyOf(location: MetricLocation, text: LocalizedText): string {
  const type = `${location.node?.plan_node_type ?? ""} ${location.operator ?? ""}`;
  if (type.includes("JOIN")) return "Join";
  if (type.includes("SCAN")) return text("Scan", "扫描");
  if (type.includes("AGG")) return text("Aggregate", "聚合");
  if (type.includes("EXCHANGE")) return "Exchange";
  return text("Operator", "算子");
}

const SHORT_METRIC_LABELS: Record<string, [string, string]> = {
  ProbeRows: ["probe rows", "探测行数"],
  PushRowNum: ["input rows", "输入行数"],
  PullRowNum: ["output rows", "输出行数"],
  OutputRows: ["output rows", "输出行数"],
  BytesSent: ["bytes sent", "发送字节"],
  QuerySpillBytes: ["spill bytes", "落盘字节"],
  NetworkTime: ["network time", "网络耗时"],
};

/** Short: card and verification copy ("Join 耗时"); long: the rail ("HASH_JOIN 算子耗时", raw metric ids otherwise). */
function changeLabel(change: ExpectedMetricChange, location: MetricLocation, short: boolean, text: LocalizedText, siblings: readonly ExpectedMetricChange[] = []): string {
  if (change.metric_id === "OperatorTotalTime") {
    if (!short) return text(`${location.operator ?? "Operator"} active time`, `${location.operator ?? ""} 算子耗时`.trim());
    const family = familyOf(location, text);
    const plain = text(`${family} time`, `${family} 耗时`);
    // "Scan time" would read like ScanTime: name the operator's own time explicitly.
    const clash = siblings.some((other) => other.metric_id !== change.metric_id && metricShortLabel(other.metric_id, text) === plain);
    return clash ? text(`${family} operator time`, `${family}算子耗时`) : plain;
  }
  return short ? metricShortLabel(change.metric_id, text) : change.metric_id;
}

function metricShortLabel(metricId: string, text: LocalizedText): string {
  const label = SHORT_METRIC_LABELS[metricId];
  return label ? text(...label) : metricLabel(metricId, text);
}

/** Compare shows metric ids, so the plan names them as such; operator time keeps its short name. */
function reviewLabel(change: ExpectedMetricChange, location: MetricLocation, text: LocalizedText, siblings: readonly ExpectedMetricChange[]): string {
  return change.metric_id === "OperatorTotalTime" ? changeLabel(change, location, true, text, siblings) : change.metric_id;
}

function directionText(change: ExpectedMetricChange, text: LocalizedText): string {
  return change.direction === "decrease" ? text("↓ Down", "↓ 下降") : text("↑ Up", "↑ 上升");
}

/** "↓ 探测行数 · ↓ Join 耗时", or the rule's direction sentence when it names no metric. */
function expectedSummary(item: RecommendationItem, location: MetricLocation, text: LocalizedText): string {
  const changes = recommendationOf(item).expected_changes;
  if (!changes.length) return expectedDirectionOf(item, text);
  return changes.map((change) => `${change.direction === "decrease" ? "↓" : "↑"} ${changeLabel(change, location, true, text, changes)}`).join(" · ");
}

function riskLevelLabel(item: RecommendationItem, text: LocalizedText): string {
  return localizedEnum(recommendationOf(item).risk_level, text);
}

function riskNote(item: RecommendationItem, text: LocalizedText): string {
  return item.kind === "product"
    ? localizedFindingText(item.finding.rule_id, "risk", item.finding.recommendation.risk, text)
    : localizedEnum(item.recommendation.recommendation.risk, text);
}

function locationOf(item: RecommendationItem, report: AnalysisSnapshotReport | null): MetricLocation {
  return resolveSubjectLocation(report, item.kind === "product" ? item.finding.subject_ref : null);
}

function itemKey(item: RecommendationItem | null): string {
  if (!item) return "";
  return item.kind === "product" ? item.finding.finding_id : item.recommendation.recommendation_id;
}

function ruleIdOf(item: RecommendationItem): string | undefined {
  return item.kind === "product" ? item.finding.rule_id : undefined;
}

function recommendationOf(item: RecommendationItem): Recommendation {
  return item.kind === "product" ? item.finding.recommendation : item.recommendation.recommendation;
}

function sourceLabel(item: RecommendationItem, text: LocalizedText): string {
  return item.kind === "product" ? text("Product rule", "产品规则") : text("Official Advisor · experimental", "官方 Advisor · 实验性");
}

/** "优化定位 · 产品规则" / "具体排查 · 产品规则"; anything not actionable is a lead. */
function kindLabel(item: RecommendationItem, location: MetricLocation, text: LocalizedText): string {
  if (item.kind === "advisor") return sourceLabel(item, text);
  const kind = !actionable(item, location) ? text("Lead to investigate", "待排查线索")
    : item.finding.recommendation.kind === "optimization_target" ? text("Optimization target", "优化定位") : text("Concrete check", "具体排查");
  return `${kind} · ${sourceLabel(item, text)}`;
}

function titleOf(item: RecommendationItem, text: LocalizedText): string {
  return item.kind === "product"
    ? localizedFindingText(item.finding.rule_id, "recommendation_title", item.finding.recommendation.title, text)
    : localizedAdvisorText("suggested_action", item.recommendation.suggested_action, text);
}

function expectedDirectionOf(item: RecommendationItem, text: LocalizedText): string {
  const recommendation = recommendationOf(item);
  return item.kind === "product"
    ? localizedFindingText(item.finding.rule_id, "expected_direction", recommendation.expected_direction, text)
    : localizedEnum(recommendation.expected_direction, text);
}

/** "HASH_JOIN · Plan 4: share of operator time 46%, probe 1.10 B rows. {next step}." — DTO facts only. */
function summaryOf(item: RecommendationItem, location: MetricLocation, text: LocalizedText): string {
  if (item.kind === "advisor") return localizedAdvisorText("suggested_action", item.recommendation.suggested_action, text);
  const object = location.state === "query" ? text("Whole query", "整条查询") : objectLabel(location);
  const facts = item.finding.measurements.map((measurement) => `${measurementLabel(measurement.measurement_id, text)} ${measurement.display_text}`).join(text(", ", "，"));
  const nextStep = localizedFindingText(item.finding.rule_id, "next_step", item.finding.recommendation.next_step, text);
  const lead = object && facts ? `${object}${text(": ", " ")}${facts}${text(". ", "。")}` : facts ? `${facts}${text(". ", "。")}` : "";
  return `${lead}${nextStep}${text(".", "。")}`;
}

function exactMetricReferencesFor(item: RecommendationItem): string[] {
  if (item.kind === "advisor") return orderedUnique(item.recommendation.evidence);
  const observed = item.evaluation?.input_observation_refs ?? [];
  return orderedUnique(observed.length ? observed : item.finding.evidence);
}

function advisorProvenanceBadges(value: string, text: LocalizedText): Array<{ key: string; label: string; className: string }> {
  if (value === "official_experimental_fe_local_volatile") return [
    { key: "official", label: localizedEnum("official", text), className: "official" },
    { key: "experimental", label: localizedEnum("experimental", text), className: "experimental" },
    { key: "fe_local", label: localizedEnum("fe_local", text), className: "fe-local" },
    { key: "volatile", label: localizedEnum("volatile", text), className: "volatile" },
  ];
  return [{ key: "unknown", label: value, className: "unknown" }];
}

function orderedUnique(values: readonly string[]): string[] {
  return Array.from(new Set(values));
}

/** Evidence cards use the compact count policy (`1.10 B`); the exact value stays in the title. */
function cardValue(cell: MetricCell): string | null {
  const number = cell.canonical_number;
  return cell.state === "exact" && cell.canonical_unit === "item" && number?.kind === "integer" && number.scale === null ? compactCountText(number.value) : null;
}

/** One phrase for a cell's parse state and the precision StarRocks printed it with. */
function precisionText(cell: MetricCell, text: LocalizedText): string {
  if (cell.state !== "exact") return localizedEnum(cell.state, text);
  switch (cell.fidelity) {
    case "exact": return text("Exact value", "精确值");
    case "rounded": return text("Rounded in the Profile", "Profile 中已四舍五入");
    case "truncated": return text("Truncated in the Profile", "Profile 中已截断");
    case "interval": return text("Range", "区间");
    default: return text("Precision unknown", "精度未知");
  }
}
