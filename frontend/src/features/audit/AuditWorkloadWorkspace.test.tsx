import { auditStatusLabel, familyMemberSqlRequest, sessionEndReason } from "./AuditWorkloadWorkspace";
import { renderToStaticMarkup } from "react-dom/server";
import { Children, isValidElement, type ReactNode } from "react";
import workspaceSource from "./AuditWorkloadWorkspace.tsx?raw";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { AuditActiveAtResponse, AuditFamilyDetailResponse, AuditFamilyPresentation, AuditFamilyResponse, AuditMetricFact, AuditMetricId, AuditOccurrenceResponse, AuditScope, AuditStatementSample } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider, type LocalizedText } from "../../i18n/locale";
import { AuditFamilyDetail } from "./AuditFamilyDetail";
import { AuditFamilyList, AuditKpiStrip } from "./AuditOverviewPanels";
import { FamilySummary, OccurrenceStatementCell, SelectedAuditOccurrence } from "./AuditWorkloadWorkspace";
import { errorProportionText } from "./audit-display";
import { AuditOccurrenceTable, AuditWindowEvidence, AuditWorkloadWorkspace, auditErrorLabel, profileFetchFailureText, auditFamilyDetailDiscoveryGeneration, auditFamilyDetailRequest, auditFamilyDetailSqlRequest, auditFamilyMemberBrowseState, auditModeSwitchScopes, auditParentScopeAfterFamilyEdits, auditProfileForSelectedOccurrence, auditReportForSelectedOccurrence, auditResponseForConnection, auditScopeForFamily, auditScopeForWindowPreset, auditTerminalStateFromControl, auditStatementFilterNote, auditUserFilterNote, defaultAuditScope, familyDetailRepresentativeRows, familyRowKey, isAuditFamilyDetailBound, isAuditRequestCurrentForConnection, isCurrentAuditRequest, metricText, occurrenceRowKey, visibleSql } from "./AuditWorkloadWorkspace";

const english = (value: string): string => value;
const chinese = (_english: string, value: string): string => value;

function returnButton(node: ReactNode): ((event: unknown) => void) | undefined {
  for (const child of Children.toArray(node)) {
    if (!isValidElement<{ children?: ReactNode; onClick?: (event: unknown) => void }>(child)) continue;
    if (child.type === "button" && child.props.children === "Return to parent window") return child.props.onClick;
    const found = returnButton(child.props.children);
    if (found) return found;
  }
}
const scope: AuditScope = { window: { start_seconds: 1_700_000_000, end_seconds: 1_700_003_600 }, state: "finished", user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "exclude_doctor_and_non_query" };
const connection = sr3514ConnectedContractGolden.connection;
const baseQuery = { ...sr3514ConnectedContractGolden.query_response.queries[0], connection_id: connection.connection_id, discovery_generation: 7, query_id: "audit-query-1", source_refs: ["audit:row:1"] };

function fact(metric_id: AuditMetricId, state: AuditMetricFact["state"], value: string | null, unit: string | null): AuditMetricFact {
  return { metric_id, input: value === null ? { state: "sql_null" } : { state: "text", value }, state, parser_state: state === "exact" ? "exact" : state === "overflow" ? "overflow" : state === "missing" ? "missing" : null, canonical_number: state === "exact" || state === "precision_unsafe" ? { kind: "integer", value: value!, scale: null } : null, canonical_unit: state === "exact" || state === "precision_unsafe" ? unit : null };
}

const queryTime = fact("query_time", "exact", "0", "ns");
const cumulativeQueryTime = fact("query_time", "exact", "7654321", "ns");
const unsafeBytes = fact("scan_bytes", "precision_unsafe", "9007199254740993", "byte");
const negativeRows = fact("scan_rows", "negative_sentinel", "-1", null);
const rows = fact("scan_rows", "exact", "42", "item");
const nullCpu = fact("cpu_cost", "missing", null, null);
const memory = fact("memory_cost", "exact", "1048576", "byte");

const occurrenceResponse: AuditOccurrenceResponse = {
  schema_version: "audit-workload.v1", connection_id: connection.connection_id, discovery_generation: 7, scope,
  cluster_identity: { state: "session_scoped", connection_id: connection.connection_id, detected_version: "3.5.14" }, audit_timestamp_time_zone: connection.time_zone,
  rows: [
    { query: baseQuery, catalog: "default_catalog", error_code: { state: "unavailable", reason: "source_column_absent" }, metrics: [queryTime, unsafeBytes, negativeRows, nullCpu, memory], family: { state: "exact_native_digest", family_key: "audit-family-v1:a", native_digest: "aabb", context: { cluster: "session", engine_version: "3.5.14", catalog: "default_catalog", database: "demo" } }, statement_sample: { state: "text", value: "SELECT customer_id,\n  SUM(amount)\nFROM orders", truncated: true } },
    { query: { ...baseQuery, query_id: "audit-query-2", source_refs: ["audit:row:2"] }, catalog: "other_catalog", error_code: { state: "unavailable", reason: "source_column_absent" }, metrics: [queryTime, unsafeBytes, rows, nullCpu, memory], family: { state: "exact_native_digest", family_key: "audit-family-v1:b", native_digest: "aabb", context: { cluster: "session", engine_version: "3.5.14", catalog: "other_catalog", database: "other" } }, statement_sample: { state: "sql_null" } },
  ],
  coverage: { raw_row_count: "1004", invalid_query_id_count: "1", exact_duplicate_row_count: "2", conflicting_duplicate_row_count: "3", retained_event_count: "1000", error_count: "17", error_proportion: { state: "exact", numerator: "17", denominator: "1000" }, retained_family_count: "2", missing_digest_count: "4", missing_context_count: "5", statement_filter: { state: "applied", hidden_non_query_row_count: "41", hidden_doctor_row_count: "9" }, metric_inputs: [{ metric_id: "query_time", sql_null_count: "6", text_present_count: "994", nonnegative_count: "992", negative_count: "2", aggregate: { aggregation: "sum", metric: cumulativeQueryTime, source_nonnegative_count: "992" } }, { metric_id: "scan_bytes", sql_null_count: "6", text_present_count: "994", nonnegative_count: "992", negative_count: "2", aggregate: { aggregation: "sum", metric: unsafeBytes, source_nonnegative_count: "992" } }] },
  top: { requested_limit: 50, returned_count: 2, full_window_count: "1000", truncated: true }, limitations: ["audit_async_ingestion"],
};

