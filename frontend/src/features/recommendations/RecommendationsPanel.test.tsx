import rulesetText from "../../../../backend/crates/rule-sets/doctor_v0_1/ruleset.json?raw";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { localizedFindingText } from "../../i18n/finding-text";
import { LocaleProvider } from "../../i18n/locale";
import { evidenceRowsForReferences, RecommendationsPanel, recommendedFindings } from "./RecommendationsPanel";
import { recommendationFixture } from "./recommendation-fixture";

// The scan rule's ruleset-0.3.0 recommendation, as the backend emits it.
const scanRecommendation = recommendationFixture({
  risk: "scan time share needs manual investigation; it does not prove pruning or I/O root cause",
  expected_direction: "reduce selected scan time share after one controlled manual change",
  validation_steps: ["inspect predicates, partition pruning, index, rows, bytes and I/O context", "re-profile after one controlled manual change"],
  recommendation_id: "rec.scan.reduce_read@v1",
  kind: "optimization_target",
  title: "Reduce what the scan reads",
  next_step: "Check partition pruning and predicate pushdown on this table before changing the query",
  risk_level: "low",
  steps: [
    { title: "Confirm filters reach the partition and bucket columns", detail: "Check that WHERE constrains the partition column directly, without wrapping it in a function or cast", evidence_refs: ["metric:operator-0:OperatorTotalTime"] },
    { title: "Compare rows read with rows the query keeps", detail: "A scan that outputs far more rows than its consumer keeps points at a filter applied after the scan", evidence_refs: [] },
  ],
  expected_changes: [{ metric_id: "ScanTime", direction: "decrease" }, { metric_id: "OperatorTotalTime", direction: "decrease" }],
  limitations: ["partition definitions and table statistics are not read; pruning conclusions rest on the Profile counters"],
});

const report: AnalysisSnapshotReport = {
  ...sr3514JsonMinimal,
  findings: [{
    finding_id: "finding:scan-time",
    rule_id: "SR3514.SCAN_TIME_SHARE",
    rule_version: "0.1.0",
    category: "operator_time_distribution",
    severity: "high",
    confidence: "high",
    confidence_reasons: ["exact_typed_inputs"],
    evidence_completeness: "complete",
    evidence_completeness_reasons: ["exact_typed_inputs"],
    subject_ref: "operator-0",
    evidence: ["metric:operator-0:OperatorTotalTime"],
    limitations: ["operator active time is not query wall time", "parallel and asynchronous operators may overlap"],
    recommendation: scanRecommendation,
    related_finding_refs: [],
    rule_trace_ref: "rule-trace:scan-time",
    measurements: [],
  }],
  rule_evaluations: [{
    rule_id: "SR3514.SCAN_TIME_SHARE",
    rule_version: "0.1.0",
    status: "triggered",
    evidence_completeness: "complete",
    evidence_completeness_reasons: ["exact_typed_inputs"],
    subject_ref: "operator-0",
    applicability_checks: ["version_profile:final_supported_3514"],
    input_observation_refs: ["metric:operator-0:OperatorTotalTime"],
    comparisons: ["share exceeds threshold"],
    threshold_receipts: ["threshold:0.5"],
    missing_prerequisites: [],
    evidence_refs: ["json:/operator_total_time_ns"],
    limitations: [],
    finding_id: "finding:scan-time",
  }],
};

/** A report whose single finding resolves to exactly one located, exact graph node. */
function locatedReport(): AnalysisSnapshotReport {
  const node = { ...report.graph_view.nodes[0], node_id: "located-node", plan_node_id: "4", display_name: "OLAP_SCAN", source_quality: "exact" as const, native_operator_refs: [{ operator_ref: "unique-physical", source_ordinal: 0, source_role: "native" as const, source_refs: [] }], subordinate_operator_refs: [], sql_location: { state: "located" as const, kind: "table" as const, before: "FROM ", highlight: "sales", after: "", source_refs: [] } };
  return { ...report, operator_rows: [], graph_view: { ...report.graph_view, nodes: [node] }, findings: [{ ...report.findings[0], subject_ref: "unique-physical" }] };
}

