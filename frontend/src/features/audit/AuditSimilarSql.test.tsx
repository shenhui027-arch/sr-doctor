import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AuditMetricFact, AuditSimilarSqlResponse, SimilarSqlMember } from "../../../../contracts/generated/doctor-contracts";
import { AuditSimilarAside, AuditSimilarSqlList, similarClusterName } from "./AuditSimilarSql";

const english = (value: string): string => value;
const fact = (value: string | null): AuditMetricFact => value === null
  ? { metric_id: "query_time", input: { state: "sql_null" }, state: "missing", parser_state: "missing", canonical_number: null, canonical_unit: null }
  : { metric_id: "query_time", input: { state: "text", value }, state: "exact", parser_state: "exact", canonical_number: { kind: "integer", value: `${value}000000`, scale: null }, canonical_unit: "ns" };
const facts = (sum: string | null) => [
  { aggregation: "sum" as const, metric: fact(sum), source_nonnegative_count: sum === null ? "0" : "1" },
  { aggregation: "max" as const, metric: fact(sum), source_nonnegative_count: sum === null ? "0" : "1" },
];
const basis = { tables: ["customers", "orders"], joins: ["INNER JOIN customers.id = orders.cid"], predicate_columns: ["customers.region", "orders.dt"] };
const member = (key: string, official: boolean, template: string, sum: string | null): SimilarSqlMember => ({
  member_key: key,
  source: official ? { kind: "official_digest", native_digest: "9a1b2c3d" } : { kind: "product_fingerprint", fingerprint: "7c41e0a9" },
  catalog: "default_catalog", database: "demo", template, structure: { state: "available", basis }, detached: false,
  execution_count: official ? "188" : "143", error_count: "0", error_proportion: { state: "exact", numerator: "0", denominator: "1" }, aggregates: facts(sum),
});
const response: AuditSimilarSqlResponse = {
  schema_version: "audit-workload.v1", connection_id: "c",
  scope: { window: { start_seconds: 1, end_seconds: 2 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" },
  cluster_identity: { state: "session_scoped", connection_id: "c", detected_version: "3.5.14" }, audit_timestamp_time_zone: null,
  structure_version: "sr-doctor-structure.v1", dimensions: { tables: true, join: true, predicates: true },
  clusters: [{
    cluster_key: "similar-v1:aa", catalog: "default_catalog", database: "demo", basis, refined: false,
    members: [member("a", true, "SELECT o.* FROM orders o JOIN customers c ON o.cid = c.id WHERE c.region = ?", "2472"), member("b", false, "SELECT o.id FROM orders o JOIN customers c ON o.cid = c.id WHERE c.region = ?", null)],
    execution_count: "331", error_count: "0", error_proportion: { state: "exact", numerator: "0", denominator: "331" }, aggregates: facts("2472"),
  }],
  independent: [],
  unavailable: [{ ...member("u", true, "WITH x AS (SELECT 1) SELECT * FROM x", "1"), structure: { state: "unavailable", reason: "common_table_expression" } }],
  top: { requested_limit: 20, returned_count: 20, full_window_count: "31", truncated: true },
  coverage: {
    exact_families: { state: "partial", read: 20, in_window: "31" },
    fingerprint: { eligible_event_count: "0", read_limit: 500, read_event_count: "0", source: { state: "complete" }, fingerprinted_event_count: "0", partial_event_count: "0", unavailable: [], family_count: "0" },
    member_count: "3", clustered_member_count: "2", independent_member_count: "0", unavailable_member_count: "1",
  },
  limitations: [],
};

describe("design board A2 similar SQL", () => {
  it("names a cluster by its server basis and shows the basis, source mix and exclusions", () => {
    expect(similarClusterName(response.clusters[0], english)).toBe("customers ⋈ orders · customers.region, orders.dt");
    const html = renderToStaticMarkup(<AuditSimilarSqlList onSelect={() => undefined} response={response} selectedKey="similar-v1:aa" text={english} />);
    expect(html).toContain('aria-haspopup="dialog"');
    expect(html).toContain('aria-label="Select cluster customers ⋈ orders · customers.region, orders.dt"');
    expect(html).toContain('class="is-selected"');
    expect(html).not.toContain("aria-pressed");
    expect(html).not.toContain("aria-selected");
    expect(html).toContain("INNER JOIN customers.id = orders.cid");
    expect(html).toContain("1 official + 1 unofficial");
    expect(html).toContain("331");
    expect(html).toContain("WITH clause 1");
    expect(html).toContain("the first 20 of 31");
  });

  it("renders the selected cluster in a right-side drawer, labels member sources, keeps a missing metric a tag, and offers refine only when predicates are off", () => {
    const html = renderToStaticMarkup(<AuditSimilarAside cluster={response.clusters[0]} dimensions={response.dimensions} loading={false} onClose={() => undefined} onDetach={() => undefined} onRefine={() => undefined} onUndo={() => undefined} splitCount={1} text={english} />);
    expect(html).toContain('role="dialog"');
    expect(html).toContain("modal-card modal-card--sheet modal-card--wide");
    expect(html).toContain("customers ⋈ orders · customers.region, orders.dt");
    expect(html).toContain('aria-label="Close dialog"');
    expect(html).not.toContain('role="complementary"');
    expect(html).toContain("Official digest");
    expect(html).toContain("Unofficial fingerprint");
    expect(html).toContain("1 template(s) split out (this view only).");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Refine by predicate columns<\/button>/);
    expect(html).toContain('<span class="badge-stamp badge-stamp--planned">Planned</span> Keeping splits for later windows is not available in this version');
    expect(html.match(/badge-stamp--planned/g)).toHaveLength(1);

    const closedHtml = renderToStaticMarkup(<AuditSimilarAside cluster={null} dimensions={response.dimensions} loading={false} onClose={() => undefined} onDetach={() => undefined} onRefine={() => undefined} onUndo={() => undefined} open={false} splitCount={0} text={english} />);
    expect(closedHtml).toBe("");
  });

  it("handles unknown unavailable reasons and unknown member source kinds without throwing or rendering undefined (correctness fix #6)", () => {
    const unknownReasonResponse: AuditSimilarSqlResponse = {
      ...response,
      unavailable: [{ ...response.unavailable[0], structure: { state: "unavailable", reason: "future_structure_reason" as never } }],
    };
    const listHtml = renderToStaticMarkup(<AuditSimilarSqlList onSelect={() => undefined} response={unknownReasonResponse} selectedKey={null} text={english} />);
    expect(listHtml).toContain("future_structure_reason 1");
    expect(listHtml).not.toContain("undefined");

    const unknownSourceCluster = {
      ...response.clusters[0],
      members: [{ ...response.clusters[0].members[0], source: { kind: "future_source_kind" } as never }],
    };
    const asideHtml = renderToStaticMarkup(<AuditSimilarAside cluster={unknownSourceCluster} dimensions={response.dimensions} loading={false} onClose={() => undefined} onDetach={() => undefined} onRefine={() => undefined} onUndo={() => undefined} splitCount={0} text={english} />);
    expect(asideHtml).toContain("future_source_kind");
    expect(asideHtml).not.toContain("undefined");
  });
});

describe("A2 split state", () => {
  it("shows an empty predicate set as words, not braces", () => {
    const empty = { ...response, clusters: [{ ...response.clusters[0], basis: { ...basis, predicate_columns: [] } }] };
    const html = renderToStaticMarkup(<AuditSimilarSqlList onSelect={() => undefined} response={empty} selectedKey={null} text={english} />);
    expect(html).toContain("no predicates");
    expect(html).not.toContain("predicates {}");
  });
});
