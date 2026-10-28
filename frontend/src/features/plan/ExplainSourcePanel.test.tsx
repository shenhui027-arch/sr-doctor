import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { ExplainSnapshot } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider } from "../../i18n/locale";
import { SelectionProvider } from "../selection/selection-store";
import { ExplainSourcePanel, explainSourceFocus, explainSourceRefs } from "./ExplainSourcePanel";

function snapshot(): ExplainSnapshot {
  return {
    ...sr3514ConnectedContractGolden.explain_response.snapshot,
    source_view: {
      ...sr3514ConnectedContractGolden.explain_response.snapshot.source_view,
      lines: [{ line_no: 1, text: "PLAN FRAGMENT 0", source_ref: "explain:line:1" }],
    },
    source_locators: [
      { kind: "result", source_ref: "explain:result", row_count: 1, byte_length: 18 },
      { kind: "row_line", source_ref: "explain:line:1", row_no: 1, row_line_no: 1, global_line_no: 1, byte_start: 0, byte_end: 17 },
    ],
    source_refs: ["explain:result", "explain:line:1"],
  };
}

describe("ExplainSourcePanel", () => {
  it("renders only typed safe source lines and locators, not a Profile source parser", () => {
    const value = snapshot();
    expect(explainSourceRefs(value)).toEqual(["explain:result", "explain:line:1"]);
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><ExplainSourcePanel onClose={() => undefined} snapshot={value} /></SelectionProvider></LocaleProvider>);
    expect(html).toContain('data-source-kind="explain_costs"');
    expect(html).toContain("Only backend-provided safe structural source is shown");
    expect(html).toContain("PLAN FRAGMENT 0");
    expect(html).toContain("All source references");
    expect(html).toContain("Fact source");
    expect(html).toContain("restricted_attribute_text");
    expect(html).toContain("No plan node is selected.");
    expect(html).toContain("3.5.14");
    expect(html).toContain("Version source");
    expect(html).toContain("Row 1 · line 1");
    expect(html).not.toContain("json_profile");
    expect(html).not.toContain("text_profile");
    expect(explainSourceFocus(value, "missing").node).toBeNull();

    const focusedHtml = renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider initialSelection={{ sourceRef: "explain:line:1", subjectRef: null, findingId: null, nodeId: null }}><ExplainSourcePanel onClose={() => undefined} snapshot={value} /></SelectionProvider></LocaleProvider>);
    expect(focusedHtml).toContain("Fact source");
    expect(focusedHtml).toContain("Row 1 · line 1");
    expect(focusedHtml).toContain("restricted_attribute_text");
  });

  it("narrows selected-node source to its DTO references, adjacent edge evidence, and version evidence", () => {
    const base = snapshot();
    const value: ExplainSnapshot = {
      ...base,
      graph_view: {
        ...base.graph_view,
        state: "partial",
        root_node_ids: ["plan:1"],
        nodes: [
          {
            node_id: "plan:1", entity_kind: "plan_node", plan_node_id: "1", fragment_id: "fragment-0", pipeline_id: null,
            plan_node_type: "PROJECT", display_name: "PROJECT", source_ordinal: 1, native_operator_refs: [], subordinate_operator_refs: [],
            metric_observation_refs: [], execution_state: null, source_quality: "exact", source_refs: ["explain:line:1"],
          },
          {
            node_id: "plan:2", entity_kind: "plan_node", plan_node_id: "2", fragment_id: "fragment-0", pipeline_id: null,
            plan_node_type: "SCAN", display_name: "SCAN", source_ordinal: 2, native_operator_refs: [], subordinate_operator_refs: [],
            metric_observation_refs: [], execution_state: null, source_quality: "exact", source_refs: ["explain:line:2"],
          },
        ],
        edges: [{
          edge_id: "child:1:2", from: "plan:1", to: "plan:2", relation: "plan_child", direction_semantics: "parent_to_child",
          source: "explicit", quality: "exact", source_ordinal: 3, input_role: "child", derivation_recipe: null, restates_edge_id: null, evidence_refs: ["explain:line:3"],
        }],
        unconnected_node_ids: [], unresolved_relations: [], source_refs: ["explain:line:1", "explain:line:2", "explain:line:3"],
        completeness: { state: "partial", present_count: 2, expected_count: null, reasons: ["fixture_partial"] },
      },
      source_view: {
        ...base.source_view,
        lines: [
          { line_no: 1, text: "1:PROJECT", source_ref: "explain:line:1" },
          { line_no: 2, text: "2:SCAN", source_ref: "explain:line:2" },
          { line_no: 3, text: "|----2:SCAN", source_ref: "explain:line:3" },
        ],
      },
    };

    const focus = explainSourceFocus(value, "plan:1");
    expect(focus.node?.plan_node_id).toBe("1");
    expect(focus.adjacentEdges.map((edge) => edge.edge_id)).toEqual(["child:1:2"]);
    expect(focus.sourceRefs).toEqual([
      "explain:line:1",
      "explain:line:3",
      ...base.version.source_refs,
    ]);
    expect(focus.lines.map((line) => line.source_ref)).toEqual(["explain:line:1", "explain:line:3"]);
    expect(focus.lines.map((line) => line.text)).not.toContain("2:SCAN");
  });
});