describe("RecommendationsPanel", () => {
  it("renders the page header with source pills counted from the DTO", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel report={report} />);
    expect(html).toContain("<h1 id=\"recommendations-title\">Optimization recommendations</h1>");
    expect(html).toContain("investigate first, then verify with before/after comparison");
    expect(html).toMatch(/aria-pressed="true"[^>]*>All <span class="recommendation-pill-count">1<\/span>/);
    expect(html).toContain("Product rules <span class=\"recommendation-pill-count\">1</span>");
    // An empty source can still be chosen; it shows its own empty state.
    expect(html).toMatch(/aria-pressed="false" type="button">Official Advisor · experimental <span class="recommendation-pill-count">0<\/span>/);
  });

  it("states an unlocated subject as a lead and asks to resolve the mapping before changing SQL", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel report={report} />);
    expect(html).toContain("Operator not located");
    expect(html).toContain("Lead to investigate");
    expect(html).not.toContain("Actionable investigation");
    expect(html).toContain('title="A lead, not a proven bottleneck."');
    expect(html).toContain("Resolve the operator mapping before changing SQL");
    expect(html).toContain("Direction only; no magnitude is promised.");
  });

  it("marks a located subject as an actionable investigation with a working locate action", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel onLocateNode={() => {}} report={locatedReport()} />);
    expect(html).toContain("Actionable investigation");
    expect(html).not.toContain("Lead to investigate");
    expect(html).toMatch(/<button class="button secondary" type="button">Locate operator<\/button>/);
    expect(html).not.toContain("Resolve the operator mapping");
    // The SQL position is located here, so the state chip carries no caveat.
    expect(html).not.toContain("subquery location has not been established");
    expect(html).toContain("<code>OLAP_SCAN · Plan 4</code>");
    expect(html).toContain("Optimization target · Product rule");
  });

  it("never prints two expected changes under the same short label", () => {
    // On a scan, ScanTime reads "Scan time"; the operator's own time must not read the same.
    const html = renderToStaticMarkup(<RecommendationsPanel report={locatedReport()} />);
    expect(html).toContain("Expected ↓ Scan time · ↓ Scan operator time");
    expect(html).toContain("In Compare, review ScanTime and Scan operator time");
  });

  it("says a located node's SQL position is unproven when the backend could not place it", () => {
    const located = locatedReport();
    const unplaced = { ...located, graph_view: { ...located.graph_view, nodes: located.graph_view.nodes.map((node) => ({ ...node, sql_location: { state: "unavailable" as const } })) } };
    const html = renderToStaticMarkup(<RecommendationsPanel report={unplaced} />);
    expect(html).toContain("Actionable investigation");
    expect(html).toContain("The exact SQL or subquery location has not been established");
  });

  it("disables operator location when the subject is not located", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel onLocateNode={() => {}} report={report} />);
    expect(html).toMatch(/<button class="button secondary" disabled="" title="Operator not located" type="button">Locate operator<\/button>/);
  });

  it("selects the first card and renders the structured recommendation from the DTO", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel onOpenCompare={() => {}} report={report} />);
    expect(html).toContain('aria-pressed="true" class="recommendation-card selected"');
    expect(html).toContain('<strong class="recommendation-card-title">Reduce what the scan reads</strong>');
    expect(html).toContain("<code title=\"SR3514.SCAN_TIME_SHARE@0.1.0\">rec.scan.reduce_read@v1</code>");
    expect(html).toContain("Linked finding · High");
    expect(html).toContain("Check partition pruning and predicate pushdown on this table before changing the query.");
    expect(html).toContain("Start before/after comparison");
    expect(html).toContain("Copy investigation steps");
    // Backend steps, numbered after the mapping step this unlocated subject needs.
    expect(html).toContain("Confirm filters reach the partition and bucket columns");
    expect(html).toMatch(/<span aria-hidden="true" class="recommendation-step-number">3<\/span>/);
    // Step evidence and evidence cards are exact cells only.
    expect(html).toContain('class="recommendation-step-evidence" title="metric:operator-0:OperatorTotalTime">OperatorTotalTime = 500000ns</code>');
    expect(html).toContain('<span class="recommendation-evidence-label" title="Operator active time">OperatorTotalTime</span>');
    // Expected change per metric, then the result effect.
    expect(html).toContain("<span>ScanTime</span><span class=\"recommendation-direction\">↓ Down</span>");
    expect(html).toContain("Query result</span><span class=\"recommendation-direction unchanged\">Unchanged</span>");
    expect(html).toContain("Expected ↓ Scan time · ↓ Operator time");
    expect(html).toContain("<strong>Low</strong> · Scan time share needs manual investigation; it does not prove a pruning or I/O root cause.");
    expect(html).toContain("In Compare, review ScanTime and Operator time");
    // The rail's limits are the recommendation's own, not the finding's measurement caveats.
    expect(html).toContain("Partition definitions and table statistics are not read");
    expect(html).not.toContain("Operator active time is not query wall time");
    expect(html).not.toContain("Apply recommendation");
  });

  it("lists every recommended finding as a card and selects only the first by default", () => {
    const second = { ...report.findings[0], finding_id: "finding:join-time", rule_id: "SR3514.JOIN_TIME_SHARE", severity: "medium", subject_ref: "query", recommendation: { ...scanRecommendation, recommendation_id: "rec.join.reduce_input@v1" } };
    const html = renderToStaticMarkup(<RecommendationsPanel report={{ ...report, findings: [report.findings[0], second] }} />);
    expect(html.match(/class="recommendation-card( selected)?"/g)).toEqual(['class="recommendation-card selected"', 'class="recommendation-card"']);
    expect(html.match(/aria-pressed="true" class="recommendation-card/g)).toHaveLength(1);
    expect(html).toContain('<span class="recommendation-severity warn severity-medium">');
    expect(html).toContain("<span>Whole query</span>");
    expect(html).toContain("rec.scan.reduce_read@v1</code>");
    expect(html).not.toContain("rec.join.reduce_input@v1");
  });

  it("does not turn a suspected (low-confidence) finding into a recommendation", () => {
    const suspected = { ...report.findings[0], finding_id: "finding:suspected", confidence: "low" as const };
    const withSuspected = { ...report, findings: [report.findings[0], suspected] };
    expect(recommendedFindings(withSuspected).map((finding) => finding.finding_id)).toEqual(["finding:scan-time"]);
    const html = renderToStaticMarkup(<RecommendationsPanel report={withSuspected} />);
    expect(html.match(/class="recommendation-card( selected)?"/g)).toHaveLength(1);
    expect(html).toContain("1 low-confidence finding(s) not turned into a recommendation");
    const none = renderToStaticMarkup(<RecommendationsPanel report={{ ...report, findings: [suspected] }} />);
    expect(none).toContain("No recommendations for this Profile");
    expect(none).toContain("1 low-confidence finding(s) not turned into a recommendation");
  });

  it("omits the compare action when no compare handler is wired", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel report={report} />);
    expect(html).not.toContain("Start before/after comparison");
  });

  it("joins evidence only by an exact unique metric-cell reference and counts unresolved refs", () => {
    const rows = evidenceRowsForReferences(report, [
      "metric:operator-0:OperatorTotalTime",
      "metric:operator-0:OperatorTotalTime",
      "observation:operator-0:derived:OperatorTotalTime",
    ]);

    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ reference: "metric:operator-0:OperatorTotalTime", subject: "" });
    expect(rows[0].cell?.display_text).toBe("500000ns");
    expect(rows[1]).toEqual({ reference: "observation:operator-0:derived:OperatorTotalTime", cell: null, subject: "" });

    const unresolved = { ...report, rule_evaluations: [{ ...report.rule_evaluations[0], input_observation_refs: ["metric:operator-0:OperatorTotalTime", "observation:operator-0:derived:OperatorTotalTime"] }] };
    const html = renderToStaticMarkup(<RecommendationsPanel report={unresolved} />);
    expect(html).toContain(">OperatorTotalTime</span>");
    expect(html).not.toContain("observation:operator-0:derived:OperatorTotalTime");
    expect(html).toContain("1 more rule input(s) in Doctor");
  });

  it("orders evidence cards by expected changes, then step checks, and caps them at three", () => {
    const cells = ["A", "B", "C", "D"].map((metric) => ({ ...report.operator_rows[0].metric_cells[0], metric_id: metric, metric_cell_ref: `metric:operator-0:${metric}` }));
    const wide: AnalysisSnapshotReport = {
      ...report,
      operator_rows: [{ ...report.operator_rows[0], metric_cells: cells }],
      findings: [{ ...report.findings[0], recommendation: { ...scanRecommendation, steps: [{ title: "s", detail: "d", evidence_refs: ["metric:operator-0:D"] }], expected_changes: [{ metric_id: "C", direction: "decrease" }] } }],
      rule_evaluations: [{ ...report.rule_evaluations[0], input_observation_refs: cells.slice(0, 3).map((cell) => cell.metric_cell_ref) }],
    };
    const html = renderToStaticMarkup(<RecommendationsPanel report={wide} />);
    const labels = [...html.matchAll(/class="recommendation-evidence-label"[^>]*>([A-D])</g)].map((match) => match[1]);
    expect(labels).toEqual(["C", "D", "A"]);
    expect(html).toContain("1 more rule input(s) in Doctor");
  });

  it("names an operator once across step chips and keeps only the plan id afterwards", () => {
    const located = locatedReport();
    const cell = { ...report.operator_rows[0].metric_cells[0], subject_ref: "unique-physical", metric_cell_ref: "metric:unique-physical:OperatorTotalTime", source_refs: [] };
    const second = { ...cell, metric_id: "ScanTime", metric_cell_ref: "metric:unique-physical:ScanTime" };
    const twoSteps: AnalysisSnapshotReport = {
      ...located,
      operator_rows: [{ ...report.operator_rows[0], operator_ref: "unique-physical", plan_node_id: "4", metric_cells: [cell, second] }],
      findings: [{ ...located.findings[0], recommendation: { ...scanRecommendation, steps: [
        { title: "one", detail: "d", evidence_refs: [cell.metric_cell_ref] },
        { title: "two", detail: "d", evidence_refs: [second.metric_cell_ref] },
      ] } }],
    };
    const html = renderToStaticMarkup(<RecommendationsPanel report={twoSteps} />);
    const chips = [...html.matchAll(/class="recommendation-step-evidence"[^>]*>([^<]*)</g)].map((match) => match[1]);
    expect(chips).toEqual(["SCAN Plan 4 · OperatorTotalTime = 500000ns", "Plan 4 · ScanTime = 500000ns"]);
  });

  it("folds the official Advisor and renders its provenance, guidance and validation from the DTO", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel advisorResponse={sr3514ConnectedContractGolden.advisor_response} report={report} />);

    expect(html).toContain('<details class="advisor-guide-panel"');
    expect(html).toContain("<summary>Official Advisor · experimental · FE-local · volatile</summary>");
    expect(html).toContain("official_plan_advisor");
    expect(html).toContain('class="official">Official</span>');
    expect(html).toContain('class="experimental">Experimental</span>');
    expect(html).toContain('class="fe-local">FE-local</span>');
    expect(html).toContain('class="volatile">Volatile</span>');
    expect(html).toContain("Review the Advisor guide before a controlled user-side rerun.");
    expect(html).toContain("Run EXPLAIN COSTS.");
    expect(html).toContain("Advisor is official, experimental, FE-local, and volatile");
    expect(html).toContain("Official Advisor · experimental <span class=\"recommendation-pill-count\">1</span>");
  });

  it("hides the Advisor block when there is no Advisor response", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel report={report} />);
    expect(html).not.toContain("advisor-guide-panel");
    expect(html).not.toContain("Official Advisor · experimental · FE-local · volatile");
  });

  it("shows one concise state-consistent card when a loaded Profile has no recommendations", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel onImportProfile={() => undefined} onOpenQueries={() => undefined} report={{ ...report, findings: [], rule_evaluations: [] }} />);
    expect(html).toContain("No recommendations for this Profile");
    expect(html).toContain("No product rule found an evidence-backed issue");
    expect(html).not.toMatch(/import a Profile/i);
    expect(html).not.toContain("Import profile");
    expect(html).not.toContain('id="recommendation-detail"');
    expect(html).not.toContain("advisor-guide-panel");
  });

  it("keeps the folded Advisor next to the empty card when the Advisor responded without guidance", () => {
    const advisor = { ...sr3514ConnectedContractGolden.advisor_response, recommendations: [] };
    const html = renderToStaticMarkup(<RecommendationsPanel advisorResponse={advisor} report={{ ...report, findings: [], rule_evaluations: [] }} />);
    expect(html).toContain("No recommendations for this Profile");
    expect(html).toContain("Advisor responded but supplied no guidance");
  });

  it("localizes the structured recommendation without changing exact evidence identifiers", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><RecommendationsPanel advisorResponse={sr3514ConnectedContractGolden.advisor_response} report={report} /></LocaleProvider>);
    expect(html).toContain(">优化建议</h1>");
    expect(html).toContain("全部来源 <span class=\"recommendation-pill-count\">2</span>");
    expect(html).toContain("减少扫描读取量");
    expect(html).toContain("确认过滤条件作用在分区列和分桶列上");
    expect(html).toContain("待排查线索");
    expect(html).toContain("具体排查");
    expect(html).toContain("复制排查步骤");
    expect(html).toContain("方向判断，不承诺幅度。");
    expect(html).toContain("未读取分区定义与表统计信息");
    expect(html).toContain("<summary>官方 Advisor · 实验性 · FE 本地 · 易变</summary>");
    expect(html).toContain('class="official">官方</span>');
    expect(html).toContain("在受控的用户侧重跑前检查 Advisor 指引。");
    expect(html).toContain("OperatorTotalTime");
  });

  it("shows an explicit guided empty state before analysis", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel report={null} />);
    expect(html).toContain("No analysis loaded");
    expect(html).toContain("nothing is auto-applied");
    expect(html).not.toContain('id="recommendation-list"');
    const guided = renderToStaticMarkup(<RecommendationsPanel onImportProfile={() => undefined} onOpenQueries={() => undefined} report={null} />);
    expect(guided).toContain("Pick a query");
    expect(guided).toContain("Import profile");
  });

  it("carries the rule identity and evaluation completeness reasons on the linked-finding chip", () => {
    const partial: AnalysisSnapshotReport = {
      ...report,
      rule_evaluations: [{
        ...report.rule_evaluations[0],
        evidence_completeness: "partial",
        evidence_completeness_reasons: ["bounded_typed_inputs"],
      }],
    };
    const html = renderToStaticMarkup(<RecommendationsPanel report={partial} />);
    expect(html).toContain('title="SR3514.SCAN_TIME_SHARE@0.1.0 · Confidence High · Evidence completeness Partial (Bounded typed inputs)"');
  });

  it("does not use opaque operator IDs as default copy", () => {
    const html = renderToStaticMarkup(<RecommendationsPanel report={report} />);
    expect(html).not.toContain(">operator-0<");
    expect(html).not.toContain("for operator-0");
  });
});

