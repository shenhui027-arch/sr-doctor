import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AuditFingerprintFamilyResponse, AuditOverviewResponse, AuditScope } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider } from "../../i18n/locale";
import { mainMembers, mainOverview } from "../../visual/demo-session";
import { AuditFamilyAside, AuditFamilyList, AuditFingerprintFamilyList, AuditKpiStrip, inFlightSamples } from "./AuditOverviewPanels";

const english = (value: string): string => value;
const scope: AuditScope = { window: { start_seconds: 1_700_000_000, end_seconds: 1_700_003_600 }, state: null, user: null, native_digest: null, catalog: null, database: "demo", resource_group: null, statement_filter: "exclude_doctor_and_non_query" };
const overview: AuditOverviewResponse = mainOverview(scope);
const families = overview.primary.state === "families" ? overview.primary.rows : [];

describe("design board 01 panels", () => {
  it("renders the KPI strip from server facts, with long durations and missing-field counts", () => {
    const html = renderToStaticMarkup(<AuditKpiStrip concurrency={overview.concurrency} coverage={overview.coverage} text={english} />);
    expect(html).toContain("1,284");
    expect(html).toContain("3h 12m");
    expect(html).toContain("2h 47m");
    expect(html).toContain("5.6 TB");
    expect(html).toContain("3 records lack this field");
    expect(html).toContain('class="audit-kpi audit-kpi-critical"');
  });

  it("samples the server step series at 30 instants without aggregating, and says so when it is unavailable", () => {
    const samples = inFlightSamples(overview.concurrency);
    expect(samples).toHaveLength(30);
    expect(samples[24].permyriad).toBe(9200);
    // Each midpoint falls inside one server step, whose exact count is shown as is.
    const points = overview.concurrency.series.state === "exact" ? overview.concurrency.series.points : [];
    expect(samples.map((sample) => sample.count)).toEqual(points.map((point) => point.active_count));
    const unavailable = { ...overview.concurrency, series: { state: "unavailable" as const, reason: "transition_point_budget_exceeded" as const } };
    expect(inFlightSamples(unavailable)).toEqual([]);
    expect(renderToStaticMarkup(<AuditKpiStrip concurrency={unavailable} coverage={overview.coverage} text={english} />)).toContain("In-flight history unavailable");
  });

  it("shows each family's audited SQL sample, the server share, and a missing scan as a dashed tag, never 0", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="en"><AuditFamilyList families={families} familyTop={overview.family_top} onSelect={() => undefined} selectedKey={null} sort="cumulative_query_time_desc" text={english} /></LocaleProvider>);
    expect(html).toContain("SELECT region, AVG(price) FROM sales");
    expect(html).toContain("Cumulative query time\u00a0↓");
    expect(html).toContain('style="width:61%"');
    expect(html).toContain('class="audit-missing-tag"');
    expect(html).toContain("1h 08m");
    expect(html).toContain('class="num last failed"');
  });

  it("labels a family without a usable statement sample instead of inventing SQL", () => {
    const sqlNull = { ...families[0], statement_sample: { state: "sql_null" as const } };
    const noColumn = { ...families[1], statement_sample: { state: "unavailable" as const, reason: "source_column_absent" as const } };
    const notRead = { ...families[2], statement_sample: { state: "unavailable" as const, reason: "not_read" as const } };
    const html = renderToStaticMarkup(<AuditFamilyList families={[sqlNull, noColumn, notRead]} familyTop={null} onSelect={() => undefined} selectedKey={null} sort="execution_count_desc" text={english} />);
    expect(html).toContain("audited stmt is NULL");
    expect(html).toContain("no stmt column in the audit table");
    expect(html).toContain("default_catalog / demo");
    expect(html).toContain("Executions\u00a0↓");
  });

  it("lists the top four members with server Profile states and keeps actions disabled until allowed", () => {
    const members = mainMembers({ ...scope, native_digest: "0f1e", catalog: "default_catalog" });
    const html = renderToStaticMarkup(<AuditFamilyAside canDiagnose={false} canPlanCheck={false} error={null} family={families[0]} fetchedQueryIds={new Set()} loading={false} members={members} onClose={() => undefined} onDiagnose={() => undefined} onSelectMember={() => undefined} onShowMembers={() => undefined} profileLoading={false} selectedMemberKey={null} text={english} />);
    expect(html).toContain("top 4 of 412");
    expect(html).toContain("7e8c3a91…4d73");
    expect(html).toContain("1.10 B rows");
    expect(html).toContain("— missing");
    expect(html).toContain("Profile fetched");
    expect(html).toContain("Not fetchable yet");
    expect(html).toMatch(/<button class="button accent primary" disabled="" type="button">Fetch Profile and diagnose/);
    expect(html).toMatch(/<button class="button secondary" disabled="" type="button">Set as baseline/);
    expect(html).toContain("SELECT customer_id,\n       SUM(amount)");
  });

  it("turns the baseline button into compare once a baseline from this list is set", () => {
    const members = mainMembers({ ...scope, native_digest: '0f1e', catalog: 'default_catalog' });
    const aside = (baseline: 'none' | 'selected' | 'comparable' | 'elsewhere') => renderToStaticMarkup(<AuditFamilyAside baseline={baseline} canDiagnose canPlanCheck error={null} family={families[0]} fetchedQueryIds={new Set()} loading={false} members={members} onClose={() => undefined} onCompareWithBaseline={() => undefined} onDiagnose={() => undefined} onSelectMember={() => undefined} onSetBaseline={() => undefined} onShowMembers={() => undefined} profileLoading={false} selectedMemberKey={null} text={english} />);
    expect(aside('none')).toContain('>Set as baseline</button>');
    expect(aside('selected')).toContain('<button class="button secondary" disabled="" type="button">Baseline set</button>');
    expect(aside('comparable')).toContain('>Compare with baseline</button>');
    expect(aside('comparable')).not.toContain('>Set as baseline</button>');
    expect(aside('elsewhere')).toContain('title="The current baseline is not in this list; this execution would replace it"');
  });

  it("labels fingerprint groups unofficial, shows only templates and server counts, and never turns a missing metric into 0", () => {
    const metric = (metric_id: "query_time" | "scan_bytes", value: string | null) => value === null
      ? { metric_id, input: { state: "sql_null" as const }, state: "missing" as const, parser_state: "missing" as const, canonical_number: null, canonical_unit: null }
      : { metric_id, input: { state: "text" as const, value }, state: "exact" as const, parser_state: "exact" as const, canonical_number: { kind: "integer" as const, value: `${value}000000`, scale: null }, canonical_unit: "ns" };
    const response: AuditFingerprintFamilyResponse = {
      schema_version: "audit-workload.v1", connection_id: "c", scope, cluster_identity: { state: "session_scoped", connection_id: "c", detected_version: "3.5.14" },
      audit_timestamp_time_zone: null, authority: "product_unofficial", fingerprint_version: "sr-doctor-fingerprint.v1",
      rows: [{
        family_key: "audit-fingerprint-v1:ab", fingerprint: "ab", template: "SELECT id FROM t WHERE id IN (?)",
        context: { cluster: "session:c", engine_version: "3.5.14", catalog: "default_catalog", database: "demo" },
        execution_count: "2", error_count: "1", error_proportion: { state: "exact", numerator: "1", denominator: "2" },
        aggregates: [
          { aggregation: "sum", metric: metric("query_time", "20"), source_nonnegative_count: "2" },
          { aggregation: "sum", metric: metric("scan_bytes", null), source_nonnegative_count: "0" },
          { aggregation: "max", metric: metric("query_time", "15"), source_nonnegative_count: "2" },
        ],
      }],
      coverage: { eligible_event_count: "9", read_limit: 500, read_event_count: "4", source: { state: "partial", reason: "read_limit" }, fingerprinted_event_count: "2", partial_event_count: "1", unavailable: [{ reason: "massive_compound_predicate", count: "1" }], family_count: "1" },
      top: { requested_limit: 20, returned_count: 1, full_window_count: "1", truncated: false }, limitations: [],
    };
    const html = renderToStaticMarkup(<AuditFingerprintFamilyList error={null} loading={false} response={response} text={english} />);
    expect(html).toContain("Unofficial fingerprint");
    expect(html).toContain("SELECT id FROM t WHERE id IN (?)");
    expect(html).toContain("Newest 4 of 9 executions read");
    expect(html).toContain("truncated 1");
    expect(html).toContain("16+ AND/OR terms 1");
    expect(html).toContain("Missing");
    // Rows are not selectable; the only control is the template's own Expand.
    expect(html).not.toContain("aria-pressed");
    expect(html).toContain('aria-expanded="false"');
  });
});

