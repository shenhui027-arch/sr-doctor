import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AnalysisSnapshotReport, GraphEdge, GraphView } from "../../../../contracts/generated/doctor-contracts";
import { sr3514ExecutionGraph } from "../../../../contracts/generated/execution-graph-fixture";
import { sr3514TextMinimal } from "../../../../contracts/generated/contract-fixtures";
import { LocaleProvider } from "../../i18n/locale";
import { SelectionProvider } from "../selection/selection-store";
import { ExecutionGraph, flowEdgeLabel, flowEdges, fragmentGroups, hottestNodeId, labelAnchor, operatorCategory, primarySqlFact, roundedPath } from "./ExecutionGraph";
import { DoctorDetails } from "../doctor/DoctorDetails";
import { doctorReport } from "../../visual/demo-doctor";
import { rankedSummaries } from "./plan-node-summary";

const report = sr3514ExecutionGraph;
const edge = (overrides: Partial<GraphEdge>): GraphEdge => ({ edge_id: "e", from: "a", to: "b", relation: "plan_child", direction_semantics: "parent_to_child", source: "explicit", quality: "exact", source_ordinal: null, input_role: null, derivation_recipe: null, restates_edge_id: null, evidence_refs: [], ...overrides });
const graph = (edges: GraphEdge[]): GraphView => ({ ...report.graph_view, nodes: ["a", "b", "c"].map((id) => ({ ...report.graph_view.nodes[0], node_id: id, plan_node_id: id })), edges });

describe("execution graph relations", () => {
  it("draws plan_child edges in data-flow direction (child → parent)", () => {
    expect(flowEdges(graph([edge({ edge_id: "p", from: "a", to: "b" })]))).toEqual([{ id: "p", source: "b", target: "a", derived: false, partial: false, role: null }]);
  });
  it("skips only the edge the backend marks as restating another", () => {
    const marked = flowEdges(graph([
      edge({ edge_id: "plan", from: "a", to: "b" }),
      edge({ edge_id: "sink", from: "b", to: "a", relation: "fragment_sink", direction_semantics: "upstream_to_downstream", restates_edge_id: "plan" }),
    ]));
    expect(marked.map((flow) => flow.id)).toEqual(["plan"]);
    // Without the backend marker the frontend does not judge: both DTO edges are drawn.
    const unmarked = flowEdges(graph([
      edge({ edge_id: "plan", from: "a", to: "b" }),
      edge({ edge_id: "sink", from: "b", to: "a", relation: "fragment_sink", direction_semantics: "upstream_to_downstream" }),
    ]));
    expect(unmarked.map((flow) => flow.id)).toEqual(["plan", "sink"]);
  });
  it("keeps derived and partial qualities and never invents an edge to an unknown node", () => {
    const edges = flowEdges(graph([
      edge({ edge_id: "d", from: "b", to: "c", source: "verified_derived", quality: "partial", direction_semantics: "upstream_to_downstream" }),
      edge({ edge_id: "x", from: "a", to: "missing" }),
    ]));
    expect(edges).toEqual([{ id: "d", source: "b", target: "c", derived: true, partial: true, role: null }]);
  });
  it("frames fragments only when the DTO places nodes in two or more", () => {
    const summaries = new Map(report.execution_summary.nodes.map((node) => [node.node_id, node] as const));
    expect(fragmentGroups(report.graph_view, summaries)).toEqual([]);
    const split = new Map([...summaries].map(([id, node], index) => [id, { ...node, fragment_ref: String(index % 2) }] as const));
    expect(fragmentGroups(report.graph_view, split).map((group) => group.fragment).sort()).toEqual(["0", "1"]);
  });
  it("selects the hottest node by backend basis points, and none without a summary", () => {
    expect(hottestNodeId(report.execution_summary)).toBe("0");
    expect(hottestNodeId(sr3514TextMinimal.execution_summary)).toBeNull();
    expect(primarySqlFact({ ...report.graph_view.nodes[0], sql_facts: [{ kind: "predicate", value: "x > 1", source_refs: ["s"] }, { kind: "table", value: "sales", source_refs: ["s"] }] })?.value).toBe("sales");
  });
});

