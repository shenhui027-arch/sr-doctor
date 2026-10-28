import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { SelectionProvider } from "../selection/selection-store";
import { LocaleProvider } from "../../i18n/locale";
import { EvidenceGaps, FindingInspector, FindingsPanel, heldBackEvaluations } from "./FindingsPanel";
import { ProvenanceFacts } from "../doctor/DoctorDetails";
import { recommendationFixture } from "../recommendations/recommendation-fixture";

/** The Doctor findings column and the inspector, as the workbench renders them. */
function DoctorFindings({ report }: { report: AnalysisSnapshotReport }) {
  return <><FindingsPanel report={report} /><EvidenceGaps report={report} /><FindingInspector report={report} /></>;
}

describe("FindingsPanel", () => {
  it("renders severity, confidence, evidence, limitations, and the rule trace from the DTO", () => {
    const report: AnalysisSnapshotReport = {
      ...sr3514JsonMinimal,
      findings: [{
        finding_id: "finding:operator-0:time-share",
        rule_id: "SR3514.SCAN_TIME_SHARE",
        rule_version: "0.1.0",
        category: "operator_time_distribution",
        severity: "moderate",
        confidence: "high",
        confidence_reasons: ["exact_typed_inputs"],
        evidence_completeness: "complete",
        evidence_completeness_reasons: ["exact_typed_inputs"],
        subject_ref: "operator-0",
        evidence: ["metric:operator-0:OperatorTotalTime"],
        limitations: ["operator active time is not query wall time"],
        recommendation: recommendationFixture({ risk: "scan time share needs manual investigation; it does not prove pruning or I/O root cause", expected_direction: "reduce selected scan time share after one controlled manual change", validation_steps: ["inspect predicates, partition pruning, index, rows, bytes and I/O context"] }),
        related_finding_refs: [],
        rule_trace_ref: "rule-trace:operator-time-share",
        measurements: [],
      }],
      rule_evaluations: [{
        rule_id: "SR3514.SCAN_TIME_SHARE",
        rule_version: "0.1.0",
        status: "triggered",
        evidence_completeness: "complete",
        evidence_completeness_reasons: ["exact_typed_inputs"],
        subject_ref: "operator-0",
        applicability_checks: ["metric exact"],
        input_observation_refs: ["metric:operator-0:OperatorTotalTime"],
        comparisons: ["share above threshold"],
        threshold_receipts: ["threshold:0.5"],
        missing_prerequisites: [],
        evidence_refs: ["json:/operator_total_time_ns"],
        limitations: ["graph unavailable"],
        finding_id: "finding:operator-0:time-share",
      }],
    };
    const html = renderToStaticMarkup(<SelectionProvider><DoctorFindings report={report} /></SelectionProvider>);

    expect(html).toMatch(/class="severity-tag [a-z]+">Moderate</);
    expect(html).toContain("Confidence High");
    expect(html).toContain("Exact typed inputs");
    expect(html).toContain("Evidence completeness");
    expect(html).toContain("Complete");
    expect(html).toContain("Operator active time is not query wall time");
    expect(html).toContain("rule-trace:operator-time-share");
    expect(html).toContain("Operator time distribution");
    expect(html).toContain("Triggered");
    expect(html).toContain("threshold:0.5");
    expect(html).toContain("metric:operator-0:OperatorTotalTime");
    expect(html).toContain("Inspect predicates, partition pruning, index, rows, bytes and I/O context");
    // The only finding is selected by default, so the inspector is never empty while findings exist.
    expect(html).toContain('aria-pressed="true" class="finding-item selected"');
    expect(html).toContain('class="panel finding-inspector"');
    expect(html).toContain('<details class="technical-disclosure"><summary>Evidence and rule details</summary>');
    expect(html.indexOf('class="inspector-direction"')).toBeLessThan(html.indexOf('class="technical-disclosure"'));
    // operator-0 is not a row of this report: no graph location is invented.
    expect(html).not.toContain("Show in execution graph");
  });

  it("names the located plan node and its backend time share in the finding title", async () => {
    const { sr3514ExecutionGraph } = await import("../../../../contracts/generated/execution-graph-fixture");
    const row = sr3514ExecutionGraph.operator_rows.find((candidate) => candidate.plan_node_id === "0")!;
    const report: AnalysisSnapshotReport = {
      ...sr3514ExecutionGraph,
      findings: [{ finding_id: "f:scan", rule_id: "SR3514.SCAN_TIME_SHARE", rule_version: "0.1.0", category: "operator_time_distribution", severity: "high", confidence: "high", confidence_reasons: [], evidence_completeness: "complete", evidence_completeness_reasons: [], subject_ref: row.operator_ref, evidence: [], limitations: [], recommendation: recommendationFixture({ risk: "r", expected_direction: "d", validation_steps: [] }), related_finding_refs: [], rule_trace_ref: "t", measurements: [] }],
      rule_evaluations: [],
    };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider><DoctorFindings report={report} /></SelectionProvider></LocaleProvider>);
    expect(html).toContain("finding-subject");
    expect(html).toMatch(/class="finding-subject">SCAN[^<]* · Plan 0<\/span>/);
    expect(html).toContain("占算子总耗时 99.96% · 4.229 s");
    expect(html).toContain("查询总耗时 8.099 s");
    expect(html).toContain("在图中定位");
  });

  it("localizes catalogued finding display copy in Chinese while preserving technical references", () => {
    const report: AnalysisSnapshotReport = {
      ...sr3514JsonMinimal,
      findings: [{
        finding_id: "finding:operator-0:time-share",
        rule_id: "SR3514.SCAN_TIME_SHARE",
        rule_version: "0.1.0",
        category: "operator_time_distribution",
        severity: "moderate",
        confidence: "high",
        confidence_reasons: ["exact_typed_inputs"],
        evidence_completeness: "complete",
        evidence_completeness_reasons: ["exact_typed_inputs"],
        subject_ref: "operator-0",
        evidence: ["metric:operator-0:OperatorTotalTime"],
        limitations: ["operator active time is not query wall time", "parallel and asynchronous operators may overlap"],
        recommendation: recommendationFixture({ risk: "scan time share needs manual investigation; it does not prove pruning or I/O root cause", expected_direction: "reduce selected scan time share after one controlled manual change", validation_steps: ["inspect predicates, partition pruning, index, rows, bytes and I/O context", "re-profile after one controlled manual change"] }),
        related_finding_refs: [],
        rule_trace_ref: "rule-trace:operator-time-share",
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
        comparisons: [],
        threshold_receipts: [],
        missing_prerequisites: [],
        evidence_refs: ["json:/operator_total_time_ns"],
        limitations: [],
        finding_id: "finding:operator-0:time-share",
      }],
    };

    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider><DoctorFindings report={report} /></SelectionProvider></LocaleProvider>);

    expect(html).toContain("算子时间分布");
    expect(html).toContain(">中等<");
    expect(html).toContain("输入已类型化且精确");
    expect(html).toContain("算子活跃时间不等同于查询墙钟时间");
    expect(html).toContain("并行和异步算子可能重叠");
    expect(html).toContain("证据完整度");
    expect(html).toContain("扫描时间占比需要人工调查；它不能证明裁剪或 I/O 根因");
    expect(html).toContain("一次受控人工变更后降低所选扫描算子的时间占比");
    expect(html).toContain("检查谓词、分区裁剪、索引、行数、字节数与 I/O 上下文");
    expect(html).toContain("一次受控人工变更后重新采集 Profile");
    expect(html).toContain("metric:operator-0:OperatorTotalTime");
    expect(html).toContain("json:/operator_total_time_ns");
  });
});

