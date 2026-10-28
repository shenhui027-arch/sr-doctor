import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import versionPack from "../../../../backend/crates/version-packs/starrocks_3_5_14/version-pack.json";
import complexReportJson from "../../../../fixtures/expected/3.5.14/docker_runtime_corpus_complex.report.json";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { AnalysisSnapshotReport, BoundedInterval, CanonicalNumber, Comparison, ConnectedProfileComparisonResponse, MetricCell, MetricDelta } from "../../../../contracts/generated/doctor-contracts";
import { metricLabel } from "../../i18n/metric-label";
import { LocaleProvider } from "../../i18n/locale";
import { CompareWorkspace, PendingComparison, ProvenancePanel } from "./CompareWorkspace";
import { recommendationFixture } from "../recommendations/recommendation-fixture";

const int = (value: string): CanonicalNumber => ({ kind: "integer", value, scale: null });

function cell(ref: string, metricId: string, value: string, unit: string, extra: Partial<MetricCell> = {}): MetricCell {
  return { metric_cell_ref: ref, metric_id: metricId, subject_ref: "query", display_text: `${value} ${unit}`, order_rank: 0, source_refs: ["json:/x"], reasons: [], state: "exact", canonical_number: int(value), canonical_unit: unit, fidelity: "exact", bounds: null, ...extra } as MetricCell;
}

function report(cells: MetricCell[], key: string): AnalysisSnapshotReport {
  return { ...sr3514JsonMinimal, input: { ...sr3514JsonMinimal.input, source_hash: key.repeat(64).slice(0, 64) }, derived_metric_cells: cells };
}

function delta(metricId: string, baselineRef: string | null, candidateRef: string | null, extra: Partial<MetricDelta> = {}): MetricDelta {
  return { metric_id: metricId, subject_match: "query", state: "comparable", baseline_metric_cell_ref: baselineRef, candidate_metric_cell_ref: candidateRef, absolute_delta: int("0"), percentage_state: "computed", percentage_value: int("0"), fidelity: "exact", bounds: null, percentage_bounds: null, order_rank: 0, limitations: [], ...extra };
}

/** One spill metric: baseline 0 B, candidate 122.499 MB (128,450,000 bytes, StarRocks binary MB). */
function comparison(): Comparison {
  const baseline = report([cell("b", "QuerySpillBytes", "0", "byte")], "b");
  const candidate = report([cell("c", "QuerySpillBytes", "128450000", "byte")], "c");
  return {
    schema_version: "1.1", comparison_receipt: { components: [] }, comparison_key: "comparison", baseline: { outcome: { report: baseline } }, candidate: { outcome: { report: candidate } }, state: "partially_comparable", reasons: ["partial topology"], compatibility_delta: [],
    structure_delta: { state: "partial", matches: [], added: ["node:new"], removed: [], changed: ["node:scan"], edge_delta: [], unresolved: ["sink missing"] },
    metric_deltas: [delta("QuerySpillBytes", "b", "c", { absolute_delta: int("128450000"), percentage_state: "undefined_baseline_zero", percentage_value: null, limitations: ["baseline_exact_zero"] })],
    finding_delta: { added: ["finding:new"], resolved: [], persisting: [], changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [] }, diagnostics: [], limitations: ["partial topology", "causal attribution limited"],
  };
}

function withValues(baselineValue: string, candidateValue: string, change: string): Comparison {
  const next = comparison();
  next.baseline = { outcome: { report: report([cell("b", "QuerySpillBytes", baselineValue, "byte")], "b") } };
  next.candidate = { outcome: { report: report([cell("c", "QuerySpillBytes", candidateValue, "byte")], "c") } };
  next.metric_deltas = [{ ...next.metric_deltas[0], absolute_delta: int(change) }];
  return next;
}

function connectedComparison(): ConnectedProfileComparisonResponse {
  const baseline = sr3514ConnectedContractGolden.query_response.queries[0];
  return {
    baseline: { query: baseline, input_origin: "connected_profile_fetch" },
    candidate: { query: { ...baseline, query_id: "candidate-query" }, input_origin: "candidate_profile_user_external_rerun" },
    comparison: comparison(),
    validation_context: sr3514ConnectedContractGolden.profile_comparison_response.validation_context,
    limitations: ["candidate_profile_user_external_rerun_required"],
  };
}

const complexReport = complexReportJson as unknown as AnalysisSnapshotReport;

/**
 * Test data only: the shape the comparator 0.2.0 returns for a text Profile
 * compared with itself (every cell paired by key, rounded values as intervals).
 */