const familyRows: AuditFamilyPresentation[] = occurrenceResponse.rows.map((row) => {
  if (row.family.state !== "exact_native_digest") throw new Error("fixture requires an exact family");
  const database = row.family.context.database;
  return {
    membership: row.family,
    execution_count: database === "demo" ? "900" : "100",
    error_count: "0",
    error_proportion: { state: "exact", numerator: "0", denominator: database === "demo" ? "900" : "100" },
    aggregates: [
      { aggregation: "sum", metric: fact("query_time", "exact", database === "demo" ? "123456" : "12", "ns"), source_nonnegative_count: "1" },
      { aggregation: "sum", metric: unsafeBytes, source_nonnegative_count: "1" },
    ],
    statement_sample: { state: "unavailable", reason: "not_read" },
  };
});

const familyResponse: AuditFamilyResponse = { ...occurrenceResponse, rows: familyRows, top: { requested_limit: 50, returned_count: 2, full_window_count: "2", truncated: false } };
const familyDetail: AuditFamilyDetailResponse = {
  schema_version: "audit-workload.v1",
  connection_id: connection.connection_id,
  discovery_generation: 8,
  scope: auditScopeForFamily(scope, familyRows[0])!,
  family: familyRows[0].membership,
  distribution: {
    state: "exact",
    algorithm: "audit-query-time-histogram.v1",
    population_count: "597",
    eligible_count: "597",
    exclusions: { sql_null_count: "0", negative_count: "0", overflow_count: "0" },
    buckets: [
      { bucket_id: "lt_10ms", lower_ms_inclusive: "0", upper_ms_exclusive: "10", display_label_seconds: "< 10 ms", count: "596" },
      { bucket_id: "10_100ms", lower_ms_inclusive: "10", upper_ms_exclusive: "100", display_label_seconds: "10–100 ms", count: "0" },
      { bucket_id: "100_1000ms", lower_ms_inclusive: "100", upper_ms_exclusive: "1000", display_label_seconds: "100 ms–1 s", count: "0" },
      { bucket_id: "1_10s", lower_ms_inclusive: "1000", upper_ms_exclusive: "10000", display_label_seconds: "1–10 s", count: "0" },
      { bucket_id: "10_60s", lower_ms_inclusive: "10000", upper_ms_exclusive: "60000", display_label_seconds: "10–60 s", count: "0" },
      { bucket_id: "1_5m", lower_ms_inclusive: "60000", upper_ms_exclusive: "300000", display_label_seconds: "1–5 min", count: "0" },
      { bucket_id: "5_60m", lower_ms_inclusive: "300000", upper_ms_exclusive: "3600000", display_label_seconds: "5–60 min", count: "1" },
      { bucket_id: "gte_60m", lower_ms_inclusive: "3600000", upper_ms_exclusive: "9223372036855", display_label_seconds: "≥ 60 min", count: "0" },
    ],
  },
  representatives: {
    recent: { state: "selected", occurrence: occurrenceResponse.rows[0], profile_evidence: null },
    slowest: { state: "selected", occurrence: occurrenceResponse.rows[1], profile_evidence: null },
    profile_known: { state: "unavailable", reason: "no_match_in_bounded_profile_evidence" },
  },
  profile_evidence_coverage: { profile_list_check: "checked", profile_list_limit: 500, profile_list_returned: 2, profile_list_at_limit: false, profile_list_excluded: 0, successful_cache_retained: 0, successful_cache_capacity: 500, successful_cache_evicted: false, candidate_count: 2, candidate_limit: 500, candidate_truncated: false },
  cluster_identity: { state: "session_scoped", connection_id: connection.connection_id, detected_version: "3.5.14" },
  audit_timestamp_time_zone: connection.time_zone,
  limitations: ["audit_non_atomic_read"],
};