describe("FindingsPanel suppressed share signals", () => {
  const suppressed: AnalysisSnapshotReport["rule_evaluations"][number] = {
    rule_id: "SR3514.SCAN_TIME_SHARE",
    rule_version: "0.2.0",
    status: "suppressed",
    evidence_completeness: "complete",
    evidence_completeness_reasons: ["exact_typed_inputs"],
    subject_ref: "operator-0",
    applicability_checks: [],
    input_observation_refs: [],
    comparisons: ["ratio_all_strictly_gt_30_percent", "subject_time_all_below_absolute_floor_100000000ns"],
    threshold_receipts: ["absolute_floor:subject_operator_time_ns<100000000ns:product_decision_ruleset_0.1.5"],
    missing_prerequisites: [],
    evidence_refs: [],
    limitations: [],
    finding_id: null,
  };
  const render = (evaluations: AnalysisSnapshotReport["rule_evaluations"]): string => renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><DoctorFindings report={{ ...sr3514JsonMinimal, findings: [], rule_evaluations: evaluations }} /></SelectionProvider></LocaleProvider>);

  it("lists backend-suppressed signals with the floor from the evaluation, not a client threshold", () => {
    const html = render([suppressed]);
    expect(html).toContain('Findings <span class="findings-count">0</span>');
    expect(html).toContain("No finding to inspect");
    expect(html).toContain("1 share signal(s) below the 100 ms floor, not reported");
    expect(html).toContain("Scan time share");
  });

  it("stays silent when nothing was suppressed", () => {
    const html = render([{ ...suppressed, status: "not_triggered", comparisons: ["ratio_all_not_greater_than_15_percent"] }]);
    expect(html).not.toContain("findings-suppressed");
  });
});

