// Demo report for design board 02 (诊断 · 证据工作台), development harness only.
// It starts from a real retained 3.5.14 report so every section keeps its
// contract shape, then replaces the plan, summary, metrics and findings with
// the design's illustrative query (orders ⋈ customers, grouped by customer).
// Findings use the design's illustrative rule ids; the product renders the
// DTO's own text for rules outside its catalog, exactly as it would for a
// future rule. Nothing here reaches the production bundle.
import type {
  AnalysisSnapshotReport,
  ConnectedQueryRef,
  Finding,
  GraphEdge,
  GraphNode,
  MetricCell,
  OperatorRow,
  PlanNodeSummary,
  Recommendation,
  ProfileFetchRequest,
  ProfileFetchResponse,
  RuleEvaluation,
  SourceLine,
  SummaryValue,
} from "../../../contracts/generated/doctor-contracts";
import { bytesText } from "../lib/duration-display";
import base from "../../../fixtures/expected/3.5.14/sr3514_execution_graph.report.json";

export const GB = 1024n ** 3n;
export const MB = 1024n ** 2n;
export const ns = (seconds: number) => String(Math.round(seconds * 1e9));

function value(raw: string, unit: string, display: string, line: number | null): SummaryValue {
  return { state: "exact", canonical_number: { kind: "integer", value: raw, scale: null }, canonical_unit: unit, display_text: display, reasons: [], source_refs: line === null ? [] : [`text:${line}`], bounds: null };
}
const missingValue: SummaryValue = { state: "unavailable", canonical_number: null, canonical_unit: null, display_text: null, reasons: ["metric_missing"], source_refs: [], bounds: null };

export function cell(subject: string, metric: string, raw: string, unit: string, display: string, line: number): MetricCell {
  return { metric_cell_ref: `metric:${subject}:${metric}`, metric_id: metric, subject_ref: subject, display_text: display, order_rank: null, source_refs: [`text:${line}`], reasons: [], state: "exact", canonical_number: { kind: "integer", value: raw, scale: null }, canonical_unit: unit, fidelity: "exact", bounds: null } as MetricCell;
}

type NodeSeed = { id: string; type: string; fact: [kind: "table" | "join_condition" | "fields", text: string] | null; time: number; share: number; hot: PlanNodeSummary["hotspot"]; rows: [raw: string, display: string] | null; inRows: [raw: string, display: string] | null; memory: bigint; line: number };
const NODES: NodeSeed[] = [
  { id: "4", type: "HASH_JOIN", fact: ["join_condition", "INNER JOIN"], time: 12.6, share: 4600, hot: "most_consuming", rows: ["1080000000", "1.08 B"], inRows: ["1102223445", "1.10 B"], memory: 19n * GB / 10n, line: 205 },
  { id: "6", type: "AGGREGATE", fact: ["fields", "customer_id"], time: 5.8, share: 2100, hot: "second_most_consuming", rows: ["3200000", "3.2 M"], inRows: ["1080000000", "1.08 B"], memory: 42n * GB / 10n, line: 232 },
  { id: "0", type: "OLAP_SCAN", fact: ["table", "demo.orders"], time: 4.9, share: 1800, hot: "second_most_consuming", rows: ["1102223445", "1.10 B"], inRows: null, memory: 512n * MB, line: 120 },
  { id: "1", type: "OLAP_SCAN", fact: ["table", "demo.customers"], time: 0.8, share: 300, hot: "none", rows: ["2401882", "2.4 M"], inRows: null, memory: 64n * MB, line: 150 },
  { id: "2", type: "EXCHANGE", fact: ["fields", "BROADCAST"], time: 0.6, share: 200, hot: "none", rows: ["2401882", "2.4 M"], inRows: ["2401882", "2.4 M"], memory: 12n * MB, line: 175 },
  { id: "7", type: "RESULT_SINK", fact: null, time: 0.1, share: 40, hot: "none", rows: ["3200000", "3.2 M"], inRows: ["3200000", "3.2 M"], memory: 8n * MB, line: 250 },
];

export const subject = (id: string) => `operator-group:0:${id}`;

function graphNode(id: string, type: string, fact: NodeSeed["fact"], quality: GraphNode["source_quality"], line: number): GraphNode {
  return {
    node_id: id, entity_kind: "plan_node", plan_node_id: id, fragment_id: "0", pipeline_id: null, plan_node_type: type, display_name: type, source_ordinal: Number(id),
    native_operator_refs: [{ operator_ref: subject(id), source_ordinal: 0, source_role: "native", source_refs: [`text:${line}`] }],
    subordinate_operator_refs: [], metric_observation_refs: [], execution_state: null, source_quality: quality, source_refs: [`text:${line}`],
    sql_facts: fact ? [{ kind: fact[0], value: fact[1], source_refs: [`text:${line}`] }] : [],
  };
}

