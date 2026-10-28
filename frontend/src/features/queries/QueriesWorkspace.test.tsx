import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { LocaleProvider } from "../../i18n/locale";
import { connectedComparisonRequest, isConnectionExpiredFailure, isCurrentRequest, profileForSelectedQuery, QueriesWorkspace, queryListRequest, queryRowKey, reportForSelectedQuery } from "./QueriesWorkspace";

const noopApi = {
  compareConnectedProfiles: async () => sr3514ConnectedContractGolden.profile_comparison_response,
  fetchConnectedProfile: async () => sr3514ConnectedContractGolden.profile_fetch_response,
  fetchConnectedQuerySql: async () => sr3514ConnectedContractGolden.query_sql_response,
  searchConnectedQueries: async () => sr3514ConnectedContractGolden.query_response,
};

describe("QueriesWorkspace", () => {
  it.each([
    [0, "0 ns"], [1, "1 ms"], [500, "500 ms"], [999, "999 ms"],
    [1000, "1 s"], [1234, "1.234 s"], [null, "—"],
    [Number.MAX_SAFE_INTEGER + 1, "9007199254740992 ms"],
  ])("adapts normalized query duration %s without rounding missing values to zero", (milliseconds, expected) => {
    const response = {
      ...sr3514ConnectedContractGolden.query_response,
      queries: [{ ...sr3514ConnectedContractGolden.query_response.queries[0], query_time_ms: milliseconds as number | null }],
    };
    const html = renderToStaticMarkup(<QueriesWorkspace api={noopApi} initialResponse={response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} />);
    expect(html).toContain(`<td>${expected}</td>`);
  });

  it("renders the actual discovery source and typed query fields without inventing SQL or missing workload metrics", () => {
    const html = renderToStaticMarkup(<QueriesWorkspace api={noopApi} initialResponse={sr3514ConnectedContractGolden.query_response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} />);

    expect(html).toContain("AuditLoader");
    // The same short form as the audit history: first 8 … last 4.
    expect(html).toContain('<td title="00000000-0000-0000-0000-000000003514">00000000…3514</td>');
    expect(html).toContain("500 ms");
    expect(html).toContain("Database time zone");
    expect(html).toContain("Etc/UTC");
    expect(html).toContain("Profile availability is not claimed before fetch.");
    expect(html).not.toContain("Scan bytes");
    expect(html).not.toContain("SQL preview");
    expect(html).not.toContain("textarea");
    expect(html).toContain('aria-pressed="true"');
    expect(html).not.toContain("aria-current");
    expect(html).toContain('aria-busy="false"');
    expect(html).toContain("Baseline slot");
    expect(html).toContain("Candidate slot");
    expect(html).toContain("Evidence source");
    expect(html).toContain("Source kind");
    expect(html).not.toContain("Source hash");
    expect(html).toContain("Analysis key");
    // Absent Profile facts keep their text but render as a dashed tag, never as a value.
    expect(html).toContain('<dt>Source kind</dt><dd class="fact-missing"><span class="fact-missing-tag">unavailable</span></dd>');
    expect(html).toContain('<dt>Baseline slot</dt><dd class="fact-missing"><span class="fact-missing-tag">not selected</span></dd>');
    expect(html).toContain("Set baseline");
    expect(html).toContain("Set candidate");
    expect(html).toContain("Plan Check");
    expect(html).toContain("View SQL");
    expect(html).toContain("Fetch profile");
    expect(html).toContain("Open Doctor");
    expect(html).toContain("Compare selected Profiles");
    expect(html).toContain("The product sends only query references and does not execute SQL.");
    expect(JSON.stringify(sr3514ConnectedContractGolden.query_response)).not.toContain('"sql"');
    expect(sr3514ConnectedContractGolden.query_sql_response.sql).toBe("SELECT synthetic_query FROM demo");
  });

  it("asks the backend to hide this tool's statements by default and sends all when unchecked", () => {
    expect(queryListRequest("conn_opaque_3514", " finished ", "", true)).toEqual({ connection_id: "conn_opaque_3514", limit: 100, status: "finished", digest: null, statement_filter: "exclude_doctor_and_non_query" });
    expect(queryListRequest("conn_opaque_3514", "", " a0b1 ", false)).toEqual({ connection_id: "conn_opaque_3514", limit: 100, status: null, digest: "a0b1", statement_filter: "all" });
    const html = renderToStaticMarkup(<QueriesWorkspace api={noopApi} initialResponse={sr3514ConnectedContractGolden.query_response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} />);
    expect(html).toContain('<label class="query-statement-filter"><input type="checkbox" checked=""/>Hide this tool&#x27;s queries and non-query statements</label>');
  });

  it.each([
    [{ state: "applied" as const, hidden_non_query_row_count: "5", hidden_doctor_row_count: "2" }, "已隐藏本工具发出的语句 2 条、非查询语句 5 条（时间窗内的原始审计行，不受列表条数上限影响）。"],
    [{ state: "doctor_only_applied" as const, hidden_doctor_row_count: "0" }, "已隐藏本工具发出的语句 0 条（本次读取的 Profile 列表行内）。Profile 列表没有语句类型列，无法区分非查询语句，它们仍会列出。"],
    [{ state: "indistinguishable" as const, reason: "audit_statement_filter_unavailable" }, "此来源无法区分本工具发出的语句，未隐藏任何行。 审计表缺少该筛选所需的 isQuery 与 stmt 列。"],
    [{ state: "not_applied" as const }, "显示全部语句，未隐藏任何行。"],
  ])("discloses the backend statement-filter outcome %j without recomputing it", (coverage, expected) => {
    const response = { ...sr3514ConnectedContractGolden.query_response, statement_filter: coverage };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><QueriesWorkspace api={noopApi} initialResponse={response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} /></LocaleProvider>);
    expect(html).toContain(`<p class="query-statement-filter-evidence" data-query-statement-filter="${coverage.state}">${expected}</p>`);
  });

  it("renders a fetched server-side report without returning raw connector Profile bytes", () => {
    const html = renderToStaticMarkup(<QueriesWorkspace api={noopApi} initialFetch={sr3514ConnectedContractGolden.profile_fetch_response} initialResponse={sr3514ConnectedContractGolden.query_response} onConnect={() => undefined} onOpenDoctor={() => undefined} session={sr3514ConnectedContractGolden.connection} />);

    expect(html).toContain("Fetched");
    expect(html).toContain("text_profile");
    expect(html).not.toContain("3aeb9a126d70be2f");
    expect(html).toContain("The raw Profile bytes stay on the server.");
    expect(html).not.toContain(">raw_connector_profile_bytes_not_returned_by_connected_fetch_response");
    expect(html).not.toMatch(/disabled[^>]*>Open Doctor<\/button>/);
  });

  it("renders a bilingual reason when Plan Check is unavailable instead of leaving a dead control", () => {
    const query = sr3514ConnectedContractGolden.query_response.queries[0];
    const response = {
      ...sr3514ConnectedContractGolden.query_response,
      queries: [{ ...query, plan_check: { state: "unavailable" as const, reason: "statement_missing_or_not_select" as const } }],
    };
    const english = renderToStaticMarkup(<QueriesWorkspace api={noopApi} initialResponse={response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} />);
    const chinese = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><QueriesWorkspace api={noopApi} initialResponse={response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} /></LocaleProvider>);

    expect(english).toContain("Statement is missing or is not a single SELECT/WITH");
    expect(chinese).toContain("语句缺失或不是单条 SELECT/WITH");
    expect(english).toMatch(/disabled[^>]*title="Statement is missing or is not a single SELECT\/WITH"/);
  });

  it("keeps duplicate canonical query IDs as distinct evidence rows", () => {
    const first = sr3514ConnectedContractGolden.query_response.queries[0];
    const duplicate = {
      ...first,
      source_refs: ["audit:row:1:timestamp", "audit:row:1:epoch"],
      profile_fetch: { state: "unavailable" as const, reason: "query_id_duplicate" as const },
      plan_check: { state: "unavailable" as const, reason: "query_id_duplicate" as const },
    };
    const response = { ...sr3514ConnectedContractGolden.query_response, queries: [first, duplicate] };

    const html = renderToStaticMarkup(<QueriesWorkspace api={noopApi} initialResponse={response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} />);

    expect(queryRowKey(first, 0)).not.toBe(queryRowKey(duplicate, 1));
    expect(html.match(/aria-pressed="true"/g)).toHaveLength(1);
    expect(html.match(/aria-pressed="false"/g)).toHaveLength(1);
  });

  it("opens only the report fetched for the currently selected query", () => {
    const selected = sr3514ConnectedContractGolden.query_response.queries[0];
    const fetched = sr3514ConnectedContractGolden.profile_fetch_response;
    expect(profileForSelectedQuery(selected, fetched)).toBe(fetched);
    expect(reportForSelectedQuery(selected, fetched)).toBe(fetched.outcome.report);
    expect(reportForSelectedQuery({ ...selected, query_id: "different-query" }, fetched)).toBeNull();
    expect(profileForSelectedQuery({ ...selected, connection_id: "other-session" }, fetched)).toBeNull();
    expect(profileForSelectedQuery({ ...selected, discovery_generation: (selected.discovery_generation ?? 0) + 1 }, fetched)).toBeNull();
  });

  it("creates a connected comparison request only for two distinct query references from the ready session", () => {
    const { connection, profile_comparison_request: expected, query_response } = sr3514ConnectedContractGolden;
    const baseline = query_response.queries[0];
    const candidate = { ...baseline, query_id: expected.candidate_query_id };

    expect(connectedComparisonRequest(connection, baseline, candidate)).toStrictEqual(expected);
    expect(connectedComparisonRequest(connection, baseline, baseline)).toBeNull();
    expect(connectedComparisonRequest(connection, { ...baseline, query_id: "aaaaaaaa-0000-0000-0000-000000000000" }, { ...candidate, query_id: "AAAAAAAA-0000-0000-0000-000000000000" })).toBeNull();
    expect(connectedComparisonRequest({ ...connection, state: "unsupported" }, baseline, candidate)).toBeNull();
    expect(connectedComparisonRequest(connection, baseline, { ...candidate, connection_id: "other-session" })).toBeNull();
    expect(connectedComparisonRequest(connection, baseline, { ...candidate, discovery_generation: candidate.discovery_generation + 1 })).toBeNull();
    expect(connectedComparisonRequest(connection, { ...baseline, profile_fetch: { state: "unavailable", reason: "query_not_finished" } }, candidate)).toBeNull();
  });

  it("marks rotated or aborted requests stale before they can replace a query list or open Compare", () => {
    const replaced = new AbortController();
    const current = new AbortController();
    expect(isCurrentRequest(8, 8, replaced.signal)).toBe(true);
    replaced.abort();
    expect(isCurrentRequest(9, 8, replaced.signal)).toBe(false);
    expect(isCurrentRequest(9, 9, current.signal)).toBe(true);
    current.abort();
    expect(isCurrentRequest(9, 9, current.signal)).toBe(false);
  });

  it("recognizes the server-authoritative connection expiry code", () => {
    expect(isConnectionExpiredFailure({ code: "connection_expired", message_id: "connection_expired" })).toBe(true);
    expect(isConnectionExpiredFailure({ code: "profile_unavailable", message_id: "profile_unavailable" })).toBe(false);
  });

  it("keeps the offline state explicit", () => {
    const html = renderToStaticMarkup(<QueriesWorkspace api={noopApi} onConnect={() => undefined} session={null} />);

    expect(html).toContain("Connect to list recent queries and their Profiles");
    expect(html).toContain("no SQL is accepted here");
  });

  it("renders the Chinese Figma copy while preserving evidence identifiers", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><QueriesWorkspace api={noopApi} initialResponse={sr3514ConnectedContractGolden.query_response} onConnect={() => undefined} session={sr3514ConnectedContractGolden.connection} /></LocaleProvider>);

    expect(html).toContain("查询发现");
    expect(html).toContain("开始时间");
    expect(html).toContain("拉取 Profile");
    expect(html).toContain("AuditLoader");
    expect(html).toContain("query_text_omitted_by_contract");
    expect(html).toContain("已完成");
    expect(html).toContain("数据库时区");
    expect(html).toContain("基线槽位");
    expect(html).toContain("候选槽位");
    expect(html).toContain("产品只发送查询引用，不执行 SQL。");
  });
});
