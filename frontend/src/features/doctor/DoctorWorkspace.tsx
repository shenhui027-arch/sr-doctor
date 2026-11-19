import { useEffect, useMemo, useRef, useState, type JSX, type ReactNode } from "react";
import type { AnalysisSnapshotReport, MetricCell } from "../../../../contracts/generated/doctor-contracts";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { metricReasonText } from "../../i18n/metric-label";
import { ExecutionGraph, type FindingTarget } from "../execution/ExecutionGraph";
import { OfficialProfilePanel, type OfficialProfileContext } from "../execution/OfficialProfilePanel";
import { EvidenceGaps, FindingInspector, FindingsPanel, findingContext, selectedFinding } from "../findings/FindingsPanel";
import { localizedFindingText } from "../../i18n/finding-text";
import { ImportPanel } from "../import/ImportPanel";
import { SelectionProvider, useSelection } from "../selection/selection-store";
import { DoctorDetails } from "./DoctorDetails";
import { AnimatedFigure } from "../shell/motion-enhance";
import "./doctor.css";

export type DoctorWorkspaceProps = {
  report: AnalysisSnapshotReport | null;
  importing?: boolean;
  officialContext?: OfficialProfileContext;
  /** Plan node to select on entry (e.g. "locate" from a recommendation). */
  focusNodeId?: string | null;
  onReport(report: AnalysisSnapshotReport): void;
  onCancelImport?(): void;
  /** Opens the before/after comparison from a finding's recommendation. */
  onOpenCompare?(): void;
  /** Rendered under the findings by the app (the Official Advisor guide); never fed to product rules. */
  findingsFooter?: ReactNode;
};

export function DoctorWorkspace({ report, importing = false, officialContext, focusNodeId = null, onReport, onCancelImport, onOpenCompare, findingsFooter = null }: DoctorWorkspaceProps): JSX.Element {
  const { text } = useLocale();
  if (!report || importing) return <SelectionProvider><div className="doctor-import">
    <ImportPanel onCancel={report ? onCancelImport : undefined} onReport={onReport} />
    {/* Review: when connected, the online path is an action, not a faint hint. */}
    {officialContext?.session && officialContext.onReturnToQueries
      ? <p className="doctor-import-hint doctor-import-connected">{text("No query selected yet.", "尚未选择查询。")} <button className="button secondary" onClick={officialContext.onReturnToQueries} type="button">{text("Pick one in Queries", "前往查询选择")}</button></p>
      : <p className="muted doctor-import-hint">{text("Or connect to a database and pick a query in Queries.", "也可以连接数据库，在“查询”页选择一条查询。")}</p>}
  </div></SelectionProvider>;
  return <DoctorReportView findingsFooter={findingsFooter} focusNodeId={focusNodeId} officialContext={officialContext} onOpenCompare={onOpenCompare} report={report} />;
}

function DoctorReportView({ report, officialContext, focusNodeId, onOpenCompare, findingsFooter }: { report: AnalysisSnapshotReport; officialContext?: OfficialProfileContext; focusNodeId: string | null; onOpenCompare?(): void; findingsFooter: ReactNode }): JSX.Element {
  const { text } = useLocale();
  const initialSelection = { sourceRef: null, subjectRef: null, findingId: null, nodeId: focusNodeId };
  // Task 65 (board 5): one screen. Left, the execution structure and its details behind tabs;
  // right, the findings with each one's evidence and recommendation. Each side scrolls on its own.
  return <SelectionProvider initialSelection={initialSelection} key={`${report.analysis_key}:${focusNodeId ?? ""}`}>
    <div className="doctor-layout">
      <DoctorSummary report={report} />
      <ProfileStateNotice report={report} />
      {/* Reading order stays summary → findings → structure → details; the grid shows the findings on the right. */}
      <div className="doctor-workbench">
        <aside aria-label={text("Findings", "发现")} className="doctor-findings-column">
          <FindingsPanel report={report}>
            <FindingInspector onOpenCompare={onOpenCompare} report={report} />
            <EvidenceGaps report={report} />
            {findingsFooter}
          </FindingsPanel>
        </aside>
        <DoctorStructure focusNodeId={focusNodeId} officialContext={officialContext} report={report} />
      </div>
    </div>
  </SelectionProvider>;
}

