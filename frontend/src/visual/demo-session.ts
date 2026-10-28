// Demo cluster session and the Main board's audit data (v0.7 pixel parity).
// Values are chosen so the product's own display policy renders the same text
// the design shows; they exist only in this development harness.
import type {
  AuditFamilyPresentation,
  AuditMetricFact,
  AuditMetricId,
  AuditOccurrencePresentation,
  AuditOccurrenceResponse,
  AuditOverviewResponse,
  AuditScope,
  ConnectedQueryRef,
  ConnectionSessionView,
} from "../../../contracts/generated/doctor-contracts";

export const DEMO_CONNECTION_ID = "demo-mock-cluster";

export const DEMO_SESSION: ConnectionSessionView = {
  connection_id: DEMO_CONNECTION_ID,
  state: "ready",
  tls_mode: "docker_loopback_plaintext",
  idle_timeout_seconds: 900,
  detected_version: { value: "3.5.14", source_ref: "demo:version" },
  time_zone: { session_time_zone: "SYSTEM", system_time_zone: "Etc/UTC", effective_time_zone: "Etc/UTC", current_utc_offset_seconds: 0, source_ref: "demo:tz" },
  capabilities: { version_probe: "available", time_zone: "available", audit: "available", audit_user_filter: "available", audit_statement_filter: "available", profile_list: "available", profile_fetch: "available", explain_costs: "available", plan_advisor: "available" },
  limitations: [],
};

const SECOND = 1_000_000_000n;
const TB = 1024n ** 4n;
const GB = 1024n ** 3n;
const ns = (seconds: number) => (BigInt(Math.round(seconds * 1000)) * SECOND / 1000n).toString();
const unit: Record<AuditMetricId, string> = { query_time: "ns", pending_time: "ns", cpu_cost: "ns", scan_bytes: "byte", memory_cost: "byte", scan_rows: "item", return_rows: "item" };

function exact(metric_id: AuditMetricId, value: string): AuditMetricFact {
  return { metric_id, input: { state: "text", value }, state: "exact", parser_state: "exact", canonical_number: { kind: "integer", value, scale: null }, canonical_unit: unit[metric_id] };
}
function missing(metric_id: AuditMetricId): AuditMetricFact {
  return { metric_id, input: { state: "sql_null" }, state: "missing", parser_state: "missing", canonical_number: null, canonical_unit: null };
}

type FamilySeed = { digest: string; sql: string; count: string; total: number; max: number; scan: bigint | null; fail: string; share: number };
const FAMILIES: FamilySeed[] = [
  { digest: "0f1e2d3c4b5a69788796a5b4c3d2e1f0", sql: "SELECT customer_id,\n       SUM(amount)\nFROM orders\nGROUP BY customer_id;", count: "412", total: 4080, max: 27.8, scan: 3408486046106n, fail: "0", share: 10000 },
  { digest: "9a1b2c3d4e5f60718293a4b5c6d7e8f9", sql: "SELECT o.*, c.region FROM orders o JOIN customers c ON o.cid = c.id WHERE …", count: "188", total: 2472, max: 12.4, scan: 1319413953331n, fail: "2", share: 6100 },
  { digest: "c4d5e6f708192a3b4c5d6e7f8091a2b3", sql: "SELECT dt, COUNT(DISTINCT uid) FROM events WHERE dt >= ? GROUP BY dt", count: "96", total: 1120, max: 9.2, scan: 640n * GB, fail: "0", share: 2700 },
  { digest: "d8e9f0a1b2c3d4e5f60718293a4b5c6d", sql: "WITH t AS (SELECT l_orderkey, SUM(l_qty) FROM lineitem …) SELECT …", count: "41", total: 665, max: 31, scan: null, fail: "1", share: 1600 },
  { digest: "7b8c9d0e1f2a3b4c5d6e7f8091a2b3c4", sql: "SELECT region, AVG(price) FROM sales WHERE ts BETWEEN ? AND ? GROUP BY region", count: "63", total: 411, max: 7.9, scan: 210n * GB, fail: "0", share: 1000 },
  { digest: "f1e2d3c4b5a69788796a5b4c3d2e1f00", sql: "SELECT * FROM dim_store WHERE store_id = ?", count: "547", total: 136, max: 0.8, scan: 12n * GB, fail: "0", share: 300 },
];