describe("node inspector", () => {
  const render = (nodeId: string) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider initialSelection={{ sourceRef: null, subjectRef: null, findingId: null, nodeId }}><ExecutionGraph graph={report.graph_view} graphKey="k" summary={report.execution_summary} /></SelectionProvider></LocaleProvider>);
  it("shows the backend time, share, hotspot tier, rows and per-operator rows", () => {
    const html = render("0");
    expect(html).toContain("已选计划节点");
    expect(html).toContain("4.229 s");
    expect(html).toContain("99.96%");
    expect(html).toContain("最耗时");
    expect(html).toContain("819");
    expect(html).toContain("运行时算子");
  });
  it("does not render an inspector without a selection", () => {
    expect(renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider><ExecutionGraph graph={report.graph_view} graphKey="k" summary={report.execution_summary} /></SelectionProvider></LocaleProvider>)).not.toContain("已选计划节点");
  });
});

describe("plan node table (Doctor details · operators)", () => {
  const details = (value: AnalysisSnapshotReport) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider><DoctorDetails report={value} /></SelectionProvider></LocaleProvider>);
  it("ranks by backend share, then lists plan nodes without a summary as missing", () => {
    expect(rankedSummaries(report.execution_summary).map((node) => node.node_id)).toEqual(["0", "2", "1"]);
    const html = details(report);
    expect(html).toContain("占算子总耗时");
    expect(html.match(/class="plan-node-link"/g)).toHaveLength(report.graph_view.nodes.length);
    expect(html.indexOf("SCHEMA_SCAN")).toBeLessThan(html.indexOf("AGGREGATION"));
    const unmeasured = report.graph_view.nodes.length - report.execution_summary.nodes.length;
    expect(html.match(/<tr class="unmeasured"/g) ?? []).toHaveLength(unmeasured);
  });
  it("marks only the input whose consumer link was derived, and an unconnected node as unavailable", () => {
    const html = details(doctorReport());
    const row = (marker: string) => { const start = html.indexOf(marker); return html.slice(start, html.indexOf("</tr>", start)); };
    // Board 02: EXCHANGE (plan 2) reaches HASH_JOIN by a verified-derived edge; PROJECT (plan 5) is unconnected.
    expect(html.match(/structure-source derived/g)).toHaveLength(1);
    expect(row("2</small> EXCHANGE")).toContain("structure-source derived");
    expect(row("4</small> HASH_JOIN")).toContain("structure-source explicit");
    expect(row("5</small> PROJECT")).toContain("structure-source unavailable");
    // The table never sums or invents: a node without a summary row reads "missing".
    expect(row("5</small> PROJECT")).toContain("缺失");
    // A source has no input rows even when its operator printed a push count (real 3.5.14 SCHEMA_SCAN: 0).
    const scan = details(report);
    const scanRow = scan.slice(scan.indexOf("0</small> SCHEMA_SCAN"), scan.indexOf("</tr>", scan.indexOf("0</small> SCHEMA_SCAN")));
    expect(scanRow).toContain('</b></span></td><td class="num">—</td>');
  });
  it("labels a backend boundary tier as undecided instead of picking a side", () => {
    const summary = { ...report.execution_summary, nodes: report.execution_summary.nodes.map((node) => node.node_id === "0" ? { ...node, hotspot: "boundary" as const } : node) };
    const html = details({ ...report, execution_summary: summary });
    expect(html).toContain("临界");
    expect(html).not.toContain("最耗时");
  });
  it("explains an unavailable summary instead of rendering zeros", () => {
    const html = details(sr3514TextMinimal);
    expect(html).not.toContain("plan-node-link");
    expect(html).toContain("无法按节点汇总耗时");
  });
  it("names the backend reason for a running or incomplete Profile", () => {
    const html = (reasons: string[]) => details({ ...sr3514TextMinimal, operator_rows: [], execution_summary: { ...sr3514TextMinimal.execution_summary, state: "unavailable", nodes: [], reasons } });
    expect(html(["profile_running_snapshot"])).toContain("查询仍在运行");
    expect(html(["runtime_profile_missing_instances"])).toContain("标记该 Profile 不完整");
    expect(html(["profile_error"])).toContain("查询执行失败");
    expect(html(["profile_running_snapshot"])).not.toContain("plan-node-link");
  });
});