describe("RecommendationsPanel finding measurements (v0.6-04)", () => {
  it("states the finding's backend measurements in the summary, never computed here", () => {
    const measured = { ...report, findings: [{ ...report.findings[0], measurements: [{ measurement_id: "broadcast_bytes_sent", display_text: "576.6 MiB", threshold_text: "> 100 MiB", operator_type: "EXCHANGE_SINK", input_refs: [] }] }] };
    const html = renderToStaticMarkup(<RecommendationsPanel report={measured} />);
    expect(html).toContain("Broadcast bytes sent 576.6 MiB. Check partition pruning and predicate pushdown on this table before changing the query.");
  });
});

describe("RecommendationsPanel evidence subjects (v0.6-09)", () => {
  it("names the physical operator a counter was printed under, joined only by the backend's exact source line", async () => {
    const { sr3514ExecutionGraph } = await import("../../../../contracts/generated/execution-graph-fixture");
    const report = sr3514ExecutionGraph;
    const finding = report.findings[0];
    const evaluation = report.rule_evaluations.find((candidate) => candidate.finding_id === finding.finding_id);
    const rows = evidenceRowsForReferences(report, evaluation?.input_observation_refs.length ? evaluation.input_observation_refs : finding.evidence);
    const named = rows.filter((row) => row.physicalOperator);
    expect(named.length).toBeGreaterThan(0);
    for (const row of named) {
      const nodeId = row.location?.node?.node_id;
      const summary = report.execution_summary.state === "available" ? report.execution_summary.nodes.find((node) => node.node_id === nodeId) : undefined;
      const matches = summary?.operators.filter((operator) => [operator.operator_time, operator.push_rows, operator.pull_rows].some((value) => value.source_refs.includes(row.cell!.source_refs[0]))) ?? [];
      expect(matches).toHaveLength(1);
      expect(row.physicalOperator).toBe(`${matches[0].operator_type} #${row.location!.node!.plan_node_id}`);
    }
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><RecommendationsPanel report={report} /></LocaleProvider>);
    expect(html).toContain(`title="${named[0].physicalOperator} · `);
  });
});

describe("Ruleset 0.3.0 recommendation copy", () => {
  it("translates every structured recommendation string the backend can emit", () => {
    const ruleset = JSON.parse(rulesetText) as {
      rules: { rule_id: string; recommendation_template: { title: string; next_step: string; limitations: string[]; steps: { title: string; detail: string }[] } }[];
    };
    const zh = (english: string, chinese: string) => chinese;
    const untranslated: string[] = [];
    for (const rule of ruleset.rules) {
      const template = rule.recommendation_template;
      const check = (field: Parameters<typeof localizedFindingText>[1], value: string) => {
        if (localizedFindingText(rule.rule_id, field, value, zh) === value) untranslated.push(`${rule.rule_id} ${field}: ${value}`);
      };
      check("recommendation_title", template.title);
      check("next_step", template.next_step);
      template.limitations.forEach((value) => check("recommendation_limitation", value));
      template.steps.forEach((step) => { check("step_title", step.title); check("step_detail", step.detail); });
    }
    expect(untranslated).toEqual([]);
  });
});