function family(seed: FamilySeed): AuditFamilyPresentation {
  const scan = seed.scan === null ? missing("scan_bytes") : exact("scan_bytes", seed.scan.toString());
  return {
    membership: { state: "exact_native_digest", family_key: `audit-family-v1:${seed.digest}`, native_digest: seed.digest, context: { cluster: `session:${DEMO_CONNECTION_ID}`, engine_version: "3.5.14", catalog: "default_catalog", database: "demo" } },
    execution_count: seed.count,
    error_count: seed.fail,
    error_proportion: { state: "exact", numerator: seed.fail, denominator: seed.count },
    aggregates: [
      { aggregation: "sum", metric: exact("query_time", ns(seed.total)), source_nonnegative_count: seed.count },
      { aggregation: "sum", metric: scan, source_nonnegative_count: seed.scan === null ? "0" : seed.count },
      { aggregation: "sum", metric: exact("cpu_cost", ns(seed.total * 0.8)), source_nonnegative_count: seed.count },
      { aggregation: "max", metric: exact("memory_cost", (4n * GB).toString()), source_nonnegative_count: seed.count },
      { aggregation: "max", metric: exact("query_time", ns(seed.max)), source_nonnegative_count: seed.count },
    ],
    statement_sample: { state: "text", value: seed.sql, truncated: false },
  };
}

/** 13:05–14:05 UTC on 2026-10-21; 30 two-minute steps shaped like the design's bars. */
const WINDOW_START = Date.UTC(2026, 9, 21, 13, 5) / 1000;
const HEIGHTS = [22, 30, 28, 35, 41, 38, 52, 47, 44, 58, 63, 55, 49, 61, 72, 68, 80, 74, 66, 59, 62, 70, 77, 85, 92, 88, 79, 71, 64, 69];

export function mainOverview(scope: AuditScope): AuditOverviewResponse {
  const rows = FAMILIES.map(family);
  const start = scope.window.start_seconds * 1000;
  const end = scope.window.end_seconds * 1000;
  const step = (end - start) / HEIGHTS.length;
  const clock = (ms: number) => new Date((WINDOW_START * 1000) + (ms - start)).toISOString().slice(11, 19);
  return {
    schema_version: "audit-workload.v1", connection_id: DEMO_CONNECTION_ID, discovery_generation: 3, scope,
    cluster_identity: { state: "session_scoped", connection_id: DEMO_CONNECTION_ID, detected_version: "3.5.14" },
    audit_timestamp_time_zone: DEMO_SESSION.time_zone,
    primary: { state: "families", rows, top: { requested_limit: 100, returned_count: rows.length, full_window_count: "6", truncated: false } },
    coverage: {
      raw_row_count: "1331", invalid_query_id_count: "0", exact_duplicate_row_count: "0", conflicting_duplicate_row_count: "0", retained_event_count: "1284", retained_family_count: "6",
      error_count: "12", error_proportion: { state: "exact", numerator: "12", denominator: "1284" }, missing_digest_count: "0", missing_context_count: "0",
      statement_filter: { state: "applied", hidden_non_query_row_count: "41", hidden_doctor_row_count: "6" },
      metric_inputs: [
        { metric_id: "query_time", sql_null_count: "0", text_present_count: "1284", nonnegative_count: "1284", negative_count: "0", aggregate: { aggregation: "sum", metric: exact("query_time", ns(11520)), source_nonnegative_count: "1284" } },
        { metric_id: "cpu_cost", sql_null_count: "0", text_present_count: "1284", nonnegative_count: "1284", negative_count: "0", aggregate: { aggregation: "sum", metric: exact("cpu_cost", ns(10020)), source_nonnegative_count: "1284" } },
        { metric_id: "scan_bytes", sql_null_count: "3", text_present_count: "1281", nonnegative_count: "1281", negative_count: "0", aggregate: { aggregation: "sum", metric: exact("scan_bytes", (56n * TB / 10n).toString()), source_nonnegative_count: "1281" } },
      ],
    },
    concurrency: {
      window: { start_milliseconds: String(start), end_milliseconds: String(end) },
      axis: { time_ticks: [{ at_milliseconds: String(start), x_permyriad: 0, label: clock(start) }, { at_milliseconds: String(end), x_permyriad: 10000, label: clock(end) }] },
      source_coverage: { state: "partial", reason: "bounded_lookback", source_start_seconds: scope.window.start_seconds - 3600, source_end_seconds: scope.window.end_seconds, lookback_seconds: 3600 },
      coverage: null,
      series: {
        state: "exact", maximum_active_count: "23",
        count_ticks: [{ active_count: "0", y_permyriad: 0 }, { active_count: "23", y_permyriad: 10000 }],
        points: HEIGHTS.map((height, index) => {
          const at = Math.round(start + index * step);
          return { at_milliseconds: String(at), active_count: String(Math.round(height / 4)), x_permyriad: Math.round(index * 10000 / HEIGHTS.length), y_permyriad: height * 100, label: clock(at) };
        }),
      },
    },
    family_top: {
      sort: "cumulative_query_time_desc", projection_scope: "returned_top_rows",
      rows: rows.map((row, index) => ({ family: row, plot: { state: "exact", permyriad: FAMILIES[index].share } })),
      top: { requested_limit: 10, returned_count: rows.length, full_window_count: "6", truncated: false },
    },
    limitations: ["audit_async_ingestion"],
  };
}

