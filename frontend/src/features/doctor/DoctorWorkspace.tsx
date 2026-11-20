import { useEffect, useMemo, useRef, useState, type JSX, type ReactNode } from "react";
import type { AnalysisSnapshotReport, MetricCell } from "../../../../contracts/generated/doctor-contracts";
import { AnimatedFigure, Button, Dialog, KpiCard, MetricName, SegTabs, StateStamp } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import { localizedFindingText } from "../../i18n/finding-text";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { metricReasonText } from "../../i18n/metric-label";
import { ExecutionGraph, type FindingTarget } from "../execution/ExecutionGraph";
import { OfficialProfilePanel, type OfficialProfileContext } from "../execution/OfficialProfilePanel";
import { summaryUnavailableText } from "../execution/plan-node-summary";
import {
  EvidenceGaps,
  findingContext,
  findingHasFooterActions,
  FindingInspectorBody,
  FindingInspectorFooter,
  FindingsPanel,
  findingTitleText,
  selectedFinding,
} from "../findings/FindingsPanel";
import { ImportPanel } from "../import/ImportPanel";
import { SelectionProvider, useSelection } from "../selection/selection-store";
import { SourcePanel } from "../source/SourcePanel";
import { honestCellState, MetricTable, OperatorTable, ProvenanceAndPlanCheck } from "./DoctorDetails";
import "./doctor.css";

export type DoctorWorkspaceProps = {
  report: AnalysisSnapshotReport | null;
  importing?: boolean;
  officialContext?: OfficialProfileContext;
  /** Plan node to select on entry (e.g. "locate" from a recommendation). */
  focusNodeId?: string | null;
  /** Optional finding opened in the right-side detail drawer (defaults to null: closed on page load). */
  initialFindingId?: string | null;
  onReport(report: AnalysisSnapshotReport): void;
  onCancelImport?(): void;
  /** Kept for App.tsx compatibility; `设为基线并对比` lives only in ContextBar in v1.1. */
  onOpenCompare?(): void;
  /** Rendered in Provenance & plan check (existing Official Advisor cache linked to the query); never fed to product rules. */
  findingsFooter?: ReactNode;
};

type DoctorMainTab = "graph" | "operators" | "metrics" | "source" | "provenance";

export function DoctorWorkspace({
  report,
  importing = false,
  officialContext,
  focusNodeId = null,
  initialFindingId = null,
  onReport,
  onCancelImport,
  findingsFooter = null,
}: DoctorWorkspaceProps): JSX.Element {
  const { text } = useLocale();
  if (!report || importing) {
    return (
      <SelectionProvider>
        <div className="workspace-stage doctor-import">
          <ImportPanel onCancel={report ? onCancelImport : undefined} onReport={onReport} />
          {officialContext?.session && officialContext.onReturnToQueries ? (
            <div className="card doctor-import-hint-card">
              <div className="card-bd doctor-import-connected">
                <span>{text("No query selected yet.", "尚未选择查询。")}</span>
                <Button onClick={officialContext.onReturnToQueries} size="sm" variant="secondary">
                  {text("Pick one in Queries", "前往查询选择")}
                </Button>
              </div>
            </div>
          ) : (
            <p className="doctor-import-hint">
              {text("Or connect to a database and pick a query in Queries.", "也可以连接数据库，在「查询」页选择一条查询。")}
            </p>
          )}
        </div>
      </SelectionProvider>
    );
  }

  return (
    <DoctorReportView
      findingsFooter={findingsFooter}
      focusNodeId={focusNodeId}
      initialFindingId={initialFindingId}
      officialContext={officialContext}
      report={report}
    />
  );
}