function complexSelfComparison(): Comparison {
  const cells = [...complexReport.derived_metric_cells, ...complexReport.operator_rows.flatMap((row) => row.metric_cells)];
  const deltas = cells.map((item): MetricDelta => {
    const base = { metric_id: item.metric_id, subject_match: item.subject_ref, state: "comparable" as const, baseline_metric_cell_ref: item.metric_cell_ref, candidate_metric_cell_ref: item.metric_cell_ref, order_rank: item.order_rank };
    if (item.state !== "exact") throw new Error("fixture cells are exact");
    if (!item.bounds) {
      const zero = item.canonical_number.value === "0";
      return { ...base, absolute_delta: int("0"), percentage_state: zero ? "undefined_baseline_zero" : "computed", percentage_value: zero ? null : int("0"), fidelity: "exact", bounds: null, percentage_bounds: null, limitations: [] };
    }
    const lower = BigInt(item.bounds.lower.value);
    const upper = BigInt(item.bounds.upper.value);
    const bounds: BoundedInterval = { lower: int(String(lower - upper)), upper: int(String(upper - lower)), lower_inclusive: item.bounds.lower_inclusive && item.bounds.upper_inclusive, upper_inclusive: item.bounds.upper_inclusive && item.bounds.lower_inclusive };
    const baselineHasZero = lower === 0n;
    return { ...base, absolute_delta: null, percentage_state: baselineHasZero ? "undefined_baseline_interval_contains_zero" : "approximate", percentage_value: null, fidelity: "interval", bounds, percentage_bounds: baselineHasZero ? null : { lower: { kind: "decimal", value: "-0.000828", scale: 9 }, upper: { kind: "decimal", value: "0.000829", scale: 9 }, lower_inclusive: true, upper_inclusive: true }, limitations: ["bounded_metric_delta"] };
  });
  return {
    schema_version: "1.1", comparison_receipt: { components: [] }, comparison_key: "self", baseline: { outcome: { report: complexReport } }, candidate: { outcome: { report: complexReport } }, state: "partially_comparable", reasons: ["bounded_metric_delta"], compatibility_delta: [],
    structure_delta: { state: "unchanged", matches: complexReport.graph_view.nodes.map((node) => `${node.node_id}:stable_id@1`), added: [], removed: [], changed: [], edge_delta: [], unresolved: [] },
    metric_deltas: deltas,
    finding_delta: { added: [], resolved: [], persisting: [], changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [] }, diagnostics: [], limitations: ["bounded_metric_delta"],
  };
}

const render = (next: Comparison | null, props: Partial<Parameters<typeof CompareWorkspace>[0]> = {}) => renderToStaticMarkup(<CompareWorkspace comparison={next} {...props} />);
const renderZh = (next: Comparison) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><CompareWorkspace comparison={next} /></LocaleProvider>);
const occurrences = (html: string, needle: string) => html.split(needle).length - 1;

// The complex self-comparison renders ~770 rows; render it once and give those
// tests headroom under the full parallel suite (same reason as graph-layout's 45-node cases).
const COMPLEX_TIMEOUT_MS = 30_000;
let cachedSelfHtml: string | null = null;
const selfHtml = () => (cachedSelfHtml ??= render(complexSelfComparison()));

describe("CompareWorkspace before a comparison exists", () => {
  it("renders nothing: no placeholder block, empty tabs or disabled export", () => {
    expect(render(null)).toBe("");
  });
});