type MemberSeed = { id: string; time: number; clock: string; rows: string | null; fetch: "available" | "fetched" | "unavailable" };
const MEMBERS: MemberSeed[] = [
  { id: "7e8c3a91-0b2f-4d73-9a1e-5c6b7d8e4d73", time: 27.8, clock: "14:02:17", rows: "1102223445", fetch: "available" },
  { id: "9a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b6789", time: 9.2, clock: "13:54:08", rows: "498800000", fetch: "fetched" },
  { id: "c4d5e6f7-0819-4a2b-9c3d-4e5f6a7bb1c0", time: 6.7, clock: "13:43:56", rows: "153200000", fetch: "unavailable" },
  { id: "d8e9f0a1-b2c3-4d4e-8f50-617283944d5e", time: 6, clock: "13:38:44", rows: null, fetch: "available" },
];

export function mainMembers(scope: AuditScope): AuditOccurrenceResponse {
  const sql = FAMILIES.find((family) => family.digest === scope.native_digest)?.sql;
  const rows = MEMBERS.map((seed, index): AuditOccurrencePresentation => {
    const query: ConnectedQueryRef = {
      connection_id: DEMO_CONNECTION_ID, query_id: seed.id, discovery_source: "audit", discovery_generation: 4,
      observed_at_utc: `2026-10-21T${seed.clock}Z`, observed_at_local: `2026-10-21 ${seed.clock}`, timestamp_state: "reader_session_interpreted",
      status: "finished", digest: scope.native_digest, database: "demo", resource_group: "default_wg", query_time_ms: Math.round(seed.time * 1000),
      profile_fetch: seed.fetch === "unavailable" ? { state: "unavailable", reason: "query_not_finished" } : { state: "available", reason: seed.fetch === "fetched" ? "profile_fetched" : "can_be_attempted" },
      plan_check: { state: "available", reason: "statement_retained", plan_sql_ref: `audit:plan:${index}` },
      source_refs: [`audit:row:${index}`],
    };
    return {
      query, catalog: "default_catalog",
      metrics: [exact("query_time", ns(seed.time)), seed.rows === null ? missing("scan_rows") : exact("scan_rows", seed.rows)],
      error_code: { state: "sql_null" },
      family: { state: "exact_native_digest", family_key: `audit-family-v1:${scope.native_digest}`, native_digest: scope.native_digest ?? "", context: { cluster: `session:${DEMO_CONNECTION_ID}`, engine_version: "3.5.14", catalog: "default_catalog", database: "demo" } },
      statement_sample: sql === undefined ? { state: "unavailable", reason: "not_read" } : { state: "text", value: sql, truncated: false },
    };
  });
  const overview = mainOverview(scope);
  return {
    schema_version: "audit-workload.v1", connection_id: DEMO_CONNECTION_ID, discovery_generation: 4, scope,
    cluster_identity: overview.cluster_identity, audit_timestamp_time_zone: DEMO_SESSION.time_zone, rows,
    coverage: { ...overview.coverage, retained_event_count: "412", retained_family_count: "1" },
    top: { requested_limit: 50, returned_count: rows.length, full_window_count: "412", truncated: true },
    limitations: [],
  };
}