/** The execution structure and its tables share one card; evidence links and "show in graph" pick the tab. */
function DoctorStructure({ report, officialContext, focusNodeId }: { report: AnalysisSnapshotReport; officialContext?: OfficialProfileContext; focusNodeId: string | null }): JSX.Element {
  const { text } = useLocale();
  const { sourceRef, revealCount } = useSelection();
  const [tab, setTab] = useState<"graph" | "details">("graph");
  // The graph header hosts the official-reading trigger next to its view switch.
  const [viewSlot, setViewSlot] = useState<HTMLSpanElement | null>(null);
  const firstReveal = useRef(revealCount);
  useEffect(() => { if (sourceRef !== null) setTab("details"); }, [sourceRef]);
  useEffect(() => { if (revealCount !== firstReveal.current) setTab("graph"); }, [revealCount]);
  return <section aria-label={text("Execution structure", "执行结构")} className="doctor-structure">
    <div aria-label={text("Structure views", "结构视图")} className="seg-tabs doctor-structure-tabs" role="tablist">
      <button aria-selected={tab === "graph"} className={tab === "graph" ? "active" : ""} onClick={() => setTab("graph")} role="tab" type="button">{text("Execution graph", "结构图")}</button>
      <button aria-selected={tab === "details"} className={tab === "details" ? "active" : ""} onClick={() => setTab("details")} role="tab" type="button">{text("Operators and source", "算子与原文")}</button>
    </div>
    <div className="doctor-graph" hidden={tab !== "graph"}>
      <DoctorGraph report={report} viewExtra={officialContext ? <span className="graph-view-slot" ref={setViewSlot} /> : null} />
      {officialContext ? <OfficialProfilePanel context={officialContext} report={report} triggerSlot={viewSlot} /> : null}
    </div>
    <div className="doctor-structure-details" hidden={tab !== "details"}><DoctorDetails focusNodeId={focusNodeId} report={report} /></div>
  </section>;
}

/** The Doctor graph, told where each finding sits so a finding and its node point at each other. */
function DoctorGraph({ report, viewExtra = null }: { report: AnalysisSnapshotReport; viewExtra?: ReactNode }): JSX.Element {
  const { text } = useLocale();
  const targets = useMemo<FindingTarget[]>(() => report.findings.flatMap((finding) => {
    const nodeId = findingContext(report, finding).nodeId;
    return nodeId ? [{
      findingId: finding.finding_id,
      subjectRef: finding.subject_ref,
      nodeId,
      title: localizedFindingText(finding.rule_id, "category", finding.category, text),
      severity: localizedEnum(finding.severity, text),
    }] : [];
  }), [report, text]);
  return <ExecutionGraph defaultFindingId={selectedFinding(report, null)?.finding_id ?? null} findingTargets={targets} graph={report.graph_view} graphKey={report.analysis_key} sourceView={report.source_view} summary={report.execution_summary} viewExtra={viewExtra} />;
}

/**
 * A Profile the rules can not read (Level 2, or a snapshot of a running
 * query) is stated up front with the next capture step, so an empty findings
 * list is never read as a healthy query. Reads only the backend ProfileContext.
 */
function ProfileStateNotice({ report }: { report: AnalysisSnapshotReport }): JSX.Element | null {
  const { text } = useLocale();
  const context = report.profile_context;
  if (context.profile_level === "level_2" || context.merge_state === "unmerged") return <section className="profile-state-notice" role="note" aria-label={text("Profile notice", "Profile 提示")}>
    <strong>{text("Level 2 Profile: per-node analysis is not available", "Level 2 Profile：无法按计划节点分析")}</strong>
    <p>{text(
      "This Profile was captured with pipeline_profile_level=2, so its counters are reported per instance and are not merged per plan node. Node time, the execution graph and the diagnosis rules are not evaluated; an empty findings list here does not mean the query is healthy.",
      "该 Profile 以 pipeline_profile_level=2 采集，计数器按实例上报、未按计划节点合并，因此不计算节点耗时、不生成执行图、不评估诊断规则；这里没有发现不代表查询正常。",
    )}</p>
    <p className="profile-state-next">{text(
      "Next: run SET pipeline_profile_level = 1 (the default) in the session, rerun the query and import that Profile.",
      "下一步：在会话中执行 SET pipeline_profile_level = 1（默认值），重跑查询后导入新的 Profile。",
    )}</p>
  </section>;
  if (context.query_state === "running" || context.snapshot_kind === "runtime") return <section className="profile-state-notice" role="note" aria-label={text("Profile notice", "Profile 提示")}>
    <strong>{text("Running-query snapshot: values are provisional", "运行中查询的快照：数值为临时值")}</strong>
    <p>{text(
      "This Profile was taken while the query was still running, and some instances had not reported. The totals above are a point-in-time snapshot; node time and the diagnosis rules are not evaluated.",
      "该 Profile 采集时查询仍在运行，部分实例尚未上报。上方汇总只是某一时刻的快照，不计算节点耗时、不评估诊断规则。",
    )}</p>
    <p className="profile-state-next">{text(
      "Next: fetch the Profile again after the query finishes and analyze that final Profile.",
      "下一步：等查询结束后重新获取 Profile，分析最终 Profile。",
    )}</p>
  </section>;
  return null;
}

