import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { GraphNode } from "../../../../contracts/generated/doctor-contracts";
import { sr3514ExecutionGraph } from "../../../../contracts/generated/execution-graph-fixture";
import { LocaleProvider } from "../../i18n/locale";
import { SelectionProvider } from "../selection/selection-store";
import { NodeSqlLocation } from "./NodeSqlLocation";
import { ExecutionGraph } from "./ExecutionGraph";

const base = sr3514ExecutionGraph.graph_view.nodes[0];
const located: GraphNode = { ...base, sql_location: { state: "located", kind: "predicate", before: "SELECT '中文😀'\r\nWHERE ", highlight: "金额 > 2 AND tag = '<script>alert(1)</script>'", after: "\r\nLIMIT 5", source_refs: [] } };
const render = (node: GraphNode) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><NodeSqlLocation node={node} /></LocaleProvider>);

describe("bound SQL excerpt rendering", () => {
  it("renders exactly one backend highlight preserving Unicode and CRLF, escaping HTML", () => {
    const html = render(located);
    expect(html).toContain("中文😀&#x27;\r\nWHERE ");
    expect(html).toContain("<mark>金额 &gt; 2 AND tag = &#x27;&lt;script&gt;alert(1)&lt;/script&gt;&#x27;</mark>\r\nLIMIT 5");
    expect(html.match(/<mark>/g)).toHaveLength(1);
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("尚未定位");
  });
  it.each(["unsupported", "unavailable", "ambiguous"] as const)("renders %s without a highlight", state => {
    const html = render({ ...base, sql_location: { state } });
    expect(html).not.toContain("<mark>");
    expect(html).toContain(state === "ambiguous" ? "存在多个匹配，尚未定位" : "尚未定位");
  });
  it("renders only the current report DTO, without retaining the previous excerpt or accepting a draft", () => {
    render(located);
    const next = { ...base, sqlDraft: "SHOULD_NOT_RENDER", sql: "SHOULD_NOT_RENDER" };
    const html = render(next);
    expect(html).not.toContain("金额");
    expect(html).not.toContain("SHOULD_NOT_RENDER");
    expect(html).not.toContain("<mark>");
    expect(html).toContain("尚未定位");
  });
  it("appears once in the execution graph node inspector", () => {
    const graph = { ...sr3514ExecutionGraph.graph_view, nodes: sr3514ExecutionGraph.graph_view.nodes.map((node) => node.node_id === located.node_id ? located : node) };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider initialSelection={{ sourceRef: null, subjectRef: null, findingId: null, nodeId: located.node_id }}><ExecutionGraph graph={graph} graphKey="sql" summary={sr3514ExecutionGraph.execution_summary} /></SelectionProvider></LocaleProvider>);
    expect(html.match(/<mark>/g)).toHaveLength(1);
  });
});
