import type { AnalysisSnapshotReport, GraphNode, GraphSqlFact, MetricCell } from "../../../../contracts/generated/doctor-contracts";

export type MetricLocation = {
  state: "located" | "query" | "unavailable" | "ambiguous";
  node: GraphNode | null;
  operator: string | null;
  facts: GraphSqlFact[];
};
const empty = (state: MetricLocation["state"]): MetricLocation => ({ state, node: null, operator: null, facts: [] });

/**
 * Identity-only lookup within one report: a public operator row joins the
 * graph through its typed `plan_node_id`; a physical operator ref through the
 * node's explicit member list. Nothing is parsed from identifier text.
 */
export function resolveSubjectLocation(report: AnalysisSnapshotReport | null, subjectRef: string | null): MetricLocation {
  if (!report || !subjectRef) return empty("unavailable");
  if (subjectRef === "query") return empty("query");
  const nodes = report.graph_view.nodes;
  const rows = report.operator_rows.filter(row => row.operator_ref === subjectRef);
  if (rows.length > 1) return empty("ambiguous");
  const row = rows[0];
  const matches = row
    ? row.plan_node_id === null ? [] : nodes.filter(node => node.plan_node_id === row.plan_node_id)
    : nodes.filter(node => [...node.native_operator_refs, ...node.subordinate_operator_refs].some(ref => ref.operator_ref === subjectRef));
  if (matches.length > 1) return empty("ambiguous");
  if (matches.length !== 1) return empty("unavailable");
  const node = matches[0];
  if (node.source_quality === "partial") return empty("unavailable");
  return {
    state: "located", node,
    operator: row?.operator_type || node.display_name || node.plan_node_type || null,
    facts: (node.sql_facts ?? []).filter(fact => fact.value.trim().length > 0 && fact.source_refs.length > 0 && fact.source_refs.every(ref => ref.trim().length > 0)),
  };
}

export function resolveMetricLocation(report: AnalysisSnapshotReport | null, cell: MetricCell): MetricLocation {
  if (!report) return empty("unavailable");
  const matches = [...report.operator_rows.flatMap(row => row.metric_cells), ...report.derived_metric_cells].filter(candidate => candidate.metric_cell_ref === cell.metric_cell_ref);
  if (matches.length > 1) return empty("ambiguous");
  if (matches.length !== 1 || matches[0].subject_ref !== cell.subject_ref || matches[0].metric_id !== cell.metric_id) return empty("unavailable");
  return resolveSubjectLocation(report, matches[0].subject_ref);
}

/**
 * The physical operator an evidence cell was printed under, joined by the
 * exact source line the backend attached to both the cell and the plan node
 * summary's operator; `null` unless exactly one operator matches.
 */
export function physicalOperatorOf(report: AnalysisSnapshotReport, cell: MetricCell, nodeId: string | null): string | null {
  const type = physicalOperatorTypeOf(report, cell, nodeId);
  if (!type) return null;
  const node = report.graph_view.nodes.find((candidate) => candidate.node_id === nodeId);
  return `${type}${node?.plan_node_id ? ` #${node.plan_node_id}` : ""}`;
}

/** Same exact source-line join as `physicalOperatorOf`, returning only the operator type. */
export function physicalOperatorTypeOf(report: AnalysisSnapshotReport, cell: MetricCell, nodeId: string | null): string | null {
  const line = cell.source_refs[0];
  if (!line || !nodeId || report.execution_summary.state !== "available") return null;
  const summary = report.execution_summary.nodes.find((candidate) => candidate.node_id === nodeId);
  if (!summary) return null;
  const matches = summary.operators.filter((operator) => [operator.operator_time, operator.push_rows, operator.pull_rows].some((value) => value.source_refs.includes(line)));
  return matches.length === 1 ? matches[0].operator_type : null;
}