/** A backend query-scope cell, shown verbatim; never re-derived. */
export function queryMetric(report: AnalysisSnapshotReport, metricId: string): MetricCell | null {
  const cells = report.derived_metric_cells.filter((cell) => cell.metric_id === metricId);
  return cells.length === 1 && cells[0].state === "exact" ? cells[0] : null;
}

/** Display text of an exact value, or the typed state that replaces it (never 0). */
type SummaryFact = { value: string } | { state: string };

function cellFact(report: AnalysisSnapshotReport, metricId: string): SummaryFact {
  const cells = report.derived_metric_cells.filter((cell) => cell.metric_id === metricId);
  if (cells.length === 0) return { state: "missing" };
  if (cells.length > 1) return { state: "ambiguous" };
  return cells[0].state === "exact" ? { value: cells[0].display_text } : { state: cells[0].state };
}

/** The cell's backend reason codes, translated; the caveat is never inferred here. */
function cellReasons(report: AnalysisSnapshotReport, metricId: string, text: LocalizedText): string | undefined {
  const cells = report.derived_metric_cells.filter((cell) => cell.metric_id === metricId);
  return cells.length === 1 && cells[0].reasons.length ? cells[0].reasons.map((code) => metricReasonText(code, text)).join(" ") : undefined;
}

/** A summary value the Profile did not print reads as missing (never 0). */
function summaryFact(value: { state: string; display_text: string | null }): SummaryFact {
  return value.state === "exact" && value.display_text ? { value: value.display_text } : { state: "missing" };
}

function DoctorSummary({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const summary = report.execution_summary;
  // [label, fact, hint, note]: the note names the Profile counter (or says why it is absent).
  const facts: Array<[string, SummaryFact, string | undefined, string]> = [
    [text("Total time", "总耗时"), summaryFact(summary.query_total_time), undefined, "QueryTotalTime"],
    [text("Planning", "规划"), cellFact(report, "QueryPlanningTime"), cellReasons(report, "QueryPlanningTime", text), "QueryPlanningTime"],
    [text("Execution (wall)", "执行（墙钟）"), cellFact(report, "QueryExecutionWallTime"), cellReasons(report, "QueryExecutionWallTime", text), "QueryExecutionWallTime"],
    [text("Operator time (sum)", "算子累计耗时"), summaryFact(summary.cumulative_operator_time), text("Sum over operators running in parallel; not wall time.", "并行算子的耗时之和，不是墙钟时间。"), text("Sum in parallel, not wall time", "并行之和，非墙钟")],
    [text("CPU (sum)", "CPU 累计"), cellFact(report, "QueryCumulativeCpuTime"), cellReasons(report, "QueryCumulativeCpuTime", text), "QueryCumulativeCpuTime"],
    [text("Peak memory / node", "单节点峰值内存"), cellFact(report, "QueryPeakMemoryUsagePerNode"), cellReasons(report, "QueryPeakMemoryUsagePerNode", text), text("PerNode peak", "PerNode 峰值")],
    [text("Rows returned", "返回行数"), summaryFact(summary.result_rows), undefined, "ResultRows"],
  ];
  return <section className="doctor-summary" aria-label={text("Query summary", "查询概要")}>
    <dl className="doctor-summary-facts">{facts.map(([label, fact, hint, note]) => <div key={label} title={hint ?? note}>
      <dt>{label}</dt>
      <dd>{"value" in fact ? <AnimatedFigure value={fact.value} /> : <span className="metric-state-badge" data-state={fact.state}>{fact.state === "missing" ? text("Missing · not 0", "缺失 · 非 0") : localizedEnum(fact.state, text)}</span>}</dd>
      <dd className="doctor-summary-note">{"value" in fact || fact.state !== "missing" ? note : text("Not in the Profile", "Profile 未提供")}</dd>
    </div>)}</dl>
  </section>;
}