describe("CompareWorkspace with a comparison", () => {
  it("renders backend states without a health score or improvement colouring", () => {
    const html = render(comparison());
    expect(html).toContain("Partially comparable");
    expect(html).toContain("Undefined · baseline = 0");
    expect(html).not.toContain("undefined_baseline_zero");
    expect(html).not.toMatch(/\bAI\b|performance score|health score|tone-success|tone-danger/i);
    const exportButton = html.match(/<button[^>]*>Export comparison report<\/button>/)?.[0];
    expect(exportButton).toBeDefined();
    expect(exportButton).not.toContain("disabled");
    expect(html).not.toContain("Not available in v0.1");
  });

  it("keeps the Raw / Provenance entry as an enabled button and renders the panel class", () => {
    const html = render(comparison());
    const button = html.match(/<button[^>]*>Raw \/ Provenance<\/button>/)?.[0];
    expect(button).toBeDefined();
    expect(button).not.toContain("disabled");
    expect(button).toContain('aria-expanded="false"');
    expect(html).not.toContain("compare-provenance-panel");
  });

  it("renders read-only identity evidence in the provenance panel", () => {
    const html = renderToStaticMarkup(<ProvenancePanel baseline={complexReport} candidate={null} />);
    expect(html).toContain('class="compare-card compare-provenance-panel"');
    expect(html).toContain("Analysis key");
    expect(html).toContain(complexReport.analysis_key);
    expect(html).toContain("No analyzed Profile selected.");
  });

  it("only renders a swap control when the host supplies a swap handler", () => {
    expect(render(comparison())).not.toContain("Swap baseline and candidate Profiles");
    expect(render(comparison(), { onSwap: () => undefined })).toContain('aria-label="Swap baseline and candidate Profiles"');
    expect(render(comparison())).not.toContain("Replace Profiles");
    expect(render(comparison(), { onReplace: () => undefined })).toContain(">Replace Profiles</button>");
  });

  it("shows each top-level reason once, localized, and does not repeat it as a limitation", () => {
    const next = complexSelfComparison();
    const html = selfHtml();
    expect(occurrences(html, "Values are rounded to display precision, so changes are intervals")).toBe(1);
    expect(html).not.toContain(">bounded_metric_delta<");
    const bounded = next.metric_deltas.filter((item) => item.limitations.includes("bounded_metric_delta")).length;
    expect(html).toContain(`${bounded} metrics`);
    expect(html).not.toContain('aria-labelledby="compare-limitations-title"');
    expect(renderZh(next)).toContain("数值经显示精度舍入，变化以区间表示");
  }, COMPLEX_TIMEOUT_MS);

  it("counts a repeated reason code instead of listing it again", () => {
    const next = comparison();
    next.reasons = ["duplicate_metric_match", "duplicate_metric_match", "bounded_metric_delta"];
    const html = render(next);
    expect(occurrences(html, "Several counters share a name and cannot be paired uniquely")).toBe(1);
    expect(html).toContain("×2");
  });

  it("keeps connected limitations that are not reasons, localized", () => {
    const response = connectedComparison();
    response.limitations = ["observed_changes_do_not_prove_causal_improvement"];
    const html = render(response.comparison, { connectedComparison: response });
    expect(html).toContain("Observed changes do not prove causal improvement");
    expect(html).toContain("Causal attribution limited");
    expect(occurrences(html, "Partial topology")).toBe(1);
  });

  it("puts query-level metrics first, in the fixed display order", () => {
    const next = complexSelfComparison();
    // Reverse the DTO order to prove the display order is not the arrival order.
    next.metric_deltas = [...next.metric_deltas].reverse();
    const html = render(next);
    const queryStart = html.indexOf("compare-query-metrics");
    const operatorStart = html.indexOf("compare-operators");
    expect(queryStart).toBeGreaterThan(-1);
    expect(queryStart).toBeLessThan(operatorStart);
    const querySection = html.slice(queryStart, operatorStart);
    const order = ["QueryTotalTime", "QueryExecutionWallTime", "QueryCumulativeCpuTime", "QueryCumulativeScanTime", "QueryPeakMemoryUsagePerNode", "QuerySpillBytes", "QueryCumulativeNetworkTime", "QueryCumulativeOperatorTime"];
    const positions = order.map((id) => querySection.indexOf(`title="${id}"`));
    expect(positions.every((position) => position > -1)).toBe(true);
    expect([...positions].sort((a, b) => a - b)).toEqual(positions);
    expect(querySection.indexOf('title="QueryCumulativeOperatorTime"')).toBeLessThan(querySection.indexOf('title="QueryPlanningTime"'));
    expect(querySection).not.toContain('title="OperatorTotalTime"');
  }, COMPLEX_TIMEOUT_MS);

  it("groups operator counters by plan node with Plan N · TYPE labels, collapsed, never raw operator-group refs", () => {
    const html = selfHtml();
    expect(html).not.toContain("operator-group:");
    expect(html).toContain('<span class="compare-operator-label">Plan 26 · TOP_N</span>');
    // StarRocks' -1 means "outside the plan tree"; it is not shown as a node id.
    expect(html).toContain('<span class="compare-operator-label">RESULT_SINK</span>');
    expect(html).not.toContain("Plan -1");
    // The exchange appears in sender and receiver fragments: same plan-node label,
    // told apart by the physical operator each group's counters were printed under.
    const exchangeSummaries = html.split("<summary>").filter((part) => part.startsWith('<span class="compare-operator-label">Plan 27 · MERGE_EXCHANGE</span>')).map((part) => part.split("</summary>")[0]);
    expect(exchangeSummaries).toHaveLength(2);
    expect(new Set(exchangeSummaries).size).toBe(2);
    expect(exchangeSummaries.join(" ")).toMatch(/EXCHANGE_SINK|MERGE_EXCHANGE_SOURCE|EXCHANGE_SOURCE|group \d of 2/);
    expect(html).toContain('<details class="compare-operator-group">');
    expect(html).not.toContain("<details open");
    expect(occurrences(html, '<details class="compare-operator-group">')).toBe(complexReport.operator_rows.length);
  }, COMPLEX_TIMEOUT_MS);

  it("shows the counter statistic from match_key as mono secondary text", () => {
    const html = selfHtml();
    expect(html).toContain("<code>__MAX_OF_OperatorTotalTime</code>");
    expect(html).toContain("<code>__MIN_OF_OperatorTotalTime</code>");
    expect(html).toContain('<code class="compare-metric-scope">1:4:LIMIT:26:11</code>');
  }, COMPLEX_TIMEOUT_MS);

  it("says no change within display precision when the DTO interval admits zero, with the interval as a tooltip", () => {
    const next = comparison();
    next.metric_deltas = [delta("QuerySpillBytes", "b", "c", { absolute_delta: null, fidelity: "interval", bounds: { lower: int("-1000"), upper: int("1000"), lower_inclusive: false, upper_inclusive: false }, percentage_state: "approximate", percentage_value: null, percentage_bounds: { lower: { kind: "decimal", value: "-0.000016562", scale: 9 }, upper: { kind: "decimal", value: "0.000016563", scale: 9 }, lower_inclusive: true, upper_inclusive: true }, limitations: ["bounded_metric_delta"] })];
    const html = render(next);
    expect(html).toContain('title="Change interval (−1000 B, 1000 B)">No change within display precision</span>');
    expect(html).toContain('title="Percentage interval [−0.0016562%, 0.0016563%]"');
    expect(html).not.toContain("−0.000016562%");
    expect(renderZh(next)).toContain("在显示精度内无变化");
  });

  it("keeps an interval that excludes zero as an approximate range", () => {
    const next = comparison();
    next.metric_deltas = [delta("QuerySpillBytes", "b", "c", { absolute_delta: null, fidelity: "interval", bounds: { lower: int("0"), upper: int("2000"), lower_inclusive: false, upper_inclusive: true }, percentage_state: "undefined_baseline_interval_contains_zero", percentage_value: null, limitations: ["bounded_metric_delta"] })];
    const html = render(next);
    expect(html).not.toContain("No change within display precision");
    expect(html).toContain('<span class="compare-change up" title="Change interval (0 B, 1.953 KB]">≈ 0 B … 1.953 KB</span>');
    expect(html).toContain("Undefined · baseline range includes 0");
    expect(html).not.toContain("compare-bar-fill");
  });

  it("collapses an interval whose ends read the same, colours it by direction, and bars only the proven change", () => {
    const next = comparison();
    const ratio = (value: string) => ({ kind: "decimal" as const, value, scale: 9 });
    next.metric_deltas = [delta("QueryTotalTime", "b", "c", { absolute_delta: null, fidelity: "interval", bounds: { lower: int("-253001000"), upper: int("-252999000"), lower_inclusive: true, upper_inclusive: true }, percentage_state: "approximate", percentage_value: null, percentage_bounds: { lower: ratio("-0.808310000"), upper: ratio("-0.401000000"), lower_inclusive: true, upper_inclusive: true }, limitations: ["bounded_metric_delta"] })];
    let html = render(next);
    expect(html).toContain('<span class="compare-change down" title="Percentage interval [−80.831%, −40.1%]">≈ −80.8% … −40.1%</span>');
    // The bar draws the end nearest 0, the smallest change the interval proves.
    expect(html).toContain('class="compare-bar-fill down" style="left:29.95%;width:20.05%"');
    next.metric_deltas[0].percentage_bounds = { lower: ratio("-0.808310000"), upper: ratio("-0.808290000"), lower_inclusive: true, upper_inclusive: true };
    html = render(next);
    expect(html).toContain('title="Percentage interval [−80.831%, −80.829%]">≈ −80.8%</span>');
    // Absolute ends that still differ at display precision stay a range.
    expect(html).toContain('class="compare-change down" title="Change interval [−241.281 MB, −241.279 MB]">≈ −241.281 MB … −241.279 MB</span>');
  });

  it.each([
    ["0", "128450000", "128450000", ["0 B", "122.499 MB", "+122.499 MB"]],
    ["1", "0", "-1", ["1 B", "0 B", "−1 B"]],
    ["0", "9007199254740993", "9007199254740993", ["8 PB", "+8 PB"]],
  ] as const)("formats bytes with StarRocks binary units (%s → %s)", (baselineValue, candidateValue, change, expected) => {
    const next = withValues(baselineValue, candidateValue, change);
    const before = JSON.stringify(next);
    const html = render(next);
    for (const value of expected) expect(html).toContain(value);
    expect(html).not.toMatch(/e[+-]\d+ GB|MiB/);
    expect(JSON.stringify(next)).toBe(before);
  });

  it("formats durations and counts through the shared display helpers", () => {
    const next = comparison();
    next.baseline = { outcome: { report: report([cell("wb", "QueryExecutionWallTime", "2410000000", "ns"), cell("rb", "RowsReturned", "1234567", "item")], "b") } };
    next.candidate = { outcome: { report: report([cell("wc", "QueryExecutionWallTime", "1860000000", "ns"), cell("rc", "RowsReturned", "1234500", "item")], "c") } };
    next.metric_deltas = [
      delta("QueryExecutionWallTime", "wb", "wc", { absolute_delta: int("-550000000"), percentage_value: { kind: "decimal", value: "-0.228215768", scale: 9 } }),
      delta("RowsReturned", "rb", "rc", { absolute_delta: int("-67"), percentage_value: { kind: "decimal", value: "-0.000054270", scale: 9 } }),
    ];
    const html = render(next);
    expect(html).toContain(">2.41 s<");
    expect(html).toContain(">1.86 s<");
    expect(html).toContain(">−550 ms<");
    expect(html).toContain(">1,234,567<");
    expect(html).toContain(">−67<");
  });

  it("renders the backend ratio as a percent and draws the bar only from percentage_value", () => {
    const next = comparison();
    next.baseline = { outcome: { report: report([cell("b", "QuerySpillBytes", "100", "byte")], "b") } };
    next.candidate = { outcome: { report: report([cell("c", "QuerySpillBytes", "200", "byte")], "c") } };
    next.metric_deltas = [delta("QuerySpillBytes", "b", "c", { absolute_delta: int("100"), percentage_value: int("1") })];
    const html = render(next);
    expect(html).toContain(">+100%<");
    expect(html).toContain('class="compare-bar-fill up" style="left:50%;width:50%"');

    const negative = comparison();
    negative.metric_deltas = [delta("QuerySpillBytes", "b", "c", { absolute_delta: int("-1"), percentage_value: { kind: "decimal", value: "-0.662000000", scale: 9 } })];
    expect(render(negative)).toContain(">−66.2%<");

    const clamped = comparison();
    clamped.metric_deltas = [delta("QuerySpillBytes", "b", "c", { absolute_delta: int("1"), percentage_value: { kind: "decimal", value: "3.5", scale: 1 } })];
    expect(render(clamped)).toContain('class="compare-bar-fill up clamped" style="left:50%;width:50%"');

    // No computed percentage → no bar.
    expect(render(comparison())).not.toContain("compare-bar-fill");
  });

  it.each([
    ["baseline = 0", { percentage_state: "undefined_baseline_zero" }, "Undefined · baseline = 0", "未定义 · 基线 = 0", null],
    ["missing baseline", { state: "not_comparable", baseline_metric_cell_ref: null, absolute_delta: null, fidelity: null, percentage_state: "missing", percentage_value: null, limitations: ["metric_missing_on_one_side"] }, "No baseline observation", "缺少基线观测值", "Only on one side"],
    ["missing candidate", { state: "not_comparable", candidate_metric_cell_ref: null, absolute_delta: null, fidelity: null, percentage_state: "missing", percentage_value: null, limitations: ["metric_missing_on_one_side"] }, "No candidate observation", "缺少候选观测值", "Only on one side"],
    ["ambiguous", { state: "not_comparable", baseline_metric_cell_ref: null, candidate_metric_cell_ref: null, absolute_delta: null, fidelity: null, percentage_state: "semantic_mismatch", percentage_value: null, limitations: ["duplicate_metric_match"] }, "No unique pairing", "无法唯一配对", "Ambiguous: several counters share this name"],
    ["unit mismatch", { state: "not_comparable", absolute_delta: null, fidelity: null, percentage_state: "semantic_mismatch", percentage_value: null, limitations: ["metric_unit_mismatch"] }, "Meaning differs", "指标含义不同", "Not comparable"],
    ["invalid", { state: "not_comparable", absolute_delta: null, fidelity: null, percentage_state: "invalid", percentage_value: null, limitations: ["metric_state_not_comparable"] }, "Invalid observation", "观测值无效", null],
  ] as const)("renders the %s state as a dashed badge without a number", (_name, patch, english, chinese, reason) => {
    const next = comparison();
    next.metric_deltas = [{ ...next.metric_deltas[0], ...(patch as Partial<MetricDelta>) }];
    const html = render(next);
    const badge = html.match(new RegExp(`<span class="compare-state[^"]*"[^>]*>${english.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}</span>`));
    expect(badge).not.toBeNull();
    // Why a pair is not comparable stays in the absolute column's title, never a number.
    if (reason) expect(html).toContain(`title="${reason}`);
    expect(html).not.toContain("metric meanings differ");
    expect(renderZh(next)).toContain(chinese);
  });

  it("keeps an unknown cell distinct from zero", () => {
    const next = comparison();
    const unknown = { ...cell("b", "QuerySpillBytes", "0", "byte"), state: "unknown", canonical_number: null, canonical_unit: null, fidelity: null, bounds: null } as MetricCell;
    next.baseline = { outcome: { report: report([unknown], "b") } };
    const html = render(next);
    expect(html).toContain('<span class="compare-state">Unknown</span>');
  });

  it("shows a parallelism-gated CPU delta as not comparable, with no change or percent", () => {
    const next = comparison();
    next.baseline = { outcome: { report: report([cell("b", "QueryCumulativeCpuTime", "49594000", "ns")], "b") } };
    next.candidate = { outcome: { report: report([cell("c", "QueryCumulativeCpuTime", "60000000", "ns")], "c") } };
    next.reasons = ["parallelism_shape_mismatch"];
    next.metric_deltas = [delta("QueryCumulativeCpuTime", "b", "c", { state: "not_comparable", absolute_delta: null, percentage_state: "semantic_mismatch", percentage_value: null, fidelity: null, limitations: ["parallelism_shape_mismatch"] })];
    const html = renderZh(next);
    expect(html).toContain("并行度不同");
    expect(html).toContain("Fragment 数或实例数不同，按实例上报求和的计数器（CPU、落盘）不参与比较");
    expect(html).not.toContain("10.406 ms");
    const unknown = comparison();
    unknown.metric_deltas = [delta("QueryCumulativeCpuTime", "b", "c", { state: "not_comparable", absolute_delta: null, percentage_state: "missing", percentage_value: null, fidelity: null, limitations: ["parallelism_shape_unavailable"] })];
    expect(renderZh(unknown)).toContain("并行度未知");
  });

  it("keeps baseline and candidate refs in separate maps when their ref strings collide", () => {
    const next = comparison();
    next.baseline = { outcome: { report: report([cell("same", "QuerySpillBytes", "100", "byte")], "b") } };
    next.candidate = { outcome: { report: report([cell("same", "QuerySpillBytes", "200", "byte")], "c") } };
    next.metric_deltas = [delta("QuerySpillBytes", "same", "same", { absolute_delta: int("100"), percentage_value: int("1") })];
    const html = render(next);
    expect(html).toContain(">100 B<");
    expect(html).toContain(">200 B<");
  });

  it("shows finding change counts and the structural diff from the DTO", () => {
    const next = comparison();
    next.finding_delta = { added: ["a"], resolved: ["r1", "r2"], persisting: ["p"], changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [] };
    const html = render(next);
    expect(html).toMatch(/<span>Resolved<\/span><strong class="ok">2<\/strong>/);
    expect(html).toMatch(/<span>New<\/span><strong class="warn">1<\/strong>/);
    expect(html).toMatch(/<span>Persisting<\/span><strong class="">1<\/strong>/);
    // "Changed" is counted only when the comparator reports one.
    expect(html).not.toContain("<span>Changed</span>");
    next.finding_delta = { ...next.finding_delta, changed: ["c"] };
    expect(render(next)).toMatch(/<span>Changed<\/span><strong class="accent">1<\/strong>/);
    expect(html).toContain("<code>node:new</code>");
    expect(html).toContain("Added in candidate");
    expect(html).toContain("Sink missing");
    expect(html).toContain("Not compared");
  });

  it("labels only the resolved and new entries the comparator names as partial evidence", () => {
    const next = comparison();
    next.finding_delta = { added: ["a"], resolved: ["r1", "r2"], persisting: ["p"], changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [], partial_evidence: ["r2", "a"] };
    const html = render(next);
    expect(html.match(/>Partial evidence</g)).toHaveLength(2);
    expect(html).toContain("2 resolved or new finding(s) rest on partial evidence");
    // An older comparison without the field shows no label.
    next.finding_delta = { ...next.finding_delta, partial_evidence: undefined };
    expect(render(next)).not.toContain("Partial evidence");
  });

  it("labels structural changes by plan node type and id when the graph has the node", () => {
    const next = complexSelfComparison();
    const [first] = complexReport.graph_view.nodes;
    // Structure rendering does not depend on the metric rows; keep this render small.
    next.metric_deltas = [];
    next.structure_delta = { ...next.structure_delta, state: "changed", removed: [first.node_id] };
    const html = render(next);
    expect(html).toContain(`<code>Plan ${first.plan_node_id} · ${first.plan_node_type}`);
    expect(html).toContain("Removed in candidate");
    expect(selfHtml()).toContain(`No structural change: ${complexReport.graph_view.nodes.length} plan nodes matched.`);
  }, COMPLEX_TIMEOUT_MS);

  it("shows the compared Profiles with query id, origin, version and pack", () => {
    const response = connectedComparison();
    const html = render(response.comparison, { baselineOrigin: "connected_profile_fetch", candidateOrigin: "candidate_profile_user_external_rerun", connectedComparison: response });
    expect(html).toContain("Before optimization · baseline");
    expect(html).toContain("After change · candidate");
    expect(html).toContain(response.baseline.query.query_id);
    expect(html).toContain("candidate-query");
    expect(html).toContain("Fetched from the database");
    expect(html).toContain("User-side rerun");
    expect(html).toContain("Same connection");
    expect(html).toContain("Observed only");
    expect(html).toContain(response.validation_context.fetch_order);
    expect(html).toContain(response.validation_context.candidate_attestation);
    const pack = sr3514JsonMinimal.version.selected_pack;
    if (pack) expect(html).toContain(`${pack.pack_id}@${pack.product_pack_version}`);
    expect(html).not.toMatch(/raw profile|profile_text/i);
  });

  it("uses explicit input origins instead of inferring a connection", () => {
    expect(render(comparison())).not.toMatch(/Connected Profile|Imported Profile/);
    expect(occurrences(render(comparison(), { baselineOrigin: "offline_import", candidateOrigin: "offline_import" }), "Imported Profile")).toBe(2);
  });

  it("renders a PlanComparison only when the DTO supplies one", () => {
    expect(render(comparison())).not.toContain("compare-plan-check");
    const html = render(comparison(), { planComparison: sr3514ConnectedContractGolden.plan_comparison });
    expect(html).toContain("compare-plan-check");
    expect(html).toContain("Plan structure only; actual performance needs execution observations.");
  });

  it("localizes closed copy in Chinese and labels familiar metrics", () => {
    const html = renderZh(comparison());
    expect(html).toContain("部分可比");
    expect(html).toContain("拓扑不完整");
    expect(html).toContain("因果归因受限");
    expect(html).toContain('<strong title="QuerySpillBytes">溢写数据量</strong>');
    expect(html).toContain("原始数据 / 溯源");
    expect(metricLabel("FutureMetric", (_en, zh) => zh)).toBe("FutureMetric");
  });
});