function edge(from: string, to: string, derived = false): GraphEdge {
  return { edge_id: `${derived ? "derived" : "explicit"}:${from}:${to}`, from, to, relation: "plan_child", direction_semantics: "parent_to_child", source: derived ? "verified_derived" : "explicit", quality: "exact", source_ordinal: null, input_role: null, derivation_recipe: null, restates_edge_id: null, evidence_refs: ["text:51"] } as GraphEdge;
}

function summary(seed: NodeSeed): PlanNodeSummary {
  const time = value(ns(seed.time), "ns", `${seed.time} s`, seed.line + 4);
  return {
    node_id: seed.id, plan_node_id: seed.id, fragment_ref: "0", total_time: time, time_share_basis_points: seed.share,
    time_share_display: seed.share < 100 ? "<1%" : `${seed.share / 100}%`, hotspot: seed.hot,
    output_rows: seed.rows ? value(seed.rows[0], "item", seed.rows[1], seed.line + 6) : missingValue,
    operators: [{ operator_type: seed.type, role: "native", fragment_ref: "0", operator_time: time, pull_rows: seed.rows ? value(seed.rows[0], "item", seed.rows[1], seed.line + 6) : missingValue, push_rows: seed.inRows ? value(seed.inRows[0], "item", seed.inRows[1], seed.line + 7) : missingValue, source_refs: [`text:${seed.line}`] }],
    reasons: [],
  };
}

function operatorRow(seed: NodeSeed): OperatorRow {
  const s = subject(seed.id);
  const cells = [
    cell(s, "OperatorTotalTime", ns(seed.time), "ns", `${seed.time} s`, seed.id === "4" ? 209 : seed.line + 4),
    cell(s, "PeakMemoryUsage", seed.memory.toString(), "byte", bytesText(seed.memory.toString()) ?? "", seed.line + 5),
  ];
  // Board 03 cites the orders scan's partition counters from its first two steps.
  if (seed.id === "0") cells.push(cell(s, "PartitionsHit", "412", "partition", "412 / 412", 41), cell(s, "PushdownPredicates", "0", "item", "0", 44));
  if (seed.id === "2") cells.push(cell(s, "PartType", "0", "item", "BROADCAST", 176));
  if (seed.id === "4") cells.push(cell(s, "ProbeRows", "1102223445", "item", "1,102,223,445", 214), cell(s, "BuildRows", "2401882", "item", "2,401,882", 198));
  return { operator_ref: s, plan_node_id: seed.id, operator_type: seed.type, metric_cells: cells, source_refs: [`text:${seed.line}`] };
}

type FindingSeed = { id: string; rule: string; category: string; severity: string; node: string; confidence: Finding["confidence"]; completeness: Finding["evidence_completeness"]; measurements: [string, string][]; refs: string[]; limitations: string[]; direction: string; rec: Omit<Recommendation, "expected_direction" | "validation_steps"> };

