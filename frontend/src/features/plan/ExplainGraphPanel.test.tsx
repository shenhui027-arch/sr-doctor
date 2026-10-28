import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { ExplainSnapshot } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider } from "../../i18n/locale";
import { ExplainGraphPanel } from "./ExplainGraphPanel";

function graphSnapshot(): ExplainSnapshot {
  const base = sr3514ConnectedContractGolden.explain_response.snapshot;
  return {
    ...base,
    graph_view: {
      ...base.graph_view,
      state: "complete",
      root_node_ids: ["plan:1"],
      nodes: [{
        node_id: "plan:1", entity_kind: "plan_node", plan_node_id: "1", fragment_id: "fragment-0", pipeline_id: null, plan_node_type: "PROJECT", display_name: "PROJECT", source_ordinal: 0,
        native_operator_refs: [], subordinate_operator_refs: [], metric_observation_refs: [], execution_state: null, source_quality: "exact", source_refs: ["explain:line:1"],
      }],
      edges: [], unconnected_node_ids: [], unresolved_relations: [], source_refs: ["explain:line:1"],
      completeness: { state: "complete", present_count: 1, expected_count: 1, reasons: [] },
    },
    source_view: { ...base.source_view, lines: [{ line_no: 1, text: "1:PROJECT", source_ref: "explain:line:1" }] },
  };
}

describe("ExplainGraphPanel", () => {
  it("renders one plan graph with no runtime metrics and no parallel projections", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><ExplainGraphPanel onClose={() => undefined} snapshot={graphSnapshot()} /></LocaleProvider>);
    expect(html).toContain("只读计划");
    expect(html).toContain("仅为估算，未执行任何查询。");
    expect(html).toContain("执行图");
    expect(html).not.toContain(">数据流<");
    expect(html).not.toContain("专家计划");
    expect(html).not.toContain("最耗时节点");
  });

  it("shows the plan text beside the graph on request", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><ExplainGraphPanel initialSourceOpen onClose={() => undefined} snapshot={graphSnapshot()} /></LocaleProvider>);
    expect(html).toContain("with-source");
    expect(html).toContain("隐藏计划文本");
  });

  it("opens the plan text only when no graph is available", () => {
    const snapshot = { ...graphSnapshot(), graph_view: { ...graphSnapshot().graph_view, state: "unavailable" as const, nodes: [], root_node_ids: [] } };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><ExplainGraphPanel onClose={() => undefined} snapshot={snapshot} /></LocaleProvider>);
    expect(html).not.toContain("execution-graph-canvas");
    expect(html).not.toContain("隐藏计划文本");
  });
});