describe("FindingsPanel real-Profile usability (v0.6-04)", () => {
  async function skewReport(): Promise<AnalysisSnapshotReport> {
    const { sr3514ExecutionGraph } = await import("../../../../contracts/generated/execution-graph-fixture");
    const row = sr3514ExecutionGraph.operator_rows.find((candidate) => candidate.plan_node_id === "0")!;
    const [mean, max] = row.metric_cells;
    return {
      ...sr3514ExecutionGraph,
      findings: [{
        finding_id: "f:skew", rule_id: "SR3514.OPERATOR_TIME_SKEW", rule_version: "0.1.0", category: "runtime_skew", severity: "moderate",
        confidence: "medium", confidence_reasons: [], evidence_completeness: "partial", evidence_completeness_reasons: [], subject_ref: row.operator_ref,
        evidence: [], limitations: [], recommendation: recommendationFixture({ risk: "r", expected_direction: "d", validation_steps: [] }), related_finding_refs: [], rule_trace_ref: "t",
        measurements: [
          { measurement_id: "slowest_over_mean_driver_time", display_text: "10.3–10.4×", threshold_text: "> 2×", operator_type: "HASH_JOIN_PROBE", input_refs: [mean.metric_cell_ref, max.metric_cell_ref] },
          { measurement_id: "slowest_driver_time", display_text: "499.854 ms", threshold_text: "≥ 100 ms", operator_type: "HASH_JOIN_PROBE", input_refs: [max.metric_cell_ref] },
        ],
      }],
      rule_evaluations: [{
        rule_id: "SR3514.OPERATOR_TIME_SKEW", rule_version: "0.1.0", status: "triggered", evidence_completeness: "partial", evidence_completeness_reasons: [],
        subject_ref: row.operator_ref, applicability_checks: [], input_observation_refs: [mean.metric_cell_ref, max.metric_cell_ref], comparisons: [], threshold_receipts: [],
        missing_prerequisites: [], evidence_refs: [], limitations: [], finding_id: "f:skew",
      }],
    };
  }

  it("leads a why-finding with the rule's own measurement and condition, not the node time share", async () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><DoctorFindings report={await skewReport()} /></SelectionProvider></LocaleProvider>);
    const card = html.slice(html.indexOf('class="finding-item'), html.indexOf("</button>", html.indexOf('class="finding-item')));
    expect(card).toContain("SCAN · Plan 0");
    expect(card).toContain("Slowest driver vs. mean 10.3–10.4×");
    expect(card).not.toContain("Share of operator time");
    // Inspector: every measurement with its operator and the rule's condition, and a single-cell measurement names its evidence row.
    expect(html).toContain("rule: &gt; 2×");
    expect(html).toContain("Slowest driver<small> · HASH_JOIN_PROBE</small>");
    expect(html).toMatch(/inspector-metric"[^>]*>Slowest driver</);
    // The rule id is a technical detail, not the inspector headline.
    expect(html.indexOf("SR3514.OPERATOR_TIME_SKEW</code>")).toBeGreaterThan(html.indexOf('class="technical-disclosure"'));
  });

  it("does not call a Profile clean when no rule could be evaluated", () => {
    const notApplicable = { rule_id: "SR3514.SCAN_TIME_SHARE", rule_version: "0.2.0", status: "not_applicable" as const, evidence_completeness: "unavailable" as const, evidence_completeness_reasons: [], subject_ref: null, applicability_checks: [], input_observation_refs: [], comparisons: [], threshold_receipts: [], missing_prerequisites: ["execution_summary_unavailable"], evidence_refs: [], limitations: [], finding_id: null };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><DoctorFindings report={{ ...sr3514JsonMinimal, findings: [], rule_evaluations: [notApplicable] }} /></SelectionProvider></LocaleProvider>);
    expect(html).not.toContain("No reportable issues were found");
    expect(html).toContain("Rules were not evaluated for this Profile");
    expect(html).toContain("Rules not evaluated");
  });

  it("shows no topology contradiction when the graph itself is unavailable", () => {
    const report: AnalysisSnapshotReport = {
      ...sr3514JsonMinimal,
      findings: [],
      graph_view: { ...sr3514JsonMinimal.graph_view, state: "unavailable", unresolved_relations: [{ relation_ref: "topology", reason: "graph_unavailable", source_refs: [] }] },
    };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><DoctorFindings report={report} /></SelectionProvider></LocaleProvider>);
    expect(html).not.toContain("Contradiction");
  });

  it("translates completeness reason codes instead of printing them", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><ProvenanceFacts report={{ ...sr3514JsonMinimal, findings: [], completeness: { ...sr3514JsonMinimal.completeness, metrics: { state: "partial", present_count: 1, expected_count: null, reasons: ["runtime_profile_registry_backed_metrics"] }, version: { state: "partial", present_count: 1, expected_count: 1, reasons: ["profile_level_2"] } } }} /></LocaleProvider>);
    expect(html).not.toContain("runtime_profile_registry_backed_metrics");
    expect(html).toContain("Captured with pipeline_profile_level=2");
  });
});