function DoctorReportView({
  report,
  officialContext,
  focusNodeId,
  initialFindingId,
  findingsFooter,
}: {
  report: AnalysisSnapshotReport;
  officialContext?: OfficialProfileContext;
  focusNodeId: string | null;
  initialFindingId: string | null;
  findingsFooter: ReactNode;
}): JSX.Element {
  const { text } = useLocale();
  const initialFinding = initialFindingId
    ? report.findings.find((candidate) => candidate.finding_id === initialFindingId) ?? null
    : null;
  const initialSelection = {
    sourceRef: null,
    subjectRef: initialFinding?.subject_ref ?? null,
    findingId: initialFinding?.finding_id ?? null,
    nodeId: focusNodeId,
  };
  return (
    <SelectionProvider initialSelection={initialSelection} key={`${report.analysis_key}:${focusNodeId ?? ""}:${initialFindingId ?? ""}`}>
      <div className="workspace-stage doctor-layout">
        <DoctorSummary report={report} />
        <ProfileStateNotice report={report} />
        <div className="workbench-split doctor-workbench">
          <DoctorMainCard
            findingsFooter={findingsFooter}
            focusNodeId={focusNodeId}
            officialContext={officialContext}
            report={report}
          />
          <aside aria-label={text("Findings", "发现")} className="doctor-findings-column">
            <FindingsPanel report={report}>
              {report.findings.length === 0 ? <EvidenceGaps embedded report={report} /> : null}
            </FindingsPanel>
          </aside>
        </div>
        <DoctorFindingDrawer includeEvidenceGaps report={report} />
      </div>
    </SelectionProvider>
  );
}

export function DoctorFindingDrawer({
  report,
  includeEvidenceGaps = false,
}: {
  report: AnalysisSnapshotReport;
  includeEvidenceGaps?: boolean;
}): JSX.Element {
  const { text } = useLocale();
  const { findingId, findingOpen, restoreFindingFocus, closeFinding } = useSelection();
  const finding = findingId
    ? report.findings.find((candidate) => candidate.finding_id === findingId) ?? null
    : null;
  const open = findingOpen && finding !== null;
  const hasFooter = finding !== null && findingHasFooterActions(report, finding);

  return (
    <Dialog
      footer={finding && hasFooter ? <FindingInspectorFooter finding={finding} report={report} /> : undefined}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) closeFinding();
      }}
      open={open}
      placement="right"
      restoreFocus={restoreFindingFocus}
      title={finding ? findingTitleText(finding, text) : ""}
      width="lg"
    >
      {finding ? (
        <div className="inspector-body" key={finding.finding_id}>
          <FindingInspectorBody
            finding={finding}
            includeEvidenceGaps={includeEvidenceGaps}
            report={report}
          />
        </div>
      ) : null}
    </Dialog>
  );
}

function DoctorMainCard({
  report,
  officialContext,
  focusNodeId,
  findingsFooter,
}: {
  report: AnalysisSnapshotReport;
  officialContext?: OfficialProfileContext;
  focusNodeId: string | null;
  findingsFooter: ReactNode;
}): JSX.Element {
  const { text } = useLocale();
  const { sourceRef, sourceRevealCount, revealCount } = useSelection();
  const [tab, setTab] = useState<DoctorMainTab>("graph");
  const [viewSlot, setViewSlot] = useState<HTMLSpanElement | null>(null);
  const firstReveal = useRef(revealCount);
  const firstSourceReveal = useRef(sourceRevealCount);

  useEffect(() => {
    if (sourceRef !== null) setTab("source");
  }, [sourceRef]);

  useEffect(() => {
    if (sourceRevealCount !== firstSourceReveal.current) {
      firstSourceReveal.current = sourceRevealCount;
      setTab("source");
    }
  }, [sourceRevealCount]);

  useEffect(() => {
    if (revealCount !== firstReveal.current) {
      firstReveal.current = revealCount;
      setTab("graph");
    }
  }, [revealCount]);

  const metricCount = report.derived_metric_cells.length + report.operator_rows.reduce((sum, row) => sum + row.metric_cells.length, 0);

  return (
    <section aria-label={text("Execution structure", "执行结构")} className="card card--table doctor-main-card">
      <div className="card-hd doctor-main-hd">
        <div className="card-hd-main">
          <SegTabs
            aria-label={text("Structure views", "结构视图")}
            items={[
              { value: "graph", label: text("Execution graph", "执行结构图") },
              { value: "operators", label: text("Operators", "算子"), count: report.graph_view.nodes.length },
              { value: "metrics", label: text("Metrics", "指标"), count: metricCount },
              { value: "source", label: text("Original Profile", "Profile 原文") },
              { value: "provenance", label: text("Provenance & plan check", "溯源与计划检查") },
            ]}
            onValueChange={setTab}
            size="sm"
            value={tab}
          />
        </div>
        {tab === "graph" && officialContext ? (
          <div className="card-hd-actions">
            <span className="graph-view-slot" ref={setViewSlot} />
          </div>
        ) : null}
      </div>
      <div className="card-bd card-bd--flush doctor-main-body">
        {tab === "graph" ? (
          <div className="doctor-graph">
            {report.execution_summary.state !== "available" ? (
              <p className="doctor-summary-unavailable muted" role="status">
                {summaryUnavailableText(report.execution_summary, text)}
              </p>
            ) : null}
            <DoctorGraph report={report} />
            {officialContext ? <OfficialProfilePanel context={officialContext} report={report} triggerSlot={viewSlot} /> : null}
          </div>
        ) : tab === "operators" ? (
          <OperatorTable focusNodeId={focusNodeId} report={report} />
        ) : tab === "metrics" ? (
          <MetricTable report={report} />
        ) : tab === "source" ? (
          <SourcePanel diagnostics={report.diagnostics} provenanceIndex={report.provenance_index} sourceView={report.source_view} />
        ) : (
          <ProvenanceAndPlanCheck planCheckExtra={findingsFooter} report={report} />
        )}
      </div>
    </section>
  );
}

