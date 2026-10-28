// Design board 02, bottom: one tabbed panel for the per-operator table, every
// parsed metric, the original Profile text and the input's provenance. Values
// are DTO facts; nothing is summed or re-derived here.
import { useEffect, useRef, useState, type JSX } from "react";
import type { AnalysisSnapshotReport, GraphNode, MetricCell, PlanNodeSummary } from "../../../../contracts/generated/doctor-contracts";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { hotspotLabel, primarySqlFact } from "../execution/ExecutionGraph";
import { rankedSummaries, summaryUnavailableText } from "../execution/plan-node-summary";
import { dimensionLabel, findingContext, selectedFinding } from "../findings/FindingsPanel";
import { useSelection } from "../selection/selection-store";
import { SourcePanel } from "../source/SourcePanel";

type Tab = "operators" | "metrics" | "source" | "provenance";

export function DoctorDetails({ report, focusNodeId = null }: { report: AnalysisSnapshotReport; focusNodeId?: string | null }): JSX.Element {
  const { text } = useLocale();
  const { sourceRef } = useSelection();
  const [tab, setTab] = useState<Tab>("operators");
  const panel = useRef<HTMLElement>(null);
  // Evidence picked anywhere (a "Source L214" link) opens the original text.
  useEffect(() => {
    if (sourceRef === null) return;
    setTab("source");
    panel.current?.scrollIntoView({ block: "nearest" });
  }, [sourceRef]);
  const tabs: [Tab, string, string | null][] = [
    ["operators", text("Operators", "算子"), String(report.graph_view.nodes.length)],
    ["metrics", text("Metrics", "指标"), null],
    ["source", text("Original Profile", "Profile 原文"), null],
    ["provenance", text("Provenance", "溯源"), null],
  ];
  return <section aria-label={text("Details", "明细")} className="doctor-details" ref={panel}>
    <div aria-label={text("Detail views", "明细视图")} className="doctor-details-tabs" role="tablist">
      {tabs.map(([id, label, count]) => <button aria-selected={tab === id} key={id} onClick={() => setTab(id)} role="tab" type="button">{label}{count ? <span>{count}</span> : null}</button>)}
      <span className="doctor-details-hint">{text("Click a row to locate it in the graph", "点击行在图中定位")}</span>
    </div>
    {tab === "operators" ? <OperatorTable focusNodeId={focusNodeId} report={report} />
      : tab === "metrics" ? <MetricTable report={report} />
      : tab === "source" ? <SourcePanel diagnostics={report.diagnostics} provenanceIndex={report.provenance_index} sourceView={report.source_view} />
      : <ProvenanceFacts report={report} />}
  </section>;
}