describe("node inspector source lines and summary limits (v0.6-07)", () => {
  it("names source references by their Profile line and translates summary limit codes", () => {
    const node = report.execution_summary.nodes.find((candidate) => candidate.node_id === "0")!;
    const summary = { ...report.execution_summary, nodes: report.execution_summary.nodes.map((candidate) => candidate === node ? { ...candidate, reasons: ["operator_role_unknown"] } : candidate) };
    const refs = [...new Set([...report.graph_view.nodes.find((candidate) => candidate.node_id === "0")!.source_refs, ...node.total_time.source_refs, ...node.output_rows.source_refs])];
    expect(refs.length).toBeGreaterThan(1);
    const inspect = (sourceView: typeof report.source_view) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider initialSelection={{ sourceRef: null, subjectRef: null, findingId: null, nodeId: "0" }}><ExecutionGraph graph={report.graph_view} graphKey="k" sourceView={sourceView} summary={summary} /></SelectionProvider></LocaleProvider>);
    // A line in the safe view shows its text; a line outside it is named by its backend line number only.
    const lineRef = report.source_view.line_refs.find((line) => line.source_ref === refs[1])!;
    expect(lineRef).toBeDefined();
    const html = inspect({ ...report.source_view, plain_text_lines: [{ line_no: 885, text: "  - OperatorTotalTime: 4.23s ", source_ref: refs[0] }] });
    expect(html).toContain(`title="${refs[0]}" type="button">第 885 行 · - OperatorTotalTime: 4.23s</button>`);
    expect(html).toContain(`title="${refs[1]}" type="button">第 ${lineRef.line_no} 行</button>`);
    // Without a backend line, a reference is named by position, never by its text.
    expect(inspect({ ...report.source_view, plain_text_lines: [], line_refs: [] })).toContain(`title="${refs[1]}" type="button">来源引用 2</button>`);
    expect(html).not.toMatch(/type="button">text:\d+</);
    expect(html).toContain("汇总限制: 此计划节点中有算子的角色未知，未汇总其耗时");
    expect(html).not.toContain("operator_role_unknown<");
  });
});

describe("topology restyle", () => {
  const zh = (_en: string, zhText: string) => zhText;
  it("colours a node by its DTO plan node type only", () => {
    expect(["OLAP_SCAN", "EXCHANGE", "MERGE_EXCHANGE", "HASH_JOIN", "NESTLOOP_JOIN", "AGGREGATION", "PROJECT", null].map(operatorCategory))
      .toEqual(["scan", "exchange", "exchange", "join", "join", "agg", "other", "other"]);
  });
  it("labels an edge with the upstream backend rows and the join input side, and invents nothing", () => {
    const upstream = report.execution_summary.nodes[0];
    const rows = upstream.output_rows.display_text;
    const flow = { id: "e", source: upstream.node_id, target: "x", derived: false, partial: false, role: "probe" as const };
    expect(flowEdgeLabel(flow, upstream, zh)).toBe(rows ? `探测 · ${rows} 行` : "探测");
    expect(flowEdgeLabel({ ...flow, role: null }, null, zh)).toBeUndefined();
  });
  it("rounds bends without moving the route ends and anchors the label on the longest segment", () => {
    const route = [{ x: 0, y: 0 }, { x: 0, y: 100 }, { x: 80, y: 100 }];
    const path = roundedPath(route);
    expect(path.startsWith("M 0 0")).toBe(true);
    expect(path.endsWith("L 80 100")).toBe(true);
    expect(path).toContain("Q 0 100");
    expect(labelAnchor(route)).toEqual({ x: 0, y: 50 });
    expect(roundedPath([{ x: 1, y: 2 }, { x: 1, y: 9 }])).toBe("M 1 2 L 1 9");
  });
});