describe("query family SQL reading", () => {
  it("shows the summary's own SQL as its title with an Expand control, and an empty stmt as a labelled state", () => {
    const sample = { ...families[0], statement_sample: { state: "text" as const, value: "SELECT id FROM t WHERE name = 'zzz'", truncated: true } };
    const empty = { ...families[1], statement_sample: { state: "text" as const, value: "  ", truncated: false } };
    const html = renderToStaticMarkup(<AuditFamilyList families={[sample, empty]} familyTop={null} onSelect={() => undefined} selectedKey={null} sort="execution_count_desc" text={english} />);
    expect(html).toContain(`title="SELECT id FROM t WHERE name = &#x27;zzz&#x27;"`);
    expect(html).toContain("zzz&#x27;…</div>");
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain("audited stmt is empty");
  });

  it("marks a truncated aside sample, copies only the returned prefix and offers a member's full SQL", () => {
    const members = mainMembers({ ...scope, native_digest: "0f1e", catalog: "default_catalog" });
    const family = { ...families[0], statement_sample: { state: "text" as const, value: "SELECT 1", truncated: true } };
    const props = { canDiagnose: false, canPlanCheck: false, error: null, family, fetchedQueryIds: new Set<string>(), loading: false, members, onClose: () => undefined, onDiagnose: () => undefined, onSelectMember: () => undefined, onShowMembers: () => undefined, profileLoading: false, text: english };
    const closed = renderToStaticMarkup(<AuditFamilyAside {...props} onViewMemberSql={() => undefined} selectedMemberKey={null} />);
    expect(closed).toContain("sample truncated (prefix only)");
    expect(closed).toContain(">Copy returned prefix</button>");
    expect(closed).toMatch(/<button aria-expanded="false" class="button secondary" disabled="" type="button">View member full SQL<\/button>/);
    const open = renderToStaticMarkup(<AuditFamilyAside {...props} memberSql={"SELECT full\nFROM t"} onViewMemberSql={() => undefined} selectedMemberKey="k" />);
    expect(open).toContain('aria-controls="audit-family-member-sql"');
    expect(open).toContain('id="audit-family-member-sql"');
    expect(open).toContain("SELECT full\nFROM t");
  });
});