describe("FindingsPanel groups findings on one plan node (v0.6-07)", () => {
  it("lists several findings of one node under one heading and keeps single findings as they are", async () => {
    const { sr3514ExecutionGraph } = await import("../../../../contracts/generated/execution-graph-fixture");
    const row = sr3514ExecutionGraph.operator_rows.find((candidate) => candidate.plan_node_id === "0")!;
    const finding = (id: string, ruleId: string, subject: string | null): AnalysisSnapshotReport["findings"][number] => ({
      finding_id: id, rule_id: ruleId, rule_version: "0.1.0", category: "operator_time_distribution", severity: "moderate", confidence: "medium", confidence_reasons: [],
      evidence_completeness: "partial", evidence_completeness_reasons: [], subject_ref: subject, evidence: [], limitations: [],
      recommendation: recommendationFixture({ risk: "r", expected_direction: "d", validation_steps: [] }), related_finding_refs: [], rule_trace_ref: "t", measurements: [],
    });
    const report: AnalysisSnapshotReport = {
      ...sr3514ExecutionGraph,
      findings: [
        finding("f:1", "SR3514.JOIN_PROBE_FANOUT", row.operator_ref),
        finding("f:query", "SR3514.QUERY_WIDE", null),
        finding("f:2", "SR3514.NODE_TIME_SHARE", row.operator_ref),
        finding("f:3", "SR3514.ROW_FLOW_AMPLIFICATION", row.operator_ref),
      ],
      rule_evaluations: [],
    };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><FindingsPanel report={report} /></SelectionProvider></LocaleProvider>);
    expect(html.match(/class="finding-group"/g)).toHaveLength(1);
    expect(html).toContain('aria-label="SCAN #0" class="finding-group" role="group"');
    expect(html).toContain("3 findings");
    // The group sits where its first finding is delivered; the node is named once, by the heading.
    const order = ["SR3514.JOIN_PROBE_FANOUT", "SR3514.NODE_TIME_SHARE", "SR3514.ROW_FLOW_AMPLIFICATION", "SR3514.QUERY_WIDE"].map((rule) => html.indexOf(`aria-label="Finding ${rule}"`));
    expect(order.every((index, position) => index > 0 && (position === 0 || index > order[position - 1]))).toBe(true);
    expect(html.match(/finding-subject/g)).toHaveLength(1);
  });
});

