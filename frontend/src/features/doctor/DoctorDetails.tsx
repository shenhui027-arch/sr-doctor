import { useEffect, useRef, useState, type JSX, type ReactNode } from "react";
import type { AnalysisSnapshotReport, GraphNode, MetricCell, PlanNodeSummary } from "../../../../contracts/generated/doctor-contracts";
import { Button, MetricName, SegTabs, StateStamp, type HonestState } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { metricLabel } from "../../i18n/metric-label";
import { hotspotLabel, primarySqlFact } from "../execution/ExecutionGraph";
import { rankedSummaries, summaryUnavailableText } from "../execution/plan-node-summary";
import { dimensionLabel, findingContext, selectedFinding } from "../findings/FindingsPanel";
import { useSelection } from "../selection/selection-store";
import { SourcePanel } from "../source/SourcePanel";

type Tab = "operators" | "metrics" | "source" | "provenance";

export function honestCellState(state: string): HonestState {
  switch (state) {
    case "missing":
    case "unavailable":
      return "missing";
    case "invalid":
    case "ambiguous":
      return "invalid";
    case "partial":
      return "partial";
    case "overflow":
      return "overflow";
    case "unknown":
    case "unknown_version":
      return "unknown_version";
    case "incompatible":
      return "not_comparable";
    default:
      return "unscored";
  }
}