describe("AuditWorkloadWorkspace", () => {
  function selectedMarkup(pending: AuditMetricFact | null, error_code: AuditOccurrenceResponse["rows"][number]["error_code"]): string {
    const occurrence = { ...occurrenceResponse.rows[0], error_code, metrics: pending ? [pending] : [] };
    return renderToStaticMarkup(<SelectedAuditOccurrence occurrence={occurrence} doctorProfile={null} onDiagnose={() => undefined} onViewSql={() => undefined} profile={null} profileLoading={false} sql={null} sqlError={null} sqlLoading={false} text={english} />);
  }

  it("shows optional facts from the DTO with adaptive durations and exact source text, without loading SQL", () => {
    const pending = { ...fact("pending_time", "exact", "125000000", "ns"), input: { state: "text", value: "125" } as const };
    const html = selectedMarkup(pending, { state: "text", value: "" });
    expect(html).toContain('<details class="audit-optional-facts"><summary>Raw audit details</summary>');
    expect(html).toContain("125 ms · exact");
    expect(html).toContain("pendingTimeMs=&quot;125&quot; → 125000000 ns");
    expect(html).toContain("<dd>&quot;&quot;</dd>");
    expect(html).toContain("empty errorCode does not mean success");
    expect(html).toContain("pending time alone does not prove blocking");
    expect(html).not.toContain('<pre');
  });

  it.each(["source_column_absent", "conflicting_source_rows"] as const)("preserves optional unavailable reason %s without losing the selected event", (reason) => {
    const pending: AuditMetricFact = { ...fact("pending_time", "unavailable", null, null), input: { state: "unavailable", reason } };
    const html = selectedMarkup(pending, { state: "unavailable", reason });
    expect(html).toContain(`Source: unavailable · ${reason}</dd>`);
    expect(html).toContain(`<dd>unavailable · ${reason}</dd>`);
    expect(html).toContain("audit-query-1");
    expect(html).not.toContain("0 ns");
  });

  it("keeps SQL NULL, absent DTO, negative sentinel and true zero separate", () => {
    const missing = selectedMarkup(fact("pending_time", "missing", null, null), { state: "sql_null" });
    expect(missing).toContain("Source: sql_null</dd>");
    expect(missing).toContain("<dd>sql_null</dd>");
    expect(missing).toContain("— (missing)");
    expect(selectedMarkup(null, { state: "sql_null" })).toContain("Source: not_returned</dd>");
    const negative = selectedMarkup(fact("pending_time", "negative_sentinel", "-1", null), { state: "text", value: "0" });
    expect(negative).toContain("-1 (negative_sentinel)");
    expect(negative).not.toContain("0 ns");
    expect(selectedMarkup(fact("pending_time", "exact", "0", "ns"), { state: "text", value: "0" })).toContain("0 ns · exact");
  });

  it("escapes optional raw text and preserves parser failure instead of deriving a number", () => {
    const html = selectedMarkup(fact("pending_time", "invalid", "12ms<bad>", null), { state: "text", value: '<script>alert("x")</script>\n' });
    expect(html).toContain("12ms&lt;bad&gt; (invalid)");
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("0 ns");
  });

  it("offers error proportion and error count as distinct server ranking dimensions", () => {
    const html = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={connection} />);
    expect(html).toContain('value="error_count_desc">error count descending</option>');
    expect(html).toContain('value="error_proportion_desc">error proportion descending</option>');
  });
  it("renders return rows separately and keeps exact zero and missing return rows distinct", () => {
    const response = { ...occurrenceResponse, rows: occurrenceResponse.rows.map((row, index) => ({ ...row, metrics: [...row.metrics, fact("return_rows", index === 0 ? "exact" : "missing", index === 0 ? "0" : null, index === 0 ? "item" : null)] })) };
    const html = renderToStaticMarkup(<AuditOccurrenceTable occurrences={response} onSelect={() => undefined} selectedKey={null} text={english} />);
    expect(html).toContain('<th title="scanRows">Scanned rows</th><th title="returnRows">Returned rows</th>');
    expect(html).toContain('title="0 item">0</td>');
    // Missing keeps its own text and renders as a dashed tag; the exact zero stays a bare number.
    expect(html).toContain('title="— (missing)"><span class="audit-value-tag audit-value-missing">— (missing)</span></td>');
    expect(html).not.toContain('<span class="audit-value-tag audit-value-missing">0</span>');
  });

  it("keeps the full exact family digest in its accessible name while visible text stays shortened", () => {
    const membership = familyRows[0].membership;
    if (membership.state !== "exact_native_digest") throw new Error("fixture must be exact");
    const digest = "aae5d60b1afa38621234567890abcdef12";
    const family = { ...familyRows[0], membership: { ...membership, native_digest: digest } };
    const html = renderToStaticMarkup(<AuditFamilyList families={[family]} familyTop={null} onSelect={() => undefined} selectedKey={null} sort="cumulative_query_time_desc" text={english} />);
    expect(html).toContain(`aria-label="Select query family ${digest}"`);
    expect(html.replace(/<[^>]*>/g, "")).not.toContain(digest);
    expect(html).toContain(">aae5d60b<");
  });

  it("disables family selection while a read is in flight instead of ignoring the click", () => {
    const enabled = renderToStaticMarkup(<AuditFamilyList families={[familyRows[0]]} familyTop={null} onSelect={() => undefined} selectedKey={null} sort="cumulative_query_time_desc" text={english} />);
    const busy = renderToStaticMarkup(<AuditFamilyList disabled families={[familyRows[0]]} familyTop={null} onSelect={() => undefined} selectedKey={null} sort="cumulative_query_time_desc" text={english} />);
    expect(enabled).not.toMatch(/aria-label="Select query family [^"]*"[^>]*disabled/);
    expect(busy).toMatch(/aria-label="Select query family [^"]*"[^>]*disabled=""/);
  });

  it("renders server family fractions and resource aggregates without deriving them from member rows", () => {
    const family: AuditFamilyPresentation = { ...familyRows[0], error_proportion: { state: "exact", numerator: "9007199254740993", denominator: "9007199254740999" }, aggregates: [...familyRows[0].aggregates, { aggregation: "sum", metric: fact("cpu_cost", "exact", "3000000000", "ns"), source_nonnegative_count: "900" }, { aggregation: "max", metric: fact("memory_cost", "exact", "2000000000", "byte"), source_nonnegative_count: "900" }] };
    const summary = renderToStaticMarkup(<FamilySummary family={family} text={english} />);
    expect(summary).toContain("9007199254740993 / 9007199254740999");
    expect(summary).toContain("Cumulative CPU");
    expect(summary).toContain("3 s");
    expect(summary).toContain('title="3000000000 ns"');
    expect(summary).toContain("Maximum audit memory");
    expect(summary).toContain("1.863 GB");
    expect(summary).toContain("audit memory max is not concurrent total memory");
  });

  it("keeps unknown, invalid and zero-denominator error proportions distinct", () => {
    expect(errorProportionText({ state: "not_applicable", numerator: "0", denominator: "0" })).toBe("— (not_applicable · 0 / 0)");
    expect(errorProportionText({ state: "unavailable", reason: "error_count_missing" })).toBe("— (unavailable · error_count_missing)");
    expect(errorProportionText({ state: "invalid", reason: "numerator_exceeds_denominator" })).toBe("— (invalid · numerator_exceeds_denominator)");
    const html = renderToStaticMarkup(<FamilySummary family={{ ...familyRows[0], error_proportion: { state: "unavailable", reason: "error_count_missing" } }} text={english} />);
    expect(html).toContain("unavailable · error_count_missing");
  });
  it("provides six rolling bounded windows, retains all filters, and defaults to one hour", () => {
    const now = new Date("2026-09-05T00:00:00.999Z");
    const expected = { "1h": 3600, "12h": 43200, "1d": 86400, "3d": 259200, "7d": 604800, "30d": 2592000 } as const;
    for (const preset of Object.keys(expected) as (keyof typeof expected)[]) {
      const result = auditScopeForWindowPreset({ ...scope, user: "same-user" }, preset, now);
      expect(result).toStrictEqual({ ...scope, user: "same-user", window: { start_seconds: 1_788_566_400 - expected[preset], end_seconds: 1_788_566_400 } });
    }
    const html = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={connection} />);
    for (const preset of Object.keys(expected)) expect(html).toContain(`value="${preset}"`);
    expect(html).toContain('value="1h" selected=""');
    expect(html).not.toContain('value="15m"');
    expect(html).toContain("Last 1 month (30 days)");
    expect(html).toContain("Manual refresh");
  });

  it("keeps NULL, real zero, negative sentinels, overflow, and precision-unsafe decimals distinct without Number conversion", () => {
    expect(metricText(queryTime)).toBe("0 ns");
    expect(metricText(rows)).toBe("42");
    expect(metricText(nullCpu)).toBe("— (missing)");
    expect(metricText(negativeRows)).toBe("-1 (negative_sentinel)");
    expect(metricText(fact("memory_cost", "overflow", "999999999999999999999", null))).toBe("999999999999999999999 (overflow)");
    expect(metricText(unsafeBytes)).toBe("8 PB (precision_unsafe)");
  });

  it("renders returned counts and coverage limitations without technical counters", () => {
    const html = renderToStaticMarkup(<AuditWindowEvidence response={occurrenceResponse} scope={scope} text={english} />);
    expect(html).toContain("Partial results");
    expect(html).toContain("Returned 2 / 1000");
    expect(html).toContain("Some audit records conflict; affected metrics may be unavailable.");
    expect(html).toContain("Some queries lack a native SQL digest");
    expect(html).not.toContain("N whole-window retained");
    // Nothing to flag: the strip is not rendered at all (the window and counts live in the filter and table).
    const quiet = { ...occurrenceResponse, top: { ...occurrenceResponse.top, truncated: false }, coverage: { ...occurrenceResponse.coverage, conflicting_duplicate_row_count: "0", missing_digest_count: "0" } };
    expect(renderToStaticMarkup(<AuditWindowEvidence response={quiet} scope={scope} text={english} />)).toBe("");
  });

  it("renders the full-window KPI strip from coverage aggregate facts rather than returned Top rows", () => {
    const html = renderToStaticMarkup(<AuditKpiStrip concurrency={null} coverage={occurrenceResponse.coverage} text={english} />);
    expect(html).toContain("Query count");
    expect(html).toContain("1,000");
    expect(html).toContain("Cumulative query time");
    expect(html).toContain("7.654 ms");
    expect(html).toContain("Cumulative scanned bytes");
    expect(html).toContain("8 PB (precision_unsafe)");
    expect(html).toContain('title="9007199254740993 byte (precision_unsafe)"');
    expect(html).toContain("6 records lack this field");
    expect(html).toContain("Failed executions");
    expect(html).toContain(">17<");
    expect(html).toContain("summed across concurrency, not elapsed");
    expect(html).toContain("In-flight history unavailable");
  });

  it("keeps equal digests in distinct complete contexts and drills using the chosen catalog/database/digest without changing the fixed time window", () => {
    const [first, second] = familyRows;
    expect(familyRowKey(first, 0)).not.toBe(familyRowKey(second, 1));
    expect(auditScopeForFamily(scope, first)).toStrictEqual({ ...scope, native_digest: "aabb", catalog: "default_catalog", database: "demo" });
    expect(auditScopeForFamily(scope, second)).toStrictEqual({ ...scope, native_digest: "aabb", catalog: "other_catalog", database: "other" });
    expect(auditScopeForFamily(scope, { ...first, membership: { state: "ungrouped", reason: "native_digest_missing" } })).toBeNull();
  });

  it("binds family SQL to the narrowed detail scope and new generation while retaining the parent only for return", () => {
    const parentScope: AuditScope = { ...scope, catalog: "parent_catalog", database: "parent_database", resource_group: "parent_resource_group" };
    const request = auditFamilyDetailRequest(connection.connection_id, 7, parentScope, familyRows[0]);
    expect(request).toStrictEqual({ connection_id: connection.connection_id, discovery_generation: 7, scope: parentScope, family_key: familyRows[0].membership.state === "exact_native_digest" ? familyRows[0].membership.family_key : "" });
    expect(auditFamilyDetailRequest(connection.connection_id, 7, parentScope, { ...familyRows[0], membership: { state: "ungrouped", reason: "native_digest_missing" } })).toBeNull();

    const response = { ...familyDetail, family: familyRows[0].membership, scope: auditScopeForFamily(parentScope, familyRows[0])! };
    expect(isAuditFamilyDetailBound(response, request!, familyRows[0])).toBe(true);
    expect(isAuditFamilyDetailBound({ ...response, scope: parentScope }, request!, familyRows[0])).toBe(false);
    const slowest = familyDetailRepresentativeRows(response)[1];
    expect(response.scope).not.toStrictEqual(parentScope);
    const sql = auditFamilyDetailSqlRequest(connection.connection_id, request!.family_key, response, slowest);
    expect(sql).toStrictEqual({ connection_id: connection.connection_id, discovery_generation: 8, scope: response.scope, query_id: slowest.query.query_id });
    expect(sql?.scope).toStrictEqual(auditScopeForFamily(parentScope, familyRows[0]));
    expect(sql?.query_id).toBe(familyDetail.representatives.slowest.state === "selected" ? familyDetail.representatives.slowest.occurrence.query.query_id : "");
    expect(auditFamilyDetailSqlRequest(connection.connection_id, request!.family_key, response, { ...slowest })).toBeNull();
  });

  it("does not admit, render, or reuse a family identity after its connection changes", () => {
    const controller = new AbortController();
    expect(isAuditRequestCurrentForConnection(19, 19, controller.signal, connection.connection_id, connection.connection_id)).toBe(true);
    expect(isAuditRequestCurrentForConnection(19, 19, controller.signal, connection.connection_id, "replacement-connection")).toBe(false);
    expect(auditResponseForConnection(familyDetail, "replacement-connection")).toBeNull();
    expect(auditResponseForConnection(familyDetail, connection.connection_id)).toBe(familyDetail);

    const slowest = familyDetailRepresentativeRows(familyDetail)[1];
    const detailState = { familyKey: familyRows[0].membership.state === "exact_native_digest" ? familyRows[0].membership.family_key : "", parentScope: scope, response: familyDetail };
    expect(auditFamilyDetailDiscoveryGeneration(detailState, slowest, connection.connection_id)).toBe(8);
    expect(auditFamilyDetailDiscoveryGeneration(detailState, slowest, "replacement-connection")).toBeNull();

    controller.abort();
    expect(isAuditRequestCurrentForConnection(19, 19, controller.signal, connection.connection_id, connection.connection_id)).toBe(false);
  });

  it("renders server DTO bucket facts, representative states, and explicit actions without raw SQL or implicit Profile", () => {
    const html = renderToStaticMarkup(<AuditFamilyDetail detail={familyDetail} onBrowseMembers={() => undefined} onFetchProfile={() => undefined} onReturn={() => undefined} onSelectRepresentative={() => undefined} onViewSql={() => undefined} parentWindow="2023-11-14 22:13:20Z → 2023-11-14 23:13:20Z" profileLoading={false} sqlLoading={false} text={english} />);
    expect(html).toContain("Query family detail · exact family");
    expect(html).toContain("2023-11-14 22:13:20Z → 2023-11-14 23:13:20Z");
    expect(html).toContain("&lt; 10 ms");
    expect(html).toContain("5–60 min");
    expect(html).toContain('aria-label="&lt; 10 ms 596 / 597"');
    expect(html).toContain("596</span>");
    // Backend codes stay in titles / technical details, never as visible text.
    expect(html).toContain('title="no_match_in_bounded_profile_evidence">Unavailable · No Profile found among the checked candidates');
    expect(html).not.toMatch(/>[^<]*\b(lt_10ms|gte_60m|audit-query-time-histogram)\b/);
    expect(html).toContain("Profile list checked");
    expect(html).toContain("View SQL for slowest representative");
    expect(html).toContain("Fetch Profile for evidence representative");
    expect(html).toContain("Browse member rankings");
    expect(html).toContain("does not fetch SQL or Profile");
    expect(html).not.toContain("SELECT ");
  });

  it("returns from a family drill to the explicit parent scope while preserving edits made inside the drill", () => {
    const [family] = familyRows;
    const parentScope: AuditScope = {
      ...scope,
      catalog: "parent_catalog",
      database: "parent_database",
      resource_group: "parent_resource_group",
      user: "parent-user",
    };
    const drillScope = auditScopeForFamily(parentScope, family)!;
    expect(auditParentScopeAfterFamilyEdits(parentScope, drillScope, drillScope)).toStrictEqual(parentScope);

    const editedInFamily: AuditScope = {
      ...drillScope,
      window: { start_seconds: 1_700_003_600, end_seconds: 1_700_007_200 },
      state: "error",
      user: "edited-user",
      catalog: "edited_catalog",
      database: null,
      resource_group: "edited_resource_group",
      statement_filter: "all",
    };
    expect(auditParentScopeAfterFamilyEdits(parentScope, drillScope, editedInFamily)).toStrictEqual({
      window: editedInFamily.window,
      state: "error",
      user: "edited-user",
      native_digest: null,
      catalog: "edited_catalog",
      database: null,
      resource_group: "edited_resource_group",
      statement_filter: "all",
    });
  });

  it("keeps exact member rankings in family mode and restores the saved parent scope on a rankings switch", () => {
    const [family] = familyRows;
    const parentScope: AuditScope = { ...scope, catalog: "parent_catalog", database: "parent_database", resource_group: "parent_resource_group" };
    const drillScope = auditScopeForFamily(parentScope, family)!;
    const detail = { ...familyDetail, family: family.membership, scope: drillScope };
    const memberBrowse = auditFamilyMemberBrowseState(family, parentScope, detail);

    expect(memberBrowse).toStrictEqual({
      activeFamily: family,
      parentScope,
      drillScope,
      requestScope: drillScope,
      mode: "family",
    });
    expect(memberBrowse.requestScope).not.toStrictEqual(parentScope);

    const queryRankingsReturn = auditModeSwitchScopes({
      mode: memberBrowse.mode,
      draftScope: memberBrowse.drillScope,
      occurrenceScope: memberBrowse.requestScope,
      familyScope: null,
      familyParentScope: memberBrowse.parentScope,
      familyDrillScope: memberBrowse.drillScope,
    });
    expect(queryRankingsReturn.requestScope).toStrictEqual(parentScope);
    expect(queryRankingsReturn.draftScope).toStrictEqual(parentScope);
  });

  it("uses the applied response scope for a mode-switch request while retaining un-applied drafts", () => {
    const unAppliedDraft: AuditScope = { ...scope, state: "error", user: "draft-user", catalog: "draft_catalog", database: "draft_database" };
    const occurrenceSwitch = auditModeSwitchScopes({
      mode: "occurrences",
      draftScope: unAppliedDraft,
      occurrenceScope: occurrenceResponse.scope,
      familyScope: null,
      familyParentScope: null,
      familyDrillScope: null,
    });
    expect(occurrenceSwitch.requestScope).toStrictEqual(occurrenceResponse.scope);
    expect(occurrenceSwitch.draftScope).toStrictEqual(unAppliedDraft);
    const evidence = renderToStaticMarkup(<AuditWindowEvidence response={occurrenceResponse} scope={occurrenceSwitch.draftScope} windowPreset="12h" text={english} />);
    expect(evidence).toContain("Filters changed. Apply them to update the results.");
    expect(evidence).toContain("2023-11-14T22:13:20.000Z → 2023-11-14T23:13:20.000Z");
    expect(evidence).not.toContain("draft-user");

    const [family] = familyRows;
    const parentScope: AuditScope = { ...scope, user: "parent-user", catalog: "parent_catalog", database: "parent_database", resource_group: "parent_resource_group" };
    const drillScope = auditScopeForFamily(parentScope, family)!;
    const familyDraft: AuditScope = { ...drillScope, state: "error", user: "draft-user", catalog: "draft_catalog", database: "draft_database", resource_group: "draft_resource_group" };
    const familyReturn = auditModeSwitchScopes({
      mode: "family",
      draftScope: familyDraft,
      occurrenceScope: drillScope,
      familyScope: null,
      familyParentScope: parentScope,
      familyDrillScope: drillScope,
    });
    expect(familyReturn.requestScope).toStrictEqual(parentScope);
    expect(familyReturn.draftScope).toStrictEqual({ ...familyDraft, native_digest: null });

    const appliedDrillScope = auditScopeForWindowPreset({ ...drillScope, state: "error", user: "applied-user", catalog: "applied_catalog", database: "applied_database" }, "12h", new Date("2026-09-05T00:00:00.000Z"));
    const appliedParentScope = auditParentScopeAfterFamilyEdits(parentScope, drillScope, appliedDrillScope);
    const appliedFamilyReturn = auditModeSwitchScopes({
      mode: "family",
      draftScope: appliedDrillScope,
      occurrenceScope: appliedDrillScope,
      familyScope: null,
      familyParentScope: appliedParentScope,
      familyDrillScope: appliedDrillScope,
    });
    expect(appliedFamilyReturn.requestScope).toStrictEqual(appliedParentScope);
    expect(appliedFamilyReturn.draftScope).toStrictEqual(appliedParentScope);
  });

  it("keeps the selected UTC window and filters as a pending draft until Apply or Refresh", () => {
    const draft = { ...scope, user: "audit-user", state: null };
    const refreshed = auditScopeForWindowPreset(draft, "12h", new Date("2026-09-05T00:00:00.000Z"));
    expect(refreshed).toStrictEqual({ ...draft, window: { start_seconds: 1_788_523_200, end_seconds: 1_788_566_400 } });
    expect(auditTerminalStateFromControl("")).toBeNull();
    expect(auditTerminalStateFromControl("finished")).toBe("finished");
    expect(auditTerminalStateFromControl("error")).toBe("error");
    const html = renderToStaticMarkup(<AuditWindowEvidence response={occurrenceResponse} scope={draft} windowPreset="12h" text={english} />);
    expect(html).toContain("Filters changed. Apply them to update the results.");
  });

  it("disables exact user filtering when the source-column capability is unknown instead of omitting a requested filter", () => {
    const unavailableSession = { ...connection, capabilities: { ...connection.capabilities, audit_user_filter: "unknown" as const } };
    const availableSession = { ...connection, capabilities: { ...connection.capabilities, audit_user_filter: "available" as const } };
    const unknownHtml = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={unavailableSession} />);
    const availableHtml = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={availableSession} />);
    expect(unknownHtml).toMatch(/aria-label="Exact user filter" class="audit-filter-user" disabled=""/);
    expect(unknownHtml).toContain("User filter availability is unknown; source column availability is unknown.");
    expect(availableHtml).toContain('aria-label="Exact user filter"');
    expect(availableHtml).not.toContain("source column availability is unknown");
    expect(auditUserFilterNote("unavailable", english)).toContain("AuditLoader user column is unavailable");
  });

  it("offers the statement filter as an explicit default-on choice only when the session proved isQuery and stmt", () => {
    const unknownSession = { ...connection, capabilities: { ...connection.capabilities, audit_statement_filter: "unknown" as const } };
    const availableSession = { ...connection, capabilities: { ...connection.capabilities, audit_statement_filter: "available" as const } };
    const unknownHtml = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={unknownSession} />);
    const availableHtml = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={availableSession} />);
    const checkbox = (html: string) => html.match(/<input[^>]*aria-label="Hide this tool&#x27;s queries and non-query statements"[^>]*>/)?.[0] ?? "";
    expect(checkbox(availableHtml)).toContain('checked=""');
    expect(checkbox(availableHtml)).not.toContain("disabled");
    expect(availableHtml).toContain('class="audit-filter-more" hidden=""');
    expect(checkbox(unknownHtml)).toContain('disabled=""');
    expect(checkbox(unknownHtml)).not.toContain("checked");
    expect(unknownHtml).toContain("Statement filter availability is unknown; all statements are shown.");
    expect(auditStatementFilterNote("unavailable", english)).toContain("TINYINT isQuery");
    expect(defaultAuditScope(new Date("2026-09-05T00:00:00.000Z"), false).statement_filter).toBe("all");
  });

  it("shows server hidden-row counts for the statement filter instead of silently dropping rows", () => {
    // The table subtitle names the filter and carries the server's hidden counts; nothing is recomputed.
    expect(workspaceSource).toContain("Hidden: ${statementFilter.hidden_non_query_row_count} non-query and ${statementFilter.hidden_doctor_row_count} tool statements");
    expect(workspaceSource).toContain("statementFilter?.state === \"applied\" ? text(\"this tool's and non-query statements hidden\"");
    const pending = renderToStaticMarkup(<AuditWindowEvidence response={occurrenceResponse} scope={{ ...scope, statement_filter: "all" }} text={english} />);
    expect(pending).toContain("Filters changed. Apply them to update the results.");
  });

  it("marks only the 11-column individual-execution table for the fixed-width layout", () => {
    // Column widths live in audit-workload.css (guarded by tests/e2e/browser-e2e.static.test.mjs).
    const html = renderToStaticMarkup(<AuditOccurrenceTable occurrences={occurrenceResponse} onSelect={() => undefined} selectedKey={null} text={english} />);
    expect(html).toContain('<table aria-label="Audited query rankings" class="audit-occurrence-table">');
    expect(html.match(/<th[ >]/g)).toHaveLength(11);
    expect(html).toContain("<th>Query ID</th><th>SQL</th><th>Start / reader zone</th>");
    // The 13-column historical in-flight view keeps the scrollable minimum and its own layout.
    const occurrence = occurrenceResponse.rows[0];
    const activeAt = { participants: { state: "exact", rows: [{ occurrence, start_milliseconds: "1", end_milliseconds: "2", elapsed_milliseconds: "1" }] } } as unknown as AuditActiveAtResponse;
    const participants = renderToStaticMarkup(<AuditOccurrenceTable activeAt={activeAt} occurrences={null} onSelect={() => undefined} selectedKey={null} text={english} />);
    expect(participants).not.toContain("audit-occurrence-table");
    expect(participants.match(/<th[ >]/g)).toHaveLength(13);
  });

  it("shows each execution's own SQL on one line with the full prefix in title, and every missing state as a dashed tag", () => {
    const html = renderToStaticMarkup(<AuditOccurrenceTable occurrences={occurrenceResponse} onSelect={() => undefined} selectedKey={null} text={english} />);
    // The returned prefix is shown as is (CSS keeps it on one line); truncation adds the ellipsis.
    expect(html).toContain('<td class="audit-cell-sql" title="SELECT customer_id,\n  SUM(amount)\nFROM orders…">SELECT customer_id,\n  SUM(amount)\nFROM orders…</td>');
    expect(html).toContain('<td class="audit-cell-sql" title="The audited stmt is SQL NULL."><span class="audit-value-tag">stmt NULL</span></td>');
    const cell = (sample: AuditStatementSample, locale: LocalizedText = english) => renderToStaticMarkup(<table><tbody><tr><OccurrenceStatementCell sample={sample} text={locale} /></tr></tbody></table>);
    const cases: [AuditStatementSample, string][] = [
      [{ state: "text", value: "", truncated: false }, "Empty stmt"],
      [{ state: "text", value: " \n ", truncated: false }, "Empty stmt"],
      [{ state: "unavailable", reason: "source_column_absent" }, "No stmt column"],
      [{ state: "unavailable", reason: "not_read" }, "Not read"],
      [{ state: "unavailable", reason: "conflicting_source_rows" }, "Rows disagree"],
    ];
    for (const [sample, label] of cases) {
      const rendered = cell(sample);
      expect(rendered).toContain(`<span class="audit-value-tag">${label}</span>`);
      expect(rendered).not.toMatch(/<td class="audit-cell-sql"[^>]*><\/td>/);
    }
    expect(cell({ state: "text", value: "SELECT 1", truncated: false })).toContain('title="SELECT 1">SELECT 1</td>');
    expect(cell({ state: "unavailable", reason: "conflicting_source_rows" }, chinese)).toContain("重复行不一致");
  });

  it("says the table is being read while a refresh has no rows yet, and opens a closable details panel", () => {
    const reading = renderToStaticMarkup(<AuditOccurrenceTable disabled occurrences={null} onSelect={() => undefined} selectedKey={null} text={english} />);
    expect(reading).toContain('<p class="audit-empty" role="status">Reading audit records…</p>');
    expect(reading).not.toContain("No occurrences were returned");
    const occurrence = occurrenceResponse.rows[0];
    const panel = renderToStaticMarkup(<SelectedAuditOccurrence occurrence={occurrence} doctorProfile={null} onClose={() => undefined} onDiagnose={() => undefined} onViewSql={() => undefined} profile={null} profileLoading={false} sql={null} sqlError={null} sqlLoading={false} text={english} />);
    expect(panel).toContain('aria-label="Close details" class="audit-selection-close"');
    // The full query ID stays on the shortened one.
    expect(panel).toContain(`<code title="${occurrence.query.query_id}">`);
  });

  it("renders every returned row inside the scrollable table and exposes an accessible row selector", () => {
    const html = renderToStaticMarkup(<AuditOccurrenceTable occurrences={occurrenceResponse} onSelect={() => undefined} selectedKey={occurrenceRowKey(occurrenceResponse.rows[0], 0)} text={english} />);
    expect(html).toContain('class="audit-table-wrap"');
    expect(html).toContain("audit-query-1");
    expect(html).toContain("audit-query-2");
    expect(html).toContain("Source state");
    expect(html).toContain('title="finished">Finished</td>');
    expect(html.match(/aria-label="Select audited query/g)).toHaveLength(2);
    expect(html).toContain("9007199254740993 byte (precision_unsafe)");
  });

  it("renders unmerged server family facts and allows exact-family member selection only", () => {
    const ungrouped: AuditFamilyPresentation = { ...familyRows[1], membership: { state: "ungrouped", reason: "native_digest_missing" } };
    const html = renderToStaticMarkup(<AuditFamilyList families={[familyRows[0], ungrouped]} familyTop={null} onSelect={() => undefined} selectedKey={null} sort="cumulative_query_time_desc" text={english} />);
    expect(html).toContain("123.456 μs");
    expect(html).toContain("8 PB (precision_unsafe)");
    expect(html).toContain('title="9007199254740993 byte (precision_unsafe)"');
    expect(html).toMatch(/aria-pressed="false" disabled="" type="button">Not grouped</);
  });

  it("binds on-demand SQL to its connection, discovery generation, fixed scope, and current query while stale or aborted responses fail closed", () => {
    const request = { connection_id: connection.connection_id, discovery_generation: 7, scope, query_id: "audit-query-1" };
    const sql = { binding: request, result: { query_id: "audit-query-1", discovery_generation: 7, sql: "SELECT explicit_only" } };
    expect(visibleSql(sql, request)).toBe("SELECT explicit_only");
    expect(visibleSql(sql, { ...request, discovery_generation: 8 })).toBeNull();
    expect(visibleSql(sql, { ...request, scope: { ...scope, database: "other" } })).toBeNull();
    const controller = new AbortController();
    expect(isCurrentAuditRequest(4, 4, controller.signal)).toBe(true);
    controller.abort();
    expect(isCurrentAuditRequest(5, 4, controller.signal)).toBe(false);
  });

  it("opens Doctor only for a fetched report bound to the selected execution and returns typed server limitation labels", () => {
    const profile = { ...sr3514ConnectedContractGolden.profile_fetch_response, query: baseQuery };
    expect(auditProfileForSelectedOccurrence(baseQuery, 7, profile)).toBe(profile);
    expect(auditReportForSelectedOccurrence(baseQuery, 7, profile)).toBe(profile.outcome.report);
    expect(auditReportForSelectedOccurrence({ ...baseQuery, query_id: "other" }, 7, profile)).toBeNull();
    expect(auditReportForSelectedOccurrence(baseQuery, 8, profile)).toBeNull();
    expect(auditProfileForSelectedOccurrence({ ...baseQuery, connection_id: "other-session" }, 7, profile)).toBeNull();
    expect(auditErrorLabel({ message_id: "request_failed", details: { limitation: "query_discovery_generation_expired" } })).toBe("query_discovery_generation_expired");
    expect(auditErrorLabel(new Error("response body must not be displayed"))).toBe("audit_request_failed");
  });

  it("uses fixed UTC defaults and Chinese/English accessible labels", () => {
    expect(defaultAuditScope(new Date("2026-09-05T00:00:00.000Z"))).toStrictEqual({ ...scope, state: null, window: { start_seconds: 1_788_562_800, end_seconds: 1_788_566_400 } });
    const englishHtml = renderToStaticMarkup(<AuditOccurrenceTable occurrences={occurrenceResponse} onSelect={() => undefined} selectedKey={null} text={english} />);
    const chineseHtml = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AuditOccurrenceTable occurrences={occurrenceResponse} onSelect={() => undefined} selectedKey={null} text={(en, zh) => zh} /></LocaleProvider>);
    expect(englishHtml).toContain('aria-label="Audited query rankings"');
    expect(chineseHtml).toContain('aria-label="审计查询排行"');
    expect(englishHtml).not.toContain('aria-pressed="true"');
  });

  it("lands on repeated SQL with an explicit empty selection instead of selecting a returned row", () => {
    const html = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={connection} />);
    expect(html).toContain('class="audit-workload audit-workload-overview"');
    expect(html).toContain("Repeated SQL · frequency and cumulative cost");
    expect(html).toMatch(/aria-pressed="true"[^>]*>Repeated SQL</);
    // No detail panel until a row is chosen: the list keeps the full width.
    expect(html).not.toContain("audit-selection");
    expect(html).not.toContain("audit-family-aside");
    expect(html).not.toContain('aria-selected="true"');
  });

  it("offers one Diagnose action, switches to Open diagnosis once fetched, and names why a row has no Profile", () => {
    const occurrence = occurrenceResponse.rows[0];
    const props = { onDiagnose: () => undefined, onViewSql: () => undefined, profile: null, profileLoading: false, sql: null, sqlError: null, sqlLoading: false, text: english };
    const unfetched = renderToStaticMarkup(<SelectedAuditOccurrence {...props} doctorProfile={null} occurrence={occurrence} onOpenDoctor={() => undefined} />);
    expect(unfetched).toContain(">Diagnose</button>");
    expect(unfetched).not.toContain("Open diagnosis");
    expect(unfetched).toContain("the SQL is never rerun");

    const fetched = { ...sr3514ConnectedContractGolden.profile_fetch_response, query: occurrence.query };
    const ready = renderToStaticMarkup(<SelectedAuditOccurrence {...props} doctorProfile={fetched} occurrence={occurrence} onOpenDoctor={() => undefined} />);
    expect(ready).toContain(">Open diagnosis</button>");
    expect(ready).not.toContain(">Diagnose</button>");

    const noProfile = { ...occurrence, query: { ...occurrence.query, profile_fetch: { state: "unavailable" as const, reason: "query_not_finished" as const } } };
    const unavailable = renderToStaticMarkup(<SelectedAuditOccurrence {...props} doctorProfile={null} occurrence={noProfile} onOpenDoctor={() => undefined} />);
    expect(unavailable).toMatch(/<button class="button accent" disabled=""[^>]*>Diagnose<\/button>/);
    expect(unavailable).toContain("No Profile to diagnose: Query not finished");

    const empty = renderToStaticMarkup(<SelectedAuditOccurrence {...props} doctorProfile={null} occurrence={null} />);
    expect(empty).toContain("audit-selection-empty");
    expect(empty).not.toContain("<button");
  });

  it("explains a missing Profile plainly and keeps other fetch failures generic", () => {
    expect(profileFetchFailureText({ message_id: "not_found_or_expired_or_not_profiled" }, english)).toContain("ran without enable_profile");
    expect(profileFetchFailureText({ message_id: "request_failed", details: { limitation: "not_found_or_expired_or_not_profiled" } }, english)).toContain("ran without enable_profile");
    const generic = profileFetchFailureText(new Error("raw body must not leak"), english);
    expect(generic).toContain("connection may be unavailable");
    expect(generic).not.toContain("raw body");
  });

  it("keeps the time range a draft like the other filters until Apply or Refresh", () => {
    const html = renderToStaticMarkup(<AuditWorkloadWorkspace onConnect={() => undefined} session={connection} />);
    expect(html).toContain('aria-label="Audit window" class="audit-filter-window"');
    expect(html).toContain('<option value="1h" selected="">Last 1 h</option>');
    expect(workspaceSource).toContain("onChange={(event) => setWindowPreset(event.target.value as AuditWindowPreset)}");
    expect(workspaceSource).toContain("const nextScope = auditScopeForWindowPreset(scope, windowPreset);");
  });
});

