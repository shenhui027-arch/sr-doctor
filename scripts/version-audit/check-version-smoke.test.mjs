import test from "node:test";
import assert from "node:assert/strict";
import { parseArgs, summarizeReport, verdict } from "./check-version-smoke.mjs";

const report = (overrides = {}) => ({
  version: { effective_version: "3.4.10", pack_match: "baseline_with_deltas", support_status: "supported", compatibility: "compatible", issues: [], selected_pack: { release_deltas: { delta_ids: ["plan_advisor_unavailable"] } } },
  profile_context: { limitations: [] },
  completeness: { metrics: { state: "partial" } },
  graph_view: { state: "complete", nodes: [{}, {}], edges: [{}] },
  execution_summary: { state: "available", nodes: [{ time_share_display: "60.00%" }, { time_share_display: null }] },
  findings: [{ rule_id: "SR3514.JOIN_TIME_SHARE", severity: "high" }],
  diagnostics: [{ code: "metric_unregistered" }, { code: "metric_unregistered" }],
  ...overrides,
});

test("a resolved release with a graph and time share passes", () => {
  const summary = summarizeReport(report());
  assert.equal(summary.pack_match, "baseline_with_deltas");
  assert.equal(summary.time_shared_nodes, 1);
  assert.deepEqual(summary.findings, ["SR3514.JOIN_TIME_SHARE:high"]);
  assert.deepEqual(summary.diagnostics, { metric_unregistered: 2 });
  assert.deepEqual(verdict(summary), []);
});

test("no pack, no graph, no time share or a parse failure fail the capture", () => {
  const summary = summarizeReport(report({
    version: { effective_version: "4.2.0", pack_match: "no_pack", support_status: "unsupported", compatibility: "incompatible", issues: [], selected_pack: null },
    graph_view: { state: "unavailable", nodes: [], edges: [] },
    execution_summary: { state: "unavailable", nodes: [] },
    diagnostics: [{ code: "invalid_number" }],
  }));
  assert.deepEqual(verdict(summary), ["pack_match=no_pack", "support_status=unsupported", "graph_unavailable", "execution_summary=unavailable", "parse:invalid_numberx1"]);
});

test("the API address is required", () => {
  assert.throws(() => parseArgs([]), /usage/);
  assert.equal(parseArgs(["--api", "http://127.0.0.1:1"]).root, "fixtures/runtime/version-smoke");
});

test("the official analysis is read per plan node, skipping fragment sinks and colour codes", async () => {
  const { parseAnalyze, compareWithOfficial } = await import("./check-version-smoke.mjs");
  const esc = String.fromCharCode(27);
  const text = [
    `${esc}[0mFragment 0${esc}[0m`,
    "└──MERGE_EXCHANGE (id=9) [GATHER]",
    "       TotalTime: 933.188us (2.46%) [CPUTime: 263.314us]",
    "       OutputRows: 4",
    "└──DATA_STREAM_SINK (id=9)",
    "   │   PartitionType: UNPARTITIONED",
    "            ├──<PROBE> OLAP_SCAN (id=0)",
    `            │      ${esc}[1mTotalTime: 20.472ms (54.05%)${esc}[0m`,
    "            │      OutputRows: 327.680K (327680)",
  ].join("\n");
  const nodes = parseAnalyze(text);
  assert.deepEqual([...nodes.keys()], ["9", "0"]);
  assert.deepEqual(nodes.get("0"), { type: "OLAP_SCAN", share: 54.05, rows: "327680" });
  const report = { execution_summary: { nodes: [
    { node_id: "9", plan_node_id: "9", time_share_display: "2.46%", output_rows: { canonical_number: { value: "4" } } },
    { node_id: "0", plan_node_id: "0", time_share_display: "54.07%", output_rows: { canonical_number: { value: "327680" } } },
  ] } };
  assert.deepEqual(compareWithOfficial(report, nodes), { compared: 2, mismatches: ["share:0:54.07!=54.05"] });
});