function DoctorGraph({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const targets = useMemo<FindingTarget[]>(
    () =>
      report.findings.flatMap((finding) => {
        const nodeId = findingContext(report, finding).nodeId;
        return nodeId
          ? [
              {
                findingId: finding.finding_id,
                subjectRef: finding.subject_ref,
                nodeId,
                title: localizedFindingText(finding.rule_id, "category", finding.category, text),
                severity: localizedEnum(finding.severity, text),
                severityCode: finding.severity,
              },
            ]
          : [];
      }),
    [report, text],
  );
  return (
    <ExecutionGraph
      defaultFindingId={selectedFinding(report, null)?.finding_id ?? null}
      embedded
      findingTargets={targets}
      graph={report.graph_view}
      graphKey={report.analysis_key}
      sourceView={report.source_view}
      summary={report.execution_summary}
    />
  );
}

function ProfileStateNotice({ report }: { report: AnalysisSnapshotReport }): JSX.Element | null {
  const { text } = useLocale();
  const context = report.profile_context;
  if (context.profile_level === "level_2" || context.merge_state === "unmerged") {
    return (
      <section aria-label={text("Profile notice", "Profile 提示")} className="notice-banner notice-banner--warn profile-state-notice" role="note">
        <div>
          <strong>{text("Level 2 Profile: per-node analysis is not available", "Level 2 Profile：无法按计划节点分析")}</strong>{" "}
          <span>
            {text(
              "Captured with pipeline_profile_level=2; counters are not merged per plan node, so node time, execution graph and diagnosis rules are not evaluated.",
              "该 Profile 以 pipeline_profile_level=2 采集，未按计划节点合并，不计算节点耗时、不生成执行图、不评估诊断规则；没有发现不代表查询正常。",
            )}{" "}
            {text(
              "Next: run SET pipeline_profile_level = 1, rerun the query and import that Profile.",
              "下一步：执行 SET pipeline_profile_level = 1，重跑查询后导入新 Profile。",
            )}
          </span>
        </div>
      </section>
    );
  }
  if (context.query_state === "running" || context.snapshot_kind === "runtime") {
    return (
      <section aria-label={text("Profile notice", "Profile 提示")} className="notice-banner notice-banner--warn profile-state-notice" role="note">
        <div>
          <strong>{text("Running-query snapshot: values are provisional", "运行中查询的快照：数值为临时值")}</strong>{" "}
          <span>
            {text(
              "Taken while the query was still running; node time and diagnosis rules are not evaluated.",
              "采集时查询仍在运行，部分实例尚未上报，不计算节点耗时、不评估诊断规则。",
            )}{" "}
            {text(
              "Next: fetch the Profile again after the query finishes.",
              "下一步：等查询结束后重新获取 Profile，分析最终 Profile。",
            )}
          </span>
        </div>
      </section>
    );
  }
  return null;
}

/** A backend query-scope cell, shown verbatim; never re-derived. */
export function queryMetric(report: AnalysisSnapshotReport, metricId: string): MetricCell | null {
  const cells = report.derived_metric_cells.filter((cell) => cell.metric_id === metricId);
  return cells.length === 1 && cells[0].state === "exact" ? cells[0] : null;
}

type SummaryFact = { value: string } | { state: string };

function cellFact(report: AnalysisSnapshotReport, metricId: string): SummaryFact {
  const cells = report.derived_metric_cells.filter((cell) => cell.metric_id === metricId);
  if (cells.length === 0) return { state: "missing" };
  if (cells.length > 1) return { state: "ambiguous" };
  return cells[0].state === "exact" ? { value: cells[0].display_text } : { state: cells[0].state };
}

function cellReasons(report: AnalysisSnapshotReport, metricId: string, text: LocalizedText): string | undefined {
  const cells = report.derived_metric_cells.filter((cell) => cell.metric_id === metricId);
  return cells.length === 1 && cells[0].reasons.length
    ? cells[0].reasons.map((code) => metricReasonText(code, text)).join(" ")
    : undefined;
}

function summaryFact(value: { state: string; display_text: string | null }): SummaryFact {
  return value.state === "exact" && value.display_text ? { value: value.display_text } : { state: "missing" };
}

type DoctorMetricSpec = {
  key: string;
  label: string;
  official: string;
  fact: SummaryFact;
  info?: string;
  caveat?: string;
};

function DoctorSummary({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const summary = report.execution_summary;
  const specs: DoctorMetricSpec[] = [
    {
      key: "total",
      label: text("Total time", "总耗时"),
      official: "QueryTotalTime",
      fact: summaryFact(summary.query_total_time),
    },
    {
      key: "planning",
      label: text("Planning time", "规划耗时"),
      official: "QueryPlanningTime",
      fact: cellFact(report, "QueryPlanningTime"),
      caveat: cellReasons(report, "QueryPlanningTime", text),
    },
    {
      key: "execution",
      label: text("Execution wall time", "执行墙钟"),
      official: "QueryExecutionWallTime",
      fact: cellFact(report, "QueryExecutionWallTime"),
      caveat: cellReasons(report, "QueryExecutionWallTime", text),
    },
    {
      key: "operator_sum",
      label: text("Operator time (sum)", "算子累计耗时"),
      official: "QueryCumulativeOperatorTime",
      fact: summaryFact(summary.cumulative_operator_time),
      info: text("Sum over operators running in parallel; not wall time.", "并行算子的耗时之和，不是墙钟时间。"),
    },
    {
      key: "cpu",
      label: text("CPU (sum)", "CPU 累计"),
      official: "QueryCumulativeCpuTime",
      fact: cellFact(report, "QueryCumulativeCpuTime"),
      caveat: cellReasons(report, "QueryCumulativeCpuTime", text),
    },
    {
      key: "peak_memory",
      label: text("Peak memory / node", "单节点峰值内存"),
      official: "QueryPeakMemoryUsagePerNode",
      fact: cellFact(report, "QueryPeakMemoryUsagePerNode"),
      caveat: cellReasons(report, "QueryPeakMemoryUsagePerNode", text),
    },
    {
      key: "result_rows",
      label: text("Rows returned", "返回行数"),
      official: "ResultRows",
      fact: summaryFact(summary.result_rows),
    },
  ];

  return (
    <section aria-label={text("Query summary", "查询概要")} className="kpi-grid kpi-grid--7 doctor-summary">
      {specs.map((spec) => (
        <KpiCard
          foot={spec.caveat ? <span title={spec.caveat}>{spec.caveat}</span> : undefined}
          info={spec.info}
          key={spec.key}
          label={<MetricName label={spec.label} official={spec.official} />}
          value={
            "value" in spec.fact ? (
              <AnimatedFigure value={spec.fact.value} />
            ) : (
              <StateStamp state={honestCellState(spec.fact.state)} />
            )
          }
        />
      ))}
    </section>
  );
}
