import { describe, expect, it } from "vitest";
import type { AnalysisSnapshotReport, GraphNode, MetricCell } from "../../../../contracts/generated/doctor-contracts";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import { resolveMetricLocation, resolveSubjectLocation } from "./recommendation-location";
function fixture(): { report: AnalysisSnapshotReport; cell: MetricCell; node: GraphNode } {
  const report: AnalysisSnapshotReport = structuredClone(sr3514JsonMinimal);
  const cell: MetricCell = { metric_cell_ref: "metric:one", subject_ref: "row:group", metric_id: "ScanTime", display_text: "—", order_rank: null, source_refs: ["source:one"], reasons: [], state: "missing", canonical_number: null, canonical_unit: null, fidelity: null, bounds: null };
  const node: GraphNode = { node_id: "node:scan", entity_kind: "plan_node", plan_node_id: "1", fragment_id: null, pipeline_id: null, plan_node_type: "OLAP_SCAN", display_name: "OLAP_SCAN", source_ordinal: 0, native_operator_refs: [{ operator_ref: "physical:one", source_ordinal: 0, source_role: "native", source_refs: ["source:one"] }], subordinate_operator_refs: [], metric_observation_refs: [], execution_state: null, source_quality: "exact", source_refs: ["source:one"], sql_facts: [{ kind: "table", value: "sales", source_refs: ["source:table"] }] };
  report.graph_view.nodes = [node];
  report.operator_rows = [{ operator_ref: cell.subject_ref, operator_type: "OLAP_SCAN", plan_node_id: "1", metric_cells: [cell], source_refs: ["source:one"] }];
  report.derived_metric_cells = [];
  return { report, cell, node };
}
describe("source-proven recommendation locations", () => {
  it("joins a public row to its plan node by typed plan_node_id and keeps only sourced SQL facts", () => {
    const { report, cell, node } = fixture();
    node.sql_facts!.push({ kind: "predicate", value: "not proved", source_refs: [] });
    const result = resolveMetricLocation(report, cell);
    expect(result.state).toBe("located"); expect(result.node).toBe(node); expect(result.operator).toBe("OLAP_SCAN");
    expect(result.facts.map(fact => fact.value)).toEqual(["sales"]);
    expect(cell.state).toBe("missing");
  });
  it("locates explicit physical membership without interpreting identifiers", () => {
    const { report, node } = fixture();
    expect(resolveSubjectLocation(report, "physical:one").node).toBe(node);
    expect(resolveSubjectLocation(report, "1").state).toBe("unavailable");
  });
  it("keeps query-level observations on the whole query", () => {
    const { report, cell } = fixture(); cell.subject_ref = "query";
    expect(resolveMetricLocation(report, cell)).toEqual({ state: "query", node: null, operator: null, facts: [] });
  });
  it("is ambiguous when two graph nodes carry the same plan node id", () => {
    const { report, cell, node } = fixture();
    report.graph_view.nodes.push({ ...node, node_id: "node:other" });
    expect(resolveMetricLocation(report, cell).state).toBe("ambiguous");
  });
  it("does not locate a partial node or a row without a plan node id", () => {
    const { report, cell, node } = fixture();
    node.source_quality = "partial";
    expect(resolveMetricLocation(report, cell).state).toBe("unavailable");
    node.source_quality = "exact";
    report.operator_rows[0].plan_node_id = null;
    expect(resolveMetricLocation(report, cell).state).toBe("unavailable");
  });
  it("rejects duplicate metrics, duplicate rows and absent subjects", () => {
    const { report, cell } = fixture();
    report.derived_metric_cells.push(cell); expect(resolveMetricLocation(report, cell).state).toBe("ambiguous"); report.derived_metric_cells = [];
    report.operator_rows.push(report.operator_rows[0]); expect(resolveSubjectLocation(report, cell.subject_ref).state).toBe("ambiguous"); report.operator_rows.pop();
    expect(resolveSubjectLocation(report, "missing").state).toBe("unavailable");
  });
});