const recStep = (title: string, detail: string, evidence_refs: string[] = []) => ({ title, detail, evidence_refs });
const down = (metric_id: string) => ({ metric_id, direction: "decrease" as const });
const FINDINGS: FindingSeed[] = [
  { id: "join", rule: "join.probe_dominant", category: "Hash Join 探测侧数据量大，耗时集中", severity: "high", node: "4", confidence: "high", completeness: "complete", measurements: [["node_time_share", "46%"], ["探测", "1.10 B 行"]], refs: [`metric:${subject("4")}:ProbeRows`, `metric:${subject("4")}:BuildRows`, `metric:${subject("4")}:OperatorTotalTime`], limitations: ["单次 Profile", "未采集集群负载与并发。占比按算子累计耗时计算，不等于墙钟时间"], direction: "先确认 orders 的分区裁剪与谓词下推，减少探测侧行数；再复核 Join 分布方式。不会自动修改 SQL。",
    rec: { recommendation_id: "rec.join.reduce_probe@v2", kind: "optimization_target", title: "减少 Hash Join 探测侧行数", next_step: "优先在上游扫描减少行数，再考虑 Join 本身", risk_level: "low", risk: "仅改变过滤条件位置；若改写谓词，需确认语义等价", result_effect: "unchanged",
      steps: [
        recStep("确认过滤条件作用在分区列上", "检查 WHERE 是否直接约束 orders 的分区列，避免对分区列做函数包裹。", [`metric:${subject("0")}:PartitionsHit`]),
        recStep("检查谓词是否下推到扫描", "在原始结构视图查看 Plan 0 的 PREDICATES，确认没有停留在 Join 之后。", [`metric:${subject("0")}:PushdownPredicates`]),
        recStep("复核 Join 分布方式", "构建侧 2.4 M 行，当前为 BROADCAST（显式）。若探测侧减少后仍集中，再评估 Join 顺序。", [`metric:${subject("2")}:PartType`]),
      ],
      expected_changes: [down("ProbeRows"), down("OperatorTotalTime")], limitations: ["未读取表的分区定义", "分区裁剪结论依赖 Profile 中的 PartitionsHit"] } },
  { id: "scan", rule: "scan.partition_pruning", category: "扫描未命中分区裁剪", severity: "medium", node: "0", confidence: "medium", completeness: "complete", measurements: [["扫描", "1.10 B 行"], ["读取", "412 个分区中的 412 个"]], refs: [`metric:${subject("0")}:OperatorTotalTime`], limitations: ["单次 Profile"], direction: "确认 orders 的分区条件能被裁剪。",
    rec: { recommendation_id: "rec.scan.partition_pushdown@v1", kind: "investigation", title: "确认 orders 分区裁剪与谓词下推", next_step: "先确认 orders 的分区条件能被裁剪", risk_level: "low", risk: "仅改变过滤条件位置", result_effect: "unchanged",
      steps: [recStep("确认过滤条件作用在分区列上", "检查 WHERE 是否直接约束 orders 的分区列。", [`metric:${subject("0")}:PartitionsHit`])],
      expected_changes: [down("ScanRows")], limitations: ["未读取表的分区定义"] } },
  { id: "agg", rule: "aggregate.memory_peak", category: "聚合阶段内存峰值偏高", severity: "medium", node: "6", confidence: "medium", completeness: "partial", measurements: [["峰值内存", "4.2 GB"], ["输出", "3.2 M 组"]], refs: [`metric:${subject("6")}:PeakMemoryUsage`], limitations: ["单次 Profile"], direction: "复核聚合键基数与预聚合。",
    rec: { recommendation_id: "rec.aggregate.memory@v1", kind: "investigation", title: "复核聚合分组基数与内存", next_step: "先复核聚合键基数与预聚合", risk_level: "medium", risk: "调整分组键会改变结果形态", result_effect: "unchanged",
      steps: [recStep("检查分组数量", "输出接近输入说明分组键几乎没有压缩数据。", [`metric:${subject("6")}:PeakMemoryUsage`])],
      expected_changes: [down("PeakMemoryUsage")], limitations: ["未读取分组键基数"] } },
  { id: "estimate", rule: "scan.estimate_deviation", category: "估算行数与实际偏差较大", severity: "low", node: "1", confidence: "low", completeness: "partial", measurements: [["估算", "63 K → 实际 2.4 M"]], refs: [`metric:${subject("1")}:OperatorTotalTime`], limitations: ["单次 Profile"], direction: "收集 customers 统计信息后复核。",
    rec: { recommendation_id: "rec.scan.statistics@v1", kind: "investigation", title: "收集 customers 统计信息", next_step: "收集统计信息后复核估算", risk_level: "low", risk: "仅更新统计信息", result_effect: "unchanged",
      steps: [recStep("收集统计信息", "对 customers 执行 ANALYZE TABLE 后重新采集 Profile。")],
      expected_changes: [down("OperatorTotalTime")], limitations: ["未读取表统计信息"] } },
];

function finding(seed: FindingSeed): Finding {
  return {
    finding_id: `finding:demo:${seed.id}`, rule_id: `${seed.rule}@v3`, rule_version: "3", category: seed.category, severity: seed.severity,
    confidence: seed.confidence, confidence_reasons: [], evidence_completeness: seed.completeness, evidence_completeness_reasons: [],
    subject_ref: subject(seed.node), evidence: seed.refs.map((_, index) => `text:${[214, 198, 209][index] ?? 209}`), limitations: seed.limitations,
    recommendation: { ...seed.rec, expected_direction: seed.direction, validation_steps: [] }, related_finding_refs: [], rule_trace_ref: `rule-trace:demo:${seed.id}`,
    measurements: seed.measurements.map(([measurement_id, display_text]) => ({ measurement_id, display_text, threshold_text: null, operator_type: null, input_refs: measurement_id === "node_time_share" ? [`metric:${subject(seed.node)}:OperatorTotalTime`, "metric:query:QueryCumulativeOperatorTime"] : [] })),
  } as Finding;
}

function evaluation(seed: FindingSeed): RuleEvaluation {
  return { rule_id: `${seed.rule}@v3`, rule_version: "3", status: "triggered", evidence_completeness: seed.completeness, evidence_completeness_reasons: [], subject_ref: subject(seed.node), applicability_checks: [], input_observation_refs: seed.refs, comparisons: [], threshold_receipts: [], missing_prerequisites: [], evidence_refs: [], limitations: [], finding_id: `finding:demo:${seed.id}` };
}