export function DoctorDetails({ report, focusNodeId = null, planCheckExtra = null }: { report: AnalysisSnapshotReport; focusNodeId?: string | null; planCheckExtra?: ReactNode }): JSX.Element {
  const { text } = useLocale();
  const { sourceRef } = useSelection();
  const [tab, setTab] = useState<Tab>("operators");
  const panel = useRef<HTMLElement>(null);

  useEffect(() => {
    if (sourceRef === null) return;
    setTab("source");
    panel.current?.scrollIntoView({ block: "nearest" });
  }, [sourceRef]);

  const metricCount = report.derived_metric_cells.length + report.operator_rows.reduce((sum, row) => sum + row.metric_cells.length, 0);

  return (
    <section aria-label={text("Details", "明细")} className="doctor-details" ref={panel}>
      <div className="doctor-details-bar">
        <SegTabs
          aria-label={text("Detail views", "明细视图")}
          items={[
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
      {tab === "operators" ? <OperatorTable focusNodeId={focusNodeId} report={report} />
        : tab === "metrics" ? <MetricTable report={report} />
          : tab === "source" ? <SourcePanel diagnostics={report.diagnostics} provenanceIndex={report.provenance_index} sourceView={report.source_view} />
            : <ProvenanceAndPlanCheck planCheckExtra={planCheckExtra} report={report} />}
    </section>
  );
}

export function OperatorTable({ report, focusNodeId = null }: { report: AnalysisSnapshotReport; focusNodeId?: string | null }): JSX.Element {
  const { text } = useLocale();
  const { nodeId, findingId, selectNode, revealNode } = useSelection();
  const summary = report.execution_summary;
  const nodes = new Map(report.graph_view.nodes.map((node) => [node.node_id, node] as const));

  if (summary.state !== "available") {
    return <div className="state-box"><p className="doctor-details-empty">{summaryUnavailableText(summary, text)}</p></div>;
  }

  const ranked = rankedSummaries(summary);
  const measured = new Set(ranked.map((row) => row.node_id));
  const unmeasured = report.graph_view.nodes.filter((node) => !measured.has(node.node_id));
  const maxShare = Math.max(0, ...ranked.map((row) => row.time_share_basis_points ?? 0));
  const derivedNodes = new Set(report.graph_view.edges.filter((edge) => edge.source === "verified_derived").map((edge) => edge.direction_semantics === "parent_to_child" ? edge.to : edge.from));
  const unconnected = new Set(report.graph_view.unconnected_node_ids);
  const consumers = new Set(report.graph_view.edges.map((edge) => edge.direction_semantics === "parent_to_child" ? edge.from : edge.to));
  const finding = selectedFinding(report, findingId);
  const active = nodeId ?? focusNodeId ?? (finding ? findingContext(report, finding).nodeId : null);
  const missing = <StateStamp state="missing" />;

  const renderSource = (node: GraphNode) => {
    if (unconnected.has(node.node_id) || node.source_quality === "partial") {
      return <span className="state-stamp state-stamp--warn structure-source unavailable">{text("Unavailable · unconnected", "不可用 · 未连线")}</span>;
    }
    if (derivedNodes.has(node.node_id)) {
      return <span className="state-stamp structure-source derived">{text("Verified derived", "已验证推导")}</span>;
    }
    return (
      <span className="status-inline structure-source explicit">
        <span aria-hidden="true" className="status-dot status-dot--ok" />
        <span>{text("Explicit", "显式")}</span>
      </span>
    );
  };

  return (
    <table aria-label={text("Plan node operators", "计划节点实测算子表")} className="data-table plan-node-table">
      <thead>
        <tr>
          <th>{text("Plan node", "计划节点")}</th>
          <th>{text("Object / condition", "对象 / 条件")}</th>
          <th className="num">{text("Operator time", "算子耗时")}</th>
          <th>{text("Share of operator time", "占算子总耗时")}</th>
          <th className="num">{text("Rows in", "输入行")}</th>
          <th className="num">{text("Rows out", "输出行")}</th>
          <th className="num">{text("Peak memory", "峰值内存")}</th>
          <th>{text("Structure source", "结构来源")}</th>
        </tr>
      </thead>
      <tbody>
        {ranked.map((row) => {
          const node = nodes.get(row.node_id);
          if (!node) return null;
          const isSelected = row.node_id === active;
          const shareFill = row.hotspot === "most_consuming"
            ? "share-fill share-fill--high"
            : row.hotspot === "second_most_consuming"
              ? "share-fill share-fill--mid"
              : "share-fill";
          return (
            <tr className={isSelected ? "is-selected" : undefined} key={row.node_id} onClick={() => selectNode(row.node_id)}>
              <td>
                <button
                  aria-pressed={isSelected}
                  className="row-pick-btn plan-node-link"
                  onClick={(event) => { event.stopPropagation(); revealNode(row.node_id); }}
                  type="button"
                >
                  <small className="mono">Plan {row.plan_node_id}</small> {node.display_name ?? node.plan_node_type ?? row.node_id}
                </button>
                {row.hotspot === "boundary" ? <span className="state-stamp plan-node-hot">{hotspotLabel(row, text)}</span> : null}
              </td>
              <td className="mono plan-node-fact" title={objectText(node, text)}>{objectText(node, text)}</td>
              <td className="num">{row.total_time.display_text ?? missing}</td>
              <td title={hotspotLabel(row, text) ?? undefined}>
                <span className="share-cell plan-node-share">
                  <span aria-hidden="true" className="share-track">
                    <span className={shareFill} style={{ width: `${maxShare > 0 ? (row.time_share_basis_points ?? 0) * 100 / maxShare : 0}%` }} />
                  </span>
                  <b className="mono">{row.time_share_display ?? "—"}</b>
                </span>
              </td>
              <td className="num">{consumers.has(row.node_id) ? inputRows(row) ?? "—" : "—"}</td>
              <td className="num">{row.output_rows.display_text ?? missing}</td>
              <td className="num">{peakMemory(report, node) ?? "—"}</td>
              <td>{renderSource(node)}</td>
            </tr>
          );
        })}
        {unmeasured.map((node) => {
          const isSelected = node.node_id === active;
          return (
            <tr className={`unmeasured${isSelected ? " is-selected" : ""}`} key={node.node_id} onClick={() => selectNode(node.node_id)}>
              <td>
                <button
                  aria-pressed={isSelected}
                  className="row-pick-btn plan-node-link"
                  onClick={(event) => { event.stopPropagation(); revealNode(node.node_id); }}
                  type="button"
                >
                  <small className="mono">Plan {node.plan_node_id ?? "—"}</small> {node.display_name ?? node.plan_node_type ?? node.node_id}
                </button>
              </td>
              <td className="mono plan-node-fact">{objectText(node, text)}</td>
              <td className="num">{missing}</td>
              <td><span className="mono">—</span></td>
              <td className="num">{missing}</td>
              <td className="num">{missing}</td>
              <td className="num">{missing}</td>
              <td>{renderSource(node)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function objectText(node: GraphNode, text: LocalizedText): string {
  const fact = primarySqlFact(node) ?? (node.sql_facts ?? []).find((candidate) => candidate.kind === "fields") ?? null;
  if (fact) return fact.value;
  if (node.plan_node_type?.includes("SINK")) return text("Result output", "结果出口");
  return "—";
}

function inputRows(row: PlanNodeSummary): string | null {
  const native = row.operators.filter((operator) => operator.role === "native" && operator.push_rows.state === "exact" && operator.push_rows.display_text);
  return native.length === 1 ? native[0].push_rows.display_text : null;
}

function peakMemory(report: AnalysisSnapshotReport, node: GraphNode): string | null {
  const cells: MetricCell[] = report.operator_rows
    .filter((row) => row.plan_node_id !== null && row.plan_node_id === node.plan_node_id)
    .flatMap((row) => row.metric_cells.filter((cell) => cell.metric_id === "PeakMemoryUsage" && cell.state === "exact"));
  return cells.length === 1 ? cells[0].display_text : null;
}

function lineNumberMap(report: AnalysisSnapshotReport): Map<string, number> {
  const lines = new Map<string, number>();
  for (const line of report.source_view.plain_text_lines) if (!lines.has(line.source_ref)) lines.set(line.source_ref, line.line_no);
  for (const line of report.source_view.line_refs ?? []) if (!lines.has(line.source_ref)) lines.set(line.source_ref, line.line_no);
  return lines;
}

export function MetricTable({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const { selectSourceRef } = useSelection();
  const lines = lineNumberMap(report);
  const rows = [
    ...report.derived_metric_cells.map((cell) => ({ owner: text("Whole query", "整条查询"), cell })),
    ...report.operator_rows.flatMap((row) => row.metric_cells.map((cell) => ({ owner: `${row.operator_type}${row.plan_node_id === null ? "" : ` · Plan ${row.plan_node_id}`}`, cell }))),
  ];
  if (!rows.length) {
    return <div className="state-box"><p className="doctor-details-empty">{text("No parsed metrics in this report.", "此报告没有解析出的指标。")}</p></div>;
  }
  return (
    <table aria-label={text("Parsed metric cells", "解析指标单元表")} className="data-table raw-metrics-table">
      <thead>
        <tr>
          <th>{text("Subject", "所属对象")}</th>
          <th>{text("Metric", "指标")}</th>
          <th className="num">{text("Measured value", "实测值")}</th>
          <th>{text("Parse state", "解析状态")}</th>
          <th>{text("Source line", "原文定位")}</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(({ owner, cell }) => {
          const ref = cell.source_refs[0];
          const line = ref ? lines.get(ref) : undefined;
          const localizedName = metricLabel(cell.metric_id, text);
          return (
            <tr key={cell.metric_cell_ref}>
              <td className="mono">{owner}</td>
              <td>
                <MetricName
                  label={localizedName}
                  official={localizedName !== cell.metric_id ? cell.metric_id : undefined}
                />
              </td>
              <td className="num">{cell.state === "exact" ? cell.display_text : "—"}</td>
              <td>{cell.state === "exact" ? localizedEnum(cell.state, text) : <StateStamp state={honestCellState(cell.state)} />}</td>
              <td>
                {ref ? (
                  <Button className="mono" onClick={() => selectSourceRef(ref)} size="sm" title={ref} variant="ghost">
                    {line !== undefined ? `${text("Source", "原文")} L${line} →` : `${text("Source", "原文")} →`}
                  </Button>
                ) : "—"}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

/** Input identity, version pack and per-dimension completeness with the backend's reasons. */
export function ProvenanceFacts({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const facts: [string, string][] = [
    [text("Input kind", "输入格式"), localizedEnum(report.input.detected_kind, text)],
    [text("Input size", "输入大小"), `${report.input.byte_length} B · ${report.input.line_count ?? "—"} ${text("lines", "行")}`],
    [text("Source SHA-256", "原文 SHA-256"), report.input.source_hash],
    [text("Version pack", "匹配版本包"), report.version.selected_pack ? `${report.version.selected_pack.pack_id} @ ${report.version.selected_pack.upstream_commit.slice(0, 12)}` : text("None", "无")],
    [text("Topology source", "拓扑来源"), localizedEnum(report.graph_view.topology_source, text)],
    [text("Located facts", "可回溯事实"), String(report.provenance_index.length)],
    ...(["summary", "metrics", "graph", "version", "provenance"] as const).map((key): [string, string] => [
      `${text("Completeness", "完整度")} · ${dimensionLabel(key, text)}`,
      [localizedEnum(report.completeness[key].state, text), ...report.completeness[key].reasons.map((reason) => localizedEnum(reason, text))].join(" · "),
    ]),
  ];
  return (
    <dl className="kv-grid doctor-provenance">
      {facts.map(([label, value]) => (
        <div className="doctor-provenance-row" key={label}>
          <dt>{label}</dt>
          <dd className="mono">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

/**
 * Provenance + read-only Plan Check (decided item 3):
 * Shows ONLY existing plan facts / Advisor cache linked to this query; never runs a new EXPLAIN here.
 */
export function ProvenanceAndPlanCheck({ report, planCheckExtra = null }: { report: AnalysisSnapshotReport; planCheckExtra?: ReactNode }): JSX.Element {
  const { text } = useLocale();
  const planFactNodes = report.graph_view.nodes.filter((node) => (node.sql_facts ?? []).length > 0);
  return (
    <div className="detail-stack doctor-provenance-pane">
      <div>
        <div className="field-label" style={{ marginBottom: 6 }}>{text("Input provenance & 5-dimension completeness", "输入溯源与五维完整度")}</div>
        <ProvenanceFacts report={report} />
      </div>
      <div className="doctor-plan-check-sec">
        <div className="doctor-plan-check-hd">
          <span className="field-label">{text("Read-only plan check & Plan Advisor", "只读计划检查 & Plan Advisor")}</span>
          <span className="state-stamp">{text("Plan estimates never mixed with runtime", "计划估算不与实测混算")}</span>
        </div>
        {planFactNodes.length > 0 ? (
          <dl className="kv-grid">
            {planFactNodes.map((node) => (
              <div className="doctor-provenance-row" key={node.node_id}>
                <dt className="mono">Plan {node.plan_node_id ?? node.node_id} · {node.plan_node_type ?? node.display_name ?? node.node_id}</dt>
                <dd className="mono">{(node.sql_facts ?? []).map((fact) => fact.value).join(" · ")}</dd>
              </div>
            ))}
          </dl>
        ) : null}
        {planCheckExtra ? (
          <div style={{ marginTop: 6 }}>{planCheckExtra}</div>
        ) : planFactNodes.length === 0 ? (
          <p className="doctor-plan-check-empty">
            {text(
              "No existing plan check result is linked to this Profile. To run a new EXPLAIN check, use Queries › Run SQL.",
              "当前 Profile 未关联已有计划检查结果；如需发起新的计划检查，请前往「查询 › 运行 SQL」。",
            )}
          </p>
        ) : null}
      </div>
    </div>
  );
}