describe("CompareWorkspace finding measurements and metric names (v0.6-07)", () => {
  const finding = (id: string, subject: string): AnalysisSnapshotReport["findings"][number] => ({
    finding_id: id, rule_id: "SR3514.BROADCAST_EXCHANGE_BYTES", rule_version: "0.1.0", category: "exchange_volume", severity: "moderate",
    confidence: "medium", confidence_reasons: [], evidence_completeness: "partial", evidence_completeness_reasons: [], subject_ref: subject,
    evidence: [], limitations: [], recommendation: recommendationFixture({ risk: "r", expected_direction: "d", validation_steps: [] }), related_finding_refs: [], rule_trace_ref: "t", measurements: [],
  });

  it("shows each finding's lead rule measurement before and after, as the comparator gave it", () => {
    const next = comparison();
    next.baseline = { outcome: { report: { ...report([], "b"), findings: [finding("f:b", "x"), finding("f:gone", "y")] } } };
    next.candidate = { outcome: { report: { ...report([], "c"), findings: [finding("f:c", "x")] } } };
    next.finding_delta = {
      added: [], resolved: ["SR3514.BROADCAST_EXCHANGE_BYTES:y"], persisting: [], changed: ["SR3514.BROADCAST_EXCHANGE_BYTES:x"], incomparable: [], algorithm_changed: [],
      measurement_changes: [
        { finding_entry: "SR3514.BROADCAST_EXCHANGE_BYTES:x", measurement_id: "broadcast_bytes_sent", baseline_display_text: "576.6 MiB", candidate_display_text: "384.5 MiB", baseline_threshold_text: "> 100 MiB", candidate_threshold_text: "> 100 MiB" },
        { finding_entry: "SR3514.BROADCAST_EXCHANGE_BYTES:x", measurement_id: "node_time_share", baseline_display_text: "21.09%", candidate_display_text: "36.63%", baseline_threshold_text: "> 15%", candidate_threshold_text: "> 30%" },
        { finding_entry: "SR3514.BROADCAST_EXCHANGE_BYTES:y", measurement_id: "broadcast_bytes_sent", baseline_display_text: "120 MiB", candidate_display_text: null, baseline_threshold_text: "> 100 MiB", candidate_threshold_text: null },
      ],
    };
    const html = render(next);
    // The lead measurement on both sides is the row's title; only the lead one per finding.
    expect(html).toContain('title="Broadcast bytes sent 576.6 MiB → 384.5 MiB · rule: &gt; 100 MiB"');
    expect(html).not.toContain("21.09%");
    // A resolved finding has no candidate value: named "absent", never 0.
    expect(html).toContain('title="Broadcast bytes sent 120 MiB → absent · rule: &gt; 100 MiB"');
    expect(html).not.toMatch(/→ 0\b/);
  });

  it("keeps both rule conditions when the two findings fired on different tiers", () => {
    const next = comparison();
    next.baseline = { outcome: { report: { ...report([], "b"), findings: [finding("f:b", "x")] } } };
    next.candidate = { outcome: { report: { ...report([], "c"), findings: [finding("f:c", "x")] } } };
    next.finding_delta = {
      added: [], resolved: [], persisting: [], changed: ["SR3514.BROADCAST_EXCHANGE_BYTES:x"], incomparable: [], algorithm_changed: [],
      measurement_changes: [{ finding_entry: "SR3514.BROADCAST_EXCHANGE_BYTES:x", measurement_id: "node_time_share", baseline_display_text: "21.09%", candidate_display_text: "36.63%", baseline_threshold_text: "> 15%", candidate_threshold_text: "> 30%" }],
    };
    const html = render(next);
    expect(html).toContain("Share of operator time 21.09% → 36.63% · rule: &gt; 15% → &gt; 30%");
  });

  it("names every counter of the 3.5.14 metric registry instead of printing its id", () => {
    const ids = versionPack.metric_registry.metrics.map((metric) => metric.metric_id);
    expect(ids).toContain("FrontendProfileMergeTime");
    for (const id of ids) {
      expect(metricLabel(id, (en) => en)).not.toBe(id);
      expect(metricLabel(id, (_en, zh) => zh)).not.toBe(id);
    }
    const next = comparison();
    next.baseline = { outcome: { report: report([cell("b", "QueryAllocatedMemoryUsage", "100", "byte")], "b") } };
    next.candidate = { outcome: { report: report([cell("c", "QueryAllocatedMemoryUsage", "200", "byte")], "c") } };
    next.metric_deltas = [delta("QueryAllocatedMemoryUsage", "b", "c", { absolute_delta: int("100"), percentage_value: int("1") })];
    expect(render(next)).toContain('<strong title="QueryAllocatedMemoryUsage">Memory allocated (cumulative)</strong>');
  });
});