it("both real return buttons discard click events and restore the parent family scope", () => {
  const source = workspaceSource;
  const body = source.slice(source.indexOf('    if (familyDetail && !currentFamilyDetail) return;'), source.indexOf('\n  function browseFamilyMembers')).replace(/\n  }\s*$/, '');
  const parent = { ...scope, user: 'parent-user', database: 'parent-db' };
  const drill = auditScopeForFamily(parent, familyRows[0])!;
  for (const kind of ['detail', 'summary']) {
    const observed: Record<string, unknown> = {};
    const run = new Function('nextScope', 'scope', 'familyDetail', 'currentFamilyDetail', 'familyParentScope', 'familyDrillScope', 'auditParentScopeAfterFamilyEdits', 'clearFamilyDrillScope', 'setScope', 'setWindowPreset', 'auditWindowPresetForScope', 'windowPreset', 'setActiveFamily', 'setFamilyDetail', 'setFamilyParentScope', 'setFamilyDrillScope', 'setMode', 'loadFamilies', body);
    const onReturn = (...args: unknown[]) => {
      expect(args).toHaveLength(0);
      run(drill, drill, true, {}, parent, drill, auditParentScopeAfterFamilyEdits, () => { throw new Error('unexpected fallback'); }, (value: unknown) => { observed.scope = value; }, () => undefined, (value: AuditScope) => { expect(value.window).toEqual(parent.window); return '1h'; }, '1h', () => undefined, (value: unknown) => { observed.detail = value; }, () => undefined, () => undefined, (value: unknown) => { observed.mode = value; }, (value: unknown) => { observed.request = value; });
    };
    const element = kind === 'detail'
      ? AuditFamilyDetail({ detail: familyDetail, parentWindow: '', onBrowseMembers() {}, onFetchProfile() {}, onReturn, onSelectRepresentative() {}, onViewSql() {}, profileLoading: false, sqlLoading: false, text: english })
      : FamilySummary({ family: familyRows[0], onReturn, text: english });
    const handler = returnButton(element);
    expect(handler).toBeTypeOf('function');
    handler!({ type: 'click', currentTarget: {} });
    expect(observed.scope).toStrictEqual(parent);
    expect(observed.request).toStrictEqual(parent);
    expect(observed.mode).toBe('families');
    expect(observed.detail).toBeNull();
  }
  expect(source).toContain('returnToParent(nextScope)');
});

