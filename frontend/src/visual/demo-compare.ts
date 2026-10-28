// Demo comparison for design board 04 (前后对比), development harness only.
// The baseline is the board-02 Profile (member 7e8c3a91 of the Main family);
// the candidate is a later execution of the same query after a filter was
// added. Values follow the design's illustrative numbers where the backend
// has an equivalent query metric; nothing here reaches the production bundle.
import type {
  AnalysisSnapshotReport,
  AuditOccurrencePresentation,
  AuditOccurrenceResponse,
  Comparison,
  ConnectedProfileComparisonRequest,
  ConnectedProfileComparisonResponse,
  ConnectedQueryRef,
  Finding,
  MetricDelta,
} from "../../../contracts/generated/doctor-contracts";
import { sr3514ConnectedContractGolden } from "../../../contracts/generated/connected-contract-fixtures";
import { cell, doctorReport, GB, MB, ns, subject } from "./demo-doctor";

export const CANDIDATE_QUERY_ID = "2b91f0c4-77ae-41c9-8a3d-6e5f7a8b9c0d";

/** The Main family's members plus the candidate execution (14:31:52, 9.4 s). */
export function membersWithCandidate(members: AuditOccurrenceResponse): AuditOccurrenceResponse {
  const template = members.rows[0];
  const candidate: AuditOccurrencePresentation = {
    ...template,
    query: {
      ...template.query, query_id: CANDIDATE_QUERY_ID, observed_at_utc: "2026-10-21T14:31:52Z", observed_at_local: "2026-10-21 14:31:52",
      query_time_ms: 9400, profile_fetch: { state: "available", reason: "can_be_attempted" }, source_refs: ["audit:row:candidate"],
    },
    metrics: template.metrics.map((metric) => metric.metric_id === "query_time" && metric.canonical_number
      ? { ...metric, input: { state: "text" as const, value: ns(9.4) }, canonical_number: { ...metric.canonical_number, value: ns(9.4) } }
      : metric),
  };
  // Ordered by duration, as the member read is: 27.8 s, then 9.4 s.
  return { ...members, rows: [members.rows[0], candidate, ...members.rows.slice(1)] };
}

const query = (metric: string, raw: string, unit: string, display: string, line: number) => cell("query", metric, raw, unit, display, line);

function baselineReport(): AnalysisSnapshotReport {
  const report = doctorReport();
  report.derived_metric_cells = [
    ...report.derived_metric_cells,
    query("QueryTotalTime", ns(27.8), "ns", "27.8 s", 6),
    query("QueryCumulativeScanTime", ns(4.9), "ns", "4.9 s", 12),
    query("QuerySpillBytes", "0", "byte", "0", 13),
    query("QueryCumulativeOperatorTime", ns(27.4), "ns", "27.4 s", 14),
  ];
  return report;
}

function candidateReport(base: AnalysisSnapshotReport): AnalysisSnapshotReport {
  const report = structuredClone(base);
  report.analysis_key = "demo-board-04-candidate";
  report.query_summary = report.query_summary ? { ...report.query_summary, query_id: CANDIDATE_QUERY_ID } : report.query_summary;
  // The added filter is printed on the orders scan.
  report.graph_view = {
    ...report.graph_view,
    nodes: report.graph_view.nodes.map((node) => node.node_id === "0"
      ? { ...node, sql_facts: [...(node.sql_facts ?? []), { kind: "predicate" as const, value: "order_date >= '2026-10-01'", source_refs: ["text:118"] }] }
      : node),
  };
  report.derived_metric_cells = [
    query("QueryTotalTime", ns(9.4), "ns", "9.4 s", 6),
    query("QueryExecutionWallTime", ns(9.3), "ns", "9.3 s", 8),
    query("QueryCumulativeCpuTime", ns(62), "ns", "1m 02s", 10),
    query("QueryPeakMemoryUsagePerNode", (39n * GB / 10n).toString(), "byte", "3.9 GB", 11),
    query("QueryCumulativeScanTime", ns(1.4), "ns", "1.4 s", 12),
    query("QuerySpillBytes", (128n * MB).toString(), "byte", "128 MB", 13),
    query("QueryCumulativeNetworkTime", ns(0.8), "ns", "0.8 s", 15),
    query("QueryCumulativeOperatorTime", ns(8.1), "ns", "8.1 s", 14),
  ];
  // The join and scan findings are gone; aggregation now spills.
  const keep = report.findings.filter((finding) => finding.finding_id === "finding:demo:agg" || finding.finding_id === "finding:demo:estimate");
  const spill: Finding = { ...keep[0], finding_id: "finding:demo:agg-spill", rule_id: "aggregate.spill@v3", category: "聚合阶段出现溢写", severity: "medium", confidence: "medium" };
  report.findings = [...keep, spill];
  return report;
}