describe("CompareWorkspace board 04 (v0.7-04)", () => {
  it("shows a changed node's differing plan facts on both sides, from the backend's changed list", () => {
    const next = complexSelfComparison();
    next.metric_deltas = [];
    const [first] = complexReport.graph_view.nodes;
    const candidate: AnalysisSnapshotReport = { ...complexReport, graph_view: { ...complexReport.graph_view, nodes: complexReport.graph_view.nodes.map((node) => node === first ? { ...node, sql_facts: [...(node.sql_facts ?? []), { kind: "predicate" as const, value: "order_date >= '2026-10-01'", source_refs: [] }] } : node) } };
    next.candidate = { outcome: { report: candidate } };
    next.structure_delta = { ...next.structure_delta, state: "changed", changed: [first.node_id] };
    const html = render(next);
    expect(html).toContain("<code>PREDICATES: —</code>");
    expect(html).toContain("<code>PREDICATES: order_date &gt;= &#x27;2026-10-01&#x27;</code>");
    expect(html).toContain("Predicate changed · explicit");
  });

  it("lists a node the backend left unconnected as not compared, without inventing a relation", () => {
    const next = comparison();
    const base = next.baseline as { outcome: { report: AnalysisSnapshotReport } };
    base.outcome.report = { ...base.outcome.report, graph_view: { ...base.outcome.report.graph_view, unconnected_node_ids: ["orphan"] } };
    const html = render(next);
    expect(html).toContain("<code>orphan</code>");
    expect(html).toContain("Relation unproven");
    expect(html).toMatch(/<tr class="muted">/);
  });

  it("waits for a candidate once a baseline is set, and says how to pick one", () => {
    const query = sr3514ConnectedContractGolden.query_response.queries[0];
    const html = renderToStaticMarkup(<PendingComparison baseline={{ query, report: sr3514JsonMinimal }} onClear={() => undefined} onPickCandidate={() => undefined} />);
    expect(html).toContain("Before optimization · baseline");
    expect(html).toContain("Fetched from the database");
    expect(html).toContain("Compare with baseline");
    expect(html).toContain(">Pick the candidate</button>");
    expect(html).toContain(">Clear baseline</button>");
  });

  it("colours the change by direction only and states it in the legend", () => {
    const next = comparison();
    next.baseline = { outcome: { report: report([cell("b", "QueryTotalTime", "27800000000", "ns")], "b") } };
    next.candidate = { outcome: { report: report([cell("c", "QueryTotalTime", "9400000000", "ns")], "c") } };
    next.metric_deltas = [delta("QueryTotalTime", "b", "c", { absolute_delta: int("-18400000000"), percentage_value: { kind: "decimal", value: "-0.661870503", scale: 9 } })];
    const html = render(next);
    expect(html).toContain("Green = down · orange = up · % relative to baseline");
    expect(html).toContain('<span class="compare-change down">−18.4 s</span>');
    expect(html).toContain('title="−66.1870503%">−66.2%</span>');
    expect(html).toContain('class="compare-bar-fill down"');
  });
});