// Task56: ordinary navigation must not turn a shared read-only session into an
// explicitly cancelled transport; existing generation/connection/view checks
// still reject the drained response after leaving this workspace.
it("invalidates Audit navigation admission without aborting the shared session read", () => {
  const invalidate = workspaceSource.slice(workspaceSource.indexOf("  const invalidateRequests ="), workspaceSource.indexOf("  const beginRequest ="));
  expect(invalidate).toContain("generation.current += 1");
  expect(invalidate).not.toContain("controller.abort()");
  expect(workspaceSource).toContain("useEffect(() => () => invalidateRequests(), [invalidateRequests])");
});


it("uses Chinese metric labels while retaining source keys and collapses input diagnostics", () => {
  const html = renderToStaticMarkup(<AuditOccurrenceTable occurrences={null} selectedKey={null} onSelect={() => undefined} text={(_en, zh) => zh} />);
  expect(html).toContain('<th title="queryTime">查询耗时</th>');
  expect(html).toContain('<th title="returnRows">返回行数</th>');
  expect(html).toContain('<details class="audit-coverage"><summary>指标覆盖详情</summary>');
});


it("keeps truncation visible without technical counters", () => {
  const html = renderToStaticMarkup(<AuditWindowEvidence response={occurrenceResponse} scope={scope} text={english} />);
  expect(html).toContain("Returned 2 / 1000");
  expect(html).toContain("Partial results");
  for (const technical of ["N whole-window", "epoch", "scope /"]) expect(html.replace(/title="[^"]*"/g, "")).not.toContain(technical);
});


it("guides investigation by existing server sorts and a one-step Profile diagnosis", () => {
  const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AuditWorkloadWorkspace session={connection} onConnect={() => undefined} /></LocaleProvider>);
  expect(html).toContain("手动刷新 · 最多最近 30 天");
  expect(html).toContain('<option value="query_time_desc" selected="">单次耗时最高</option>');
  expect(html).toContain('<option value="execution_count_desc">执行最频繁</option>');
  expect(html).toContain("累计耗时最高");
  expect(html).toContain("同类 SQL · 频次与累计消耗");
  expect(workspaceSource).toContain('text("Diagnose", "诊断")');
  expect(workspaceSource).toContain("onDiagnose={() => void fetchProfile(selected, true)}");
  expect(workspaceSource).toContain("if (openAfter) onOpenDoctor?.(result);");
});


