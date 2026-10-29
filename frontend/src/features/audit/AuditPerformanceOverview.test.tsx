import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type {
  AuditMetricFact,
  AuditMetricId,
  AuditPerformanceAggregate,
  AuditPerformanceBucket,
  AuditPerformanceResponse,
  AuditPerformanceSummary,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { AuditPerformanceView, type AuditPerformanceOverviewProps } from "./AuditPerformanceOverview";
import source from "./AuditPerformanceOverview.tsx?raw";

const english: LocalizedText = (en) => en;
const chinese: LocalizedText = (_en, zh) => zh;

function fact(metric_id: AuditMetricId, value: string | null): AuditMetricFact {
  const unit = metric_id === "scan_bytes" || metric_id === "memory_cost" ? "byte" : "ns";
  const scale = metric_id === "cpu_cost" ? 1n : 1_000_000n;
  if (value === null) return { metric_id, input: { state: "sql_null" }, state: "missing", parser_state: "missing", canonical_number: null, canonical_unit: null };
  const canonical = unit === "byte" ? value : (BigInt(value) * scale).toString();
  return { metric_id, input: { state: "text", value }, state: "exact", parser_state: "exact", canonical_number: { kind: "integer", value: canonical, scale: null }, canonical_unit: unit };
}

function aggregate(metric_id: AuditMetricId, value: string | null, excluded = "0", aggregation: "sum" | "max" = "sum"): AuditPerformanceAggregate {
  return { aggregate: { aggregation, metric: fact(metric_id, value), source_nonnegative_count: "0" }, excluded_count: excluded };
}

function summary(executions: string, errors: string, scan: string | null, scanExcluded = "0"): AuditPerformanceSummary {
  const empty = executions === "0";
  return {
    execution_count: executions,
    error_count: errors,
    error_share_permyriad: empty ? null : errors === "0" ? 0 : 5000,
    query_time: { eligible_count: executions, p50: empty ? null : fact("query_time", "100"), p95: empty ? null : fact("query_time", "2500"), p99: empty ? null : fact("query_time", "2500") },
    query_time_sum: aggregate("query_time", empty ? null : "2600"),
    scan_bytes_sum: aggregate("scan_bytes", scan, scanExcluded),
    cpu_cost_sum: aggregate("cpu_cost", empty ? null : "21000"),
    memory_cost_max: aggregate("memory_cost", empty ? null : "4096", "0", "max"),
    pending_time: { state: "unavailable", reason: "source_column_absent" },
  };
}

function bucket(index: number, value: AuditPerformanceSummary, scan: number | null, recent = false): AuditPerformanceBucket {
  return {
    start_milliseconds: String((1_800_000_000 + index * 900) * 1000),
    end_milliseconds: String((1_800_000_000 + (index + 1) * 900) * 1000),
    x_permyriad: index * 3333,
    width_permyriad: 3333,
    summary: value,
    projection: { execution_permyriad: value.execution_count === "0" ? 0 : 10000, error_permyriad: value.error_count === "0" ? 0 : 5000, query_time_p50_permyriad: null, query_time_p95_permyriad: null, query_time_p99_permyriad: null, scan_bytes_permyriad: scan, cpu_cost_permyriad: null, memory_cost_permyriad: null, pending_time_p95_permyriad: null },
    recent_ingestion: recent,
  };
}

const response: AuditPerformanceResponse = {
  schema_version: "audit-performance.v1",
  connection_id: "c1",
  scope: { window: { start_seconds: 1_800_000_000, end_seconds: 1_800_002_700 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" },
  bucket_seconds: 900,
  read_at_milliseconds: "1800002700000",
  window: summary("3", "1", "1300", "1"),
  buckets: [bucket(0, summary("2", "0", "1300"), 10000), bucket(1, summary("0", "0", null), null), bucket(2, summary("1", "1", null, "1"), null, true)],
  axes: { time_ticks: [{ at_milliseconds: "1800000000000", x_permyriad: 0, label: "08:00" }], execution: [{ y_permyriad: 0, value: { kind: "count", count: "0" } }, { y_permyriad: 10000, value: { kind: "count", count: "2" } }], query_time_log: [], scan_bytes: [], cpu_cost: [], memory_cost: [], pending_time: [] },
  latency_histogram: {
    eligible_count: "3",
    exclusions: { sql_null_count: "0", negative_count: "0", overflow_count: "0" },
    buckets: [{ bucket: { bucket_id: "100_1000ms", lower_ms_inclusive: "100", upper_ms_exclusive: "1000", display_label_seconds: "100 ms–1 s", count: "2" }, share_permyriad: 6666, width_permyriad: 10000 }, { bucket: { bucket_id: "1_10s", lower_ms_inclusive: "1000", upper_ms_exclusive: "10000", display_label_seconds: "1–10 s", count: "1" }, share_permyriad: 3333, width_permyriad: 5000 }],
    slow: [{ threshold_ms: "1000", count: "1", share_permyriad: 3333 }, { threshold_ms: "10000", count: "0", share_permyriad: 0 }],
  },
  errors: { state: "available", error_count: "1", rows: [{ code: { state: "sql_null" }, count: "1", share_permyriad: 10000, last_event_milliseconds: "1800001900000" }], remainder_count: "0", remainder_code_count: "0" },
  dimensions: [
    { dimension: "user", state: { state: "available", distinct_value_count: "3", rows: [
      { value: { state: "text", value: "alice" }, execution_count: "2", error_count: "0", query_time_sum: aggregate("query_time", "200") },
      { value: { state: "conflicting" }, execution_count: "1", error_count: "1", query_time_sum: aggregate("query_time", "2500") },
    ], rankings: [
      { key: "query_time_sum", entries: [{ row_index: 1, share_permyriad: 9259, width_permyriad: 10000 }, { row_index: 0, share_permyriad: 740, width_permyriad: 800 }], remainder_value_count: "1", remainder_share_permyriad: 1 },
      { key: "execution_count", entries: [{ row_index: 0, share_permyriad: 6666, width_permyriad: 10000 }], remainder_value_count: "2", remainder_share_permyriad: 3333 },
      { key: "error_count", entries: [{ row_index: 1, share_permyriad: 10000, width_permyriad: 10000 }], remainder_value_count: "2", remainder_share_permyriad: 0 },
    ] } },
    { dimension: "database", state: { state: "unavailable", reason: "source_column_absent" } },
    { dimension: "resource_group", state: { state: "available", distinct_value_count: "0", rows: [], rankings: [] } },
    { dimension: "catalog", state: { state: "available", distinct_value_count: "0", rows: [], rankings: [] } },
  ],
  cluster_identity: { state: "session_scoped", connection_id: "c1", detected_version: "3.5.14" },
  audit_timestamp_time_zone: null,
  limitations: ["audit_loader_ingestion_async_completeness_unknown", "bucket_quantiles_are_never_combined_across_buckets"],
};

function render(text: LocalizedText, filterableUser = true): string {
  const props: AuditPerformanceOverviewProps = {
    api: { readAuditPerformance: () => Promise.resolve(response) },
    connectionId: "c1",
    scope: response.scope,
    concurrency: null,
    coverage: null,
    families: [],
    familySortLabel: "cumulative",
    describeFailure: () => "failed",
    onDrill: () => undefined,
    onFilter: () => undefined,
    onOpenFamilies: () => undefined,
    filterableUser,
    text,
  };
  return renderToStaticMarkup(<AuditPerformanceView {...props} bucket="auto" onBucket={() => undefined} response={response} />);
}

describe("AuditPerformanceView", () => {
  it("renders the cards, chart, histogram, failures and composition from server facts", () => {
    const html = render(english);
    expect(html).toContain("Executions");
    expect(html).toContain("50.0%");
    expect(html).toContain("100 ms / 2.5 s / 2.5 s");
    expect(html).toContain("1 records lack scanBytes, not counted");
    expect(html).toContain("Volume and failures · every 15 min");
    expect(html).toContain("100 ms–1 s");
    expect(html).toContain("(empty)");
    expect(html).toContain("(duplicate rows disagree)");
    expect(html).toContain("1 other values");
    // The latest bucket with executions is selected and flagged as still arriving.
    expect(html).toContain("this bucket may still be filling");
    expect(html).toContain("The audit table has no pendingTimeMs");
  });

  it("keeps an empty bucket an exact zero and never offers to filter on a non-text value", () => {
    const html = render(english);
    // The empty middle bucket is an exact zero: a zero-height bar, not a gap marker.
    expect(html).toContain('class="ok" height="0"');
    expect(html).not.toContain('class="gap"');
    // alice is filterable; the conflicting row is not.
    expect(html.match(/>Filter</g)?.length).toBe(1);
    expect(render(english, false).match(/>Filter</g)).toBeNull();
  });

  it("speaks Chinese and never derives numbers from counters in the browser", () => {
    const html = render(chinese);
    expect(html).toContain("耗时分布");
    expect(html).toContain("失败拆分");
    expect(html).toContain("负载构成");
    expect(source).not.toMatch(/Number\((?:[a-z]+\.)*(?:count|execution_count|error_count|sql_null_count)\)/);
    expect(source).not.toContain("innerHTML");
  });
});