function OperatorTable({ report, focusNodeId }: { report: AnalysisSnapshotReport; focusNodeId: string | null }): JSX.Element {
  const { text } = useLocale();
  const { nodeId, findingId, selectNode } = useSelection();
  const summary = report.execution_summary;
  const nodes = new Map(report.graph_view.nodes.map((node) => [node.node_id, node] as const));
  // No per-node summary: say why (from the backend reasons) instead of a table of blanks.
  if (summary.state !== "available") return <p className="doctor-details-empty">{summaryUnavailableText(summary, text)}</p>;
  const ranked = rankedSummaries(summary);
  const measured = new Set(ranked.map((row) => row.node_id));
  const unmeasured = report.graph_view.nodes.filter((node) => !measured.has(node.node_id));
  const maxShare = Math.max(0, ...ranked.map((row) => row.time_share_basis_points ?? 0));
  // A node is "derived" when the link to its consumer was derived, not printed.
  const derivedNodes = new Set(report.graph_view.edges.filter((edge) => edge.source === "verified_derived").map((edge) => edge.direction_semantics === "parent_to_child" ? edge.to : edge.from));
  const unconnected = new Set(report.graph_view.unconnected_node_ids);
  // A source (no input edge in the DTO) has no input rows to show, whatever its operator counted.
  const consumers = new Set(report.graph_view.edges.map((edge) => edge.direction_semantics === "parent_to_child" ? edge.from : edge.to));
  const finding = selectedFinding(report, findingId);
  // The row in focus follows the graph: a picked node, else the selected finding's node.
  const active = nodeId ?? focusNodeId ?? (finding ? findingContext(report, finding).nodeId : null);
  const missing = <span className="metric-unavailable">{text("Missing", "缺失")}</span>;
  const source = (node: GraphNode) => unconnected.has(node.node_id) || node.source_quality === "partial"
    ? <span className="structure-source unavailable">{text("Unavailable", "不可用")}</span>
    : derivedNodes.has(node.node_id) ? <span className="structure-source derived">{text("Verified derived", "已验证推导")}</span>
    : <span className="structure-source explicit">{text("Explicit", "显式")}</span>;
  return <div className="doctor-details-table-wrap"><table className="plan-node-table">
    <thead><tr>
      <th>{text("Plan node", "计划节点")}</th><th>{text("Object", "对象")}</th><th className="num">{text("Operator time", "算子耗时")}</th><th className="share">{text("Share of operator time", "占算子总耗时")}</th>
      <th className="num">{text("Rows in", "输入行")}</th><th className="num">{text("Rows out", "输出行")}</th><th className="num">{text("Peak memory", "峰值内存")}</th><th>{text("Structure source", "结构来源")}</th>
    </tr></thead>
    <tbody>
      {ranked.map((row) => {
        const node = nodes.get(row.node_id);
        if (!node) return null;
        const hot = row.hotspot === "most_consuming";
        const tone = hot ? "hot-most" : row.hotspot === "second_most_consuming" ? "hot-second" : "";
        return <tr className={[row.node_id === active ? "selected" : "", tone].filter(Boolean).join(" ")} key={row.node_id} onClick={() => selectNode(row.node_id)}>
          <td><button className="plan-node-link" onClick={(event) => { event.stopPropagation(); selectNode(row.node_id); }} type="button"><small>{row.plan_node_id}</small> {node.display_name ?? node.plan_node_type ?? row.node_id}</button>{row.hotspot === "boundary" ? <em className="plan-node-hot">{hotspotLabel(row, text)}</em> : null}</td>
          <td className="plan-node-fact" title={objectText(node, text)}>{objectText(node, text)}</td>
          <td className="num time">{row.total_time.display_text ?? missing}</td>
          <td className="share" title={hotspotLabel(row, text) ?? undefined}><span className="plan-node-share"><span className="share-bar" aria-hidden="true"><span style={{ width: `${maxShare > 0 ? (row.time_share_basis_points ?? 0) * 100 / maxShare : 0}%` }} /></span><b>{row.time_share_display ?? "—"}</b></span></td>
          <td className="num">{consumers.has(row.node_id) ? inputRows(row) ?? "—" : "—"}</td>
          <td className="num">{row.output_rows.display_text ?? missing}</td>
          <td className="num">{peakMemory(report, node) ?? "—"}</td>
          <td>{source(node)}</td>
        </tr>;
      })}
      {unmeasured.map((node) => <tr className="unmeasured" key={node.node_id} onClick={() => selectNode(node.node_id)}>
        <td><button className="plan-node-link" onClick={(event) => { event.stopPropagation(); selectNode(node.node_id); }} type="button"><small>{node.plan_node_id ?? "—"}</small> {node.display_name ?? node.plan_node_type ?? node.node_id}</button></td>
        <td className="plan-node-fact">{objectText(node, text)}</td>
        <td className="num">{missing}</td>
        <td className="share"><span className="plan-node-share"><span className="share-bar" aria-hidden="true"><span style={{ width: 0 }} /></span><b>—</b></span></td>
        <td className="num">{missing}</td><td className="num">{missing}</td><td className="num">{missing}</td>
        <td>{source(node)}</td>
      </tr>)}
    </tbody>
  </table></div>;
}