it("distinguishes readable terminal statuses and preserves unknown future states", () => {
  const zh = (_en: string, chinese: string) => chinese;
  expect(auditStatusLabel("finished", zh)).toBe("已完成");
  expect(auditStatusLabel("error", zh)).toBe("执行失败");
  expect(auditStatusLabel("unknown", zh)).toBe("状态未知");
  expect(auditStatusLabel("future_state", zh)).toBe("future_state");
  const response = { ...occurrenceResponse, rows: [{ ...occurrenceResponse.rows[0], query: { ...baseQuery, profile_fetch: { state: "unavailable" as const, reason: "query_not_finished" as const } } }] };
  const html = renderToStaticMarkup(<AuditOccurrenceTable occurrences={response} selectedKey={null} onSelect={() => undefined} text={zh} />);
  expect(html).toContain('title="query_not_finished">查询尚未结束</td>');
});

describe("session end reason and family member SQL binding", () => {
  it("tells a timed-out read apart from an idle expiry", () => {
    expect(sessionEndReason({ code: "remote_timeout" })).toBe("remote_timeout");
    expect(sessionEndReason({ code: "connection_expired" })).toBeUndefined();
    expect(sessionEndReason(new Error("connection_expired"))).toBeUndefined();
  });

  it("binds a member SQL read to the member list's own generation and scope", () => {
    const row = occurrenceResponse.rows[0];
    expect(familyMemberSqlRequest("c1", occurrenceResponse, row)).toEqual({ connection_id: "c1", discovery_generation: occurrenceResponse.discovery_generation, scope: occurrenceResponse.scope, query_id: row.query.query_id });
    expect(familyMemberSqlRequest("c1", occurrenceResponse, { ...row })).toBeNull();
    expect(familyMemberSqlRequest("c1", null, row)).toBeNull();
  });
});