function delta(metric: string, state: MetricDelta["state"], fields: Partial<MetricDelta>): MetricDelta {
  return {
    metric_id: metric, subject_match: "query", state,
    baseline_metric_cell_ref: `metric:query:${metric}`, candidate_metric_cell_ref: `metric:query:${metric}`,
    absolute_delta: null, percentage_state: "computed", percentage_value: null, fidelity: "exact",
    bounds: null, percentage_bounds: null, order_rank: null, limitations: [],
    ...fields,
  };
}
const integer = (value: string) => ({ kind: "integer" as const, value, scale: null });
const ratio = (value: string) => ({ kind: "decimal" as const, value, scale: 9 });

/** The Demo cluster's answer to "compare the baseline with this execution" (board 01 → 04). */
export function demoComparison(request: ConnectedProfileComparisonRequest, baselineQuery: ConnectedQueryRef, candidateQuery: ConnectedQueryRef): ConnectedProfileComparisonResponse {
  const template = sr3514ConnectedContractGolden.profile_comparison_response as unknown as ConnectedProfileComparisonResponse;
  const baseline = { ...baselineReport(), analysis_state: "complete" as const };
  const candidate = { ...candidateReport(baseline), analysis_state: "complete" as const };
  const entry = (finding: Finding) => `${finding.rule_id}:${finding.subject_ref ?? ""}`;
  const byId = (report: AnalysisSnapshotReport, id: string) => report.findings.find((finding) => finding.finding_id === id)!;
  const comparison: Comparison = {
    ...template.comparison,
    baseline: { outcome: { report: baseline } },
    candidate: { outcome: { report: candidate } },
    state: "comparable",
    reasons: [],
    compatibility_delta: [],
    structure_delta: { state: "changed", matches: ["0", "1", "2", "4", "5", "6", "7"].map((id) => `${id}:stable_id@1`), added: [], removed: [], changed: ["0"], edge_delta: [], unresolved: [] },
    metric_deltas: [
      delta("QueryTotalTime", "comparable", { absolute_delta: integer(`-${ns(18.4)}`), percentage_value: ratio("-0.661870503") }),
      delta("QueryExecutionWallTime", "comparable", { absolute_delta: integer(`-${ns(18.3)}`), percentage_value: ratio("-0.663043478") }),
      delta("QueryCumulativeCpuTime", "comparable", { absolute_delta: integer(`-${ns(116)}`), percentage_value: ratio("-0.651685393") }),
      delta("QueryCumulativeScanTime", "comparable", { absolute_delta: integer(`-${ns(3.5)}`), percentage_value: ratio("-0.714285714") }),
      delta("QueryPeakMemoryUsagePerNode", "comparable", { absolute_delta: integer(`-${(3n * GB / 10n).toString()}`), percentage_value: ratio("-0.071428571") }),
      delta("QuerySpillBytes", "comparable", { absolute_delta: integer((128n * MB).toString()), percentage_state: "undefined_baseline_zero" }),
      delta("QueryCumulativeNetworkTime", "not_comparable", { baseline_metric_cell_ref: null, percentage_state: "missing", fidelity: null, limitations: ["metric_missing_on_one_side"] }),
      delta("QueryCumulativeOperatorTime", "not_comparable", { percentage_state: "semantic_mismatch", fidelity: null, limitations: ["registry_semantic_mismatch"] }),
    ],
    finding_delta: {
      added: [entry(byId(candidate, "finding:demo:agg-spill"))],
      resolved: [entry(byId(baseline, "finding:demo:join")), entry(byId(baseline, "finding:demo:scan"))],
      persisting: [entry(byId(baseline, "finding:demo:agg")), entry(byId(baseline, "finding:demo:estimate"))],
      changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [],
    },
    diagnostics: [],
    limitations: [],
  };
  void subject;
  return {
    baseline: { query: { ...baselineQuery, discovery_generation: request.discovery_generation }, input_origin: "connected_profile_fetch" },
    candidate: { query: { ...candidateQuery, discovery_generation: request.discovery_generation }, input_origin: "candidate_profile_user_external_rerun" },
    comparison,
    validation_context: { ...template.validation_context, same_connection_session: true, same_discovery_generation: true },
    // The backend's own connected-comparison limitations.
    limitations: ["candidate_profile_user_external_rerun_is_user_responsibility", "raw_connector_profile_bytes_not_returned_by_connected_profile_comparison", "observed_changes_do_not_prove_causal_improvement"],
  };
}
