import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { AuditActiveAtParticipant, AuditActiveAtParticipants, AuditActiveAtResponse, AuditMetricFact, AuditOccurrencePresentation, AuditOccurrenceResponse, AuditScope } from "../../../../contracts/generated/doctor-contracts";
import { AuditActiveAtEvidence, AuditOccurrenceTable } from "./AuditWorkloadWorkspace";

const english = (value: string): string => value;
const connection = sr3514ConnectedContractGolden.connection;
const scope: AuditScope = {
  window: { start_seconds: 1_700_000_000, end_seconds: 1_700_003_600 },
  state: "finished",
  user: null,
  native_digest: null,
  catalog: null,
  database: null,
  resource_group: null,
  statement_filter: "all",
};

const queryTime: AuditMetricFact = {
  metric_id: "query_time",
  input: { state: "text", value: "7" },
  state: "exact",
  parser_state: "exact",
  canonical_number: { kind: "integer", value: "7", scale: null },
  canonical_unit: "ns",
};

const makeOccurrence = (queryId: string): AuditOccurrencePresentation => ({
  query: {
    ...sr3514ConnectedContractGolden.query_response.queries[0],
    connection_id: connection.connection_id,
    discovery_generation: 7,
    query_id: queryId,
    observed_at_utc: "2026-09-05T01:02:03Z",
    query_time_ms: 7,
    source_refs: [`audit:${queryId}`],
  },
  catalog: "default_catalog", error_code: { state: "unavailable", reason: "source_column_absent" },
  metrics: [queryTime],
  family: { state: "ungrouped", reason: "native_digest_missing" },
  statement_sample: { state: "unavailable", reason: "not_read" },
});

const makeActiveAt = (participants: AuditActiveAtParticipants, activeCount = "700"): AuditActiveAtResponse => ({
  schema_version: "audit-workload.v1",
  connection_id: connection.connection_id,
  discovery_generation: 7,
  scope,
  at_milliseconds: "9007199254740993",
  source_coverage: { state: "partial", reason: "bounded_lookback", source_start_seconds: 1_699_996_400, source_end_seconds: 1_700_003_600, lookback_seconds: 3600 },
  active_count: activeCount,
  participants,
  cluster_identity: { state: "session_scoped", connection_id: connection.connection_id, detected_version: "3.5.14" },
  audit_timestamp_time_zone: connection.time_zone,
  limitations: ["audit_async_ingestion"],
});

const participantRows: AuditActiveAtParticipant[] = [
  { occurrence: makeOccurrence("participant-query-1"), start_milliseconds: "9007199254740990", end_milliseconds: "9007199254740997", elapsed_milliseconds: "9007199254740998" },
  { occurrence: makeOccurrence("participant-query-2"), start_milliseconds: "9007199254740991", end_milliseconds: "9007199254740999", elapsed_milliseconds: "9007199254741000" },
  { occurrence: makeOccurrence("participant-query-3"), start_milliseconds: "9007199254740992", end_milliseconds: "9007199254741001", elapsed_milliseconds: "9007199254741002" },
];

const truncatedActiveAt = makeActiveAt({ state: "truncated", rows: participantRows, omitted_count: "697" });
const legacyOccurrences = { rows: [makeOccurrence("legacy-query")] } as unknown as AuditOccurrenceResponse;

describe("Audit active-at participant evidence contract", () => {
  it("labels a fresh historical, non-blocking snapshot and keeps 3/700/697 separate from Top", () => {
    const html = renderToStaticMarkup(<AuditActiveAtEvidence response={truncatedActiveAt} text={english} />);

    expect(html).toContain("historical in-flight");
    expect(html).toContain("Fresh separate read; not the chart snapshot.");
    expect(html).toContain("not a blocked-query list");
    expect(html).toContain("historical in-flight 700");
    expect(html).toContain("Returned 3");
    expect(html).toContain("697 omitted");
    expect(html).toContain("The curve is not a Top-500 sample.");
    expect(html).not.toContain("Top 10");
  });

  it("does not fall back to the old occurrence rows when the active-at drill is empty", () => {
    const emptyActiveAt = makeActiveAt({ state: "exact", rows: [] }, "0");
    const html = renderToStaticMarkup(<AuditOccurrenceTable activeAt={emptyActiveAt} occurrences={legacyOccurrences} onSelect={() => undefined} selectedKey={null} text={english} />);

    expect(html).toContain("No retained query interval covers this time.");
    expect(html).not.toContain("legacy-query");
    expect(html).not.toContain("Query rankings");
  });

  it("renders server end and elapsed displays while retaining exact raw values instead of deriving from queryTime", () => {
    const html = renderToStaticMarkup(<AuditOccurrenceTable activeAt={truncatedActiveAt} occurrences={legacyOccurrences} onSelect={() => undefined} selectedKey={null} text={english} />);

    expect(html).toContain("9007199254740997 ms");
    expect(html).toContain('title="9007199254740997 ms"');
    expect(html).toContain("2501999792h 59m");
    expect(html).toContain('title="9007199254740998 ms"');
    expect(html).toContain("7 ns");
    expect(html).toContain('title="7 ns"');
    expect(html).not.toContain("9007199254740996");
    expect(html).not.toContain("9007199254740995");
  });

  it("keeps the disabled participant row and SQL default hidden from the table", () => {
    const html = renderToStaticMarkup(<AuditOccurrenceTable activeAt={truncatedActiveAt} disabled occurrences={legacyOccurrences} onSelect={() => undefined} selectedKey={null} text={english} />);

    expect(html.match(/disabled=""/g)).toHaveLength(3);
    expect(html).not.toContain("SELECT");
    expect(html).not.toContain("Selected audited SQL");
  });
});