/** The node's primary plan fact (table, join condition, predicate or fields), or its role. */
function objectText(node: GraphNode, text: LocalizedText): string {
  const fact = primarySqlFact(node) ?? (node.sql_facts ?? []).find((candidate) => candidate.kind === "fields") ?? null;
  if (fact) return fact.value;
  if (node.plan_node_type?.includes("SINK")) return text("Result output", "结果出口");
  return "—";
}

/** Rows pushed into the node's single native operator; never a sum across operators. */
function inputRows(row: PlanNodeSummary): string | null {
  const native = row.operators.filter((operator) => operator.role === "native" && operator.push_rows.state === "exact" && operator.push_rows.display_text);
  return native.length === 1 ? native[0].push_rows.display_text : null;
}

/** The node's own PeakMemoryUsage cell, when exactly one exact cell exists. */
function peakMemory(report: AnalysisSnapshotReport, node: GraphNode): string | null {
  const cells: MetricCell[] = report.operator_rows
    .filter((row) => row.plan_node_id !== null && row.plan_node_id === node.plan_node_id)
    .flatMap((row) => row.metric_cells.filter((cell) => cell.metric_id === "PeakMemoryUsage" && cell.state === "exact"));
  return cells.length === 1 ? cells[0].display_text : null;
}

function MetricTable({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const { selectSourceRef } = useSelection();
  const rows = [
    ...report.derived_metric_cells.map((cell) => ({ owner: text("Query", "整条查询"), cell })),
    ...report.operator_rows.flatMap((row) => row.metric_cells.map((cell) => ({ owner: `${row.operator_type}${row.plan_node_id === null ? "" : ` · Plan ${row.plan_node_id}`}`, cell }))),
  ];
  if (!rows.length) return <p className="doctor-details-empty">{text("No parsed metrics in this report.", "此报告没有解析出的指标。")}</p>;
  return <div className="doctor-details-table-wrap"><table className="raw-metrics-table">
    <thead><tr><th>{text("Subject", "对象")}</th><th>{text("Metric", "指标")}</th><th className="num">{text("Value", "数值")}</th><th>{text("State", "状态")}</th><th>{text("Source", "来源")}</th></tr></thead>
    <tbody>{rows.map(({ owner, cell }) => <tr key={cell.metric_cell_ref}>
      <td>{owner}</td><td className="mono">{cell.metric_id}</td><td className="num">{cell.state === "exact" ? cell.display_text : "—"}</td><td>{localizedEnum(cell.state, text)}</td>
      <td>{cell.source_refs[0] ? <button className="reference-button" onClick={() => selectSourceRef(cell.source_refs[0])} type="button">{cell.source_refs[0]}</button> : "—"}</td>
    </tr>)}</tbody>
  </table></div>;
}

/** Input identity, version pack and per-dimension completeness with the backend's reasons. */
export function ProvenanceFacts({ report }: { report: AnalysisSnapshotReport }): JSX.Element {
  const { text } = useLocale();
  const facts: [string, string][] = [
    [text("Input kind", "输入格式"), localizedEnum(report.input.detected_kind, text)],
    [text("Input size", "输入大小"), `${report.input.byte_length} B · ${report.input.line_count ?? "—"} ${text("lines", "行")}`],
    [text("Source hash", "原文哈希"), report.input.source_hash],
    [text("Version pack", "版本包"), report.version.selected_pack ? `${report.version.selected_pack.pack_id} @ ${report.version.selected_pack.upstream_commit.slice(0, 12)}` : text("None", "无")],
    [text("Topology source", "拓扑来源"), localizedEnum(report.graph_view.topology_source, text)],
    [text("Located facts", "可回溯事实"), String(report.provenance_index.length)],
    ...(["summary", "metrics", "graph", "version", "provenance"] as const).map((key): [string, string] => [
      `${text("Completeness", "完整度")} · ${dimensionLabel(key, text)}`,
      [localizedEnum(report.completeness[key].state, text), ...report.completeness[key].reasons.map((reason) => localizedEnum(reason, text))].join(" · "),
    ]),
  ];
  return <dl className="doctor-provenance">{facts.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>;
}