/** Two checks the analysis could not run because a prerequisite counter is absent. */
const SUPPRESSED: RuleEvaluation[] = [
  { rule_id: "SR3514.SPILL_OBSERVED", rule_version: "0.3.0", status: "not_applicable", evidence_completeness: "unavailable", evidence_completeness_reasons: ["spill_observation_missing"], subject_ref: null, applicability_checks: [], input_observation_refs: [], comparisons: [], threshold_receipts: [], missing_prerequisites: ["SpillBytes"], evidence_refs: [], limitations: [], finding_id: null },
  { rule_id: "SR3514.OPERATOR_TIME_SKEW", rule_version: "0.3.0", status: "not_applicable", evidence_completeness: "unavailable", evidence_completeness_reasons: ["runtime_operator_missing"], subject_ref: null, applicability_checks: [], input_observation_refs: [], comparisons: [], threshold_receipts: [], missing_prerequisites: ["per_instance_metrics"], evidence_refs: [], limitations: [], finding_id: null },
];

function sourceLines(): SourceLine[] {
  const special: Record<number, string> = { 198: "             - BuildRows: 2.401882M (2401882)", 209: "             - OperatorTotalTime: 12s600ms", 214: "             - ProbeRows: 1.102223445B (1102223445)" };
  return Array.from({ length: 260 }, (_, index) => {
    const line = index + 1;
    return { line_no: line, text: special[line] ?? (line === 1 ? "Query:" : `           - Counter${line}: ${line}`), source_ref: `text:${line}` };
  });
}

export function doctorReport(): AnalysisSnapshotReport {
  const source = base as unknown as { report?: AnalysisSnapshotReport } & AnalysisSnapshotReport;
  const report = structuredClone(source.report ?? source);
  const seeds = new Map(NODES.map((seed) => [seed.id, seed] as const));
  const nodes = ["0", "1", "2", "4", "5", "6", "7"].map((id) => id === "5"
    ? graphNode("5", "PROJECT", null, "partial", 190)
    : graphNode(id, seeds.get(id)!.type, seeds.get(id)!.fact, "exact", seeds.get(id)!.line));
  report.graph_view = {
    ...report.graph_view, state: "partial", root_node_ids: ["7"], nodes,
    edges: [edge("7", "6"), edge("6", "4"), edge("4", "0"), edge("4", "2", true), edge("2", "1")],
    unconnected_node_ids: ["5"], unresolved_relations: [],
  };
  const total = NODES.reduce((sum, seed) => sum + seed.time, 0);
  report.execution_summary = {
    ...report.execution_summary, state: "available",
    query_total_time: value(ns(27.8), "ns", "27.8 s", 6),
    cumulative_operator_time: value(ns(total), "ns", "27.4 s", 9),
    result_rows: missingValue,
    nodes: NODES.map(summary),
    reasons: [],
  } as AnalysisSnapshotReport["execution_summary"];
  report.operator_rows = NODES.map(operatorRow);
  report.findings = FINDINGS.map(finding);
  report.rule_evaluations = [...FINDINGS.map(evaluation), ...SUPPRESSED];
  const query = (metric: string, raw: string, unit: string, display: string, line: number) => cell("query", metric, raw, unit, display, line);
  report.derived_metric_cells = [
    query("QueryPlanningTime", ns(0.112), "ns", "112 ms", 7),
    query("QueryExecutionWallTime", ns(27.6), "ns", "27.6 s", 8),
    query("QueryCumulativeCpuTime", ns(178), "ns", "2m 58s", 10),
    query("QueryPeakMemoryUsagePerNode", (42n * GB / 10n).toString(), "byte", "4.2 GB", 11),
  ];
  report.source_view = { ...report.source_view, source_kind: "text_profile", plain_text_lines: sourceLines(), json_tokens: [] };
  report.analysis_key = "demo-board-02";
  report.query_summary = report.query_summary ? { ...report.query_summary, query_id: "7e8c3a91-0b2f-4d73-9a1e-5c6b7d8e4d73" } : report.query_summary;
  return report;
}

/** The Demo cluster's answer to "fetch this member's Profile" (board 01 → 02). */
export function doctorFetch(request: ProfileFetchRequest, member: ConnectedQueryRef): ProfileFetchResponse {
  const report = doctorReport();
  return {
    query: { ...member, discovery_generation: request.discovery_generation, profile_fetch: { state: "available", reason: "profile_fetched" } },
    input_origin: "connected_profile_fetch", source_kind: "text_profile", state: "fetched",
    source_hash: report.input.source_hash, analysis_key: report.analysis_key, analysis_receipt: { components: [] },
    outcome: { report }, source_refs: [], limitations: [],
  };
}