describe("EvidenceGaps held-back checks (v0.7-02)", () => {
  const notApplicable = (rule_id: string, missing_prerequisites: string[]): AnalysisSnapshotReport["rule_evaluations"][number] => ({
    rule_id, rule_version: "0.3.0", status: "not_applicable", evidence_completeness: "unavailable", evidence_completeness_reasons: [], subject_ref: null,
    applicability_checks: [], input_observation_refs: [], comparisons: [], threshold_receipts: [], missing_prerequisites, evidence_refs: [], limitations: [], finding_id: null,
  });

  it("lists checks blocked by a missing counter, not checks whose operator this plan does not have", () => {
    const report = { ...sr3514JsonMinimal, findings: [], rule_evaluations: [
      notApplicable("SR3514.SPILL_OBSERVED", ["spill_observation_missing"]),
      notApplicable("SR3514.JOIN_PROBE_FANOUT", ["join_probe_operator_missing"]),
      notApplicable("SR3514.BROADCAST_EXCHANGE_BYTES", ["broadcast_exchange_missing"]),
    ] };
    expect(heldBackEvaluations(report).map((evaluation) => evaluation.rule_id)).toEqual(["SR3514.SPILL_OBSERVED"]);
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SelectionProvider><EvidenceGaps report={report} /></SelectionProvider></LocaleProvider>);
    expect(html).toContain("溢写检查");
    expect(html).toContain("溢写计数");
    expect(html).not.toContain("Join 扇出检查");
    expect(html).not.toContain("join_probe_operator_missing");
  });
});

describe("EvidenceGaps re-capture hint (v0.7-02)", () => {
  it("offers a re-capture only for gaps a fresh run can close", () => {
    const render = (report: AnalysisSnapshotReport) => renderToStaticMarkup(<LocaleProvider initialLocale="en"><SelectionProvider><EvidenceGaps report={report} /></SelectionProvider></LocaleProvider>);
    const exactRows = { ...sr3514JsonMinimal.execution_summary.result_rows, state: "exact" as const };
    const limitsOnly: AnalysisSnapshotReport = { ...sr3514JsonMinimal, findings: [], rule_evaluations: [], graph_view: { ...sr3514JsonMinimal.graph_view, unconnected_node_ids: [], unresolved_relations: [] }, execution_summary: { ...sr3514JsonMinimal.execution_summary, result_rows: exactRows }, completeness: { ...sr3514JsonMinimal.completeness, metrics: { state: "partial", present_count: 1, expected_count: null, reasons: ["runtime_profile_registry_backed_metrics"] } } };
    expect(render(limitsOnly)).toContain("Metrics");
    expect(render(limitsOnly)).not.toContain("capture again");
    expect(render({ ...limitsOnly, execution_summary: { ...limitsOnly.execution_summary, result_rows: { ...exactRows, state: "unavailable" as const } } })).toContain("capture again");
  });
});
