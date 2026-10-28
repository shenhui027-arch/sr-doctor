import { afterEach, describe, expect, it, vi } from "vitest";
import type { OfficialProfileReadRequest, OfficialProfileReadResponse } from "../../../contracts/generated/doctor-contracts";
import { sr3514JsonMinimal } from "../../../contracts/generated/contract-fixtures";
import { sr3514ExportAnalysis } from "../../../contracts/generated/export-contract-fixtures";
import { sr3514ConnectedContractGolden } from "../../../contracts/generated/connected-contract-fixtures";
import { DoctorApi } from "./api";

const capabilities = {
  api_version: "v0.1",
  schema_version: "1.0",
  supported_inputs: ["json_profile"],
  version_packs: [],
  rule_sets: [],
  limits: {},
  csrf: { nonce: "nonce-from-capabilities", required_when_origin_present: true },
  build: { git_commit: "test", candidate_label: "test" },
};

describe("DoctorApi", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("posts one Overview batch and an explicitly selected historical point with exact time and generation", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ discovery_generation: 8 }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ discovery_generation: 9 }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const scope = { window: { start_seconds: 100, end_seconds: 200 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" as const };
    const controller = new AbortController();
    await api.readAuditOverview({ connection_id: "connection/one", request: { scope, primary: { state: "occurrences", sort: "query_time_desc", limit: 50 }, family_top_sort: "cumulative_query_time_desc" } }, controller.signal);
    const point = { connection_id: "connection/one", discovery_generation: 8, scope, at_milliseconds: "152001" };
    await api.readAuditActiveAt(point, controller.signal);
    expect(request.mock.calls.map((call) => call[0])).toEqual(["/api/v1/capabilities", "/api/v1/connections/connection%2Fone/audit/overview", "/api/v1/connections/connection%2Fone/audit/active-at"]);
    expect(request.mock.calls[2][1].body).toBe(JSON.stringify(point));
    expect(request.mock.calls[2][1].signal).toBe(controller.signal);
    expect(request.mock.calls[2][1].headers["X-Doctor-CSRF"]).toBe("nonce-from-capabilities");
    expect(request.mock.calls.some((call) => /profiles\/fetch|queries\/sql|runtime/.test(call[0]))).toBe(false);
  });

  it.each([200, 409])("preserves family detail identity and makes one explicit request (HTTP %s)", async (status) => {
    const scope = { window: { start_seconds: 100, end_seconds: 200 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" as const };
    const detail = { connection_id: "connection/one", discovery_generation: 7, scope, family_key: "opaque-family-key" };
    const response = status === 200 ? { connection_id: detail.connection_id, discovery_generation: 8, scope, family: { state: "exact_native_digest", family_key: detail.family_key }, distribution: {}, representatives: {}, profile_evidence_coverage: {}, limitations: [] }
      : { code: "query_discovery_generation_expired", stage: "query_source", message_id: "query_discovery_generation_expired", recoverability: "change_input", details: {} };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(response), { status }));
    const api = new DoctorApi("/api/v1", request);
    const controller = new AbortController();
    const result = api.readAuditFamilyDetail(detail, controller.signal);
    if (status === 200) await expect(result).resolves.toStrictEqual(response);
    else await expect(result).rejects.toStrictEqual(response);
    expect(request.mock.calls.map((call) => call[0])).toEqual([
      "/api/v1/capabilities", "/api/v1/connections/connection%2Fone/audit/family-detail",
    ]);
    expect(request.mock.calls[1][1]).toMatchObject({
      method: "POST", body: JSON.stringify(detail), signal: controller.signal,
      headers: { "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" },
    });
  });

  it.each([
    ["empty body", null], ["wrong connection", { connection_id: "other" }],
    ["stale generation", { discovery_generation: 7 }], ["fractional generation", { discovery_generation: 8.5 }],
    ["wrong family", { family: { state: "exact_native_digest", family_key: "other" } }],
    ["ungrouped", { family: { state: "ungrouped" } }],
    ["missing distribution", { distribution: null }], ["missing representatives", { representatives: null }],
  ])("rejects a family detail %s without retry or implicit evidence fetching", async (_label, patch) => {
    const scope = { window: { start_seconds: 100, end_seconds: 200 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" as const };
    const detail = { connection_id: "connection/one", discovery_generation: 7, scope, family_key: "opaque-family-key" };
    const body = patch === null ? null : { connection_id: detail.connection_id, discovery_generation: 8, scope,
      family: { state: "exact_native_digest", family_key: detail.family_key }, distribution: {}, representatives: {}, profile_evidence_coverage: {}, limitations: [], ...patch };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(body), { status: 200 }));
    await expect(new DoctorApi("/api/v1", request).readAuditFamilyDetail(detail)).rejects.toMatchObject({ code: "invalid_audit_family_detail_response", recoverability: "change_input" });
    expect(request).toHaveBeenCalledTimes(2);
    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/connection%2Fone/audit/family-detail");
  });

  it("reads running candidates then one selected snapshot without executing SQL or polling", async () => {
    const golden = sr3514ConnectedContractGolden;
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(golden.running_profile_candidate_list_response), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(golden.running_profile_snapshot_response), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const controller = new AbortController();
    const candidates = { ...golden.running_profile_candidate_list_request, connection_id: "connection/one" };
    const snapshot = { ...golden.running_profile_snapshot_request, connection_id: "connection/one" };
    await expect(api.readRunningProfileCandidates(candidates, controller.signal)).resolves.toStrictEqual(golden.running_profile_candidate_list_response);
    expect(request).toHaveBeenCalledTimes(2);
    await expect(api.readRunningProfileSnapshot(snapshot, controller.signal)).resolves.toStrictEqual(golden.running_profile_snapshot_response);
    expect(request.mock.calls.map((call) => call[0])).toEqual([
      "/api/v1/capabilities", "/api/v1/connections/connection%2Fone/profiles/running-candidates",
      "/api/v1/connections/connection%2Fone/profiles/running-snapshot",
    ]);
    for (const [index, body] of [[1, candidates], [2, snapshot]] as const) {
      expect(request.mock.calls[index][1]).toMatchObject({
        method: "POST", body: JSON.stringify(body), signal: controller.signal,
        headers: { "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" },
      });
    }
  });

  it("returns stale running snapshot failure without rediscovery or retry", async () => {
    const failure = { code: "profile_unavailable", stage: "query_source", message_id: "profile_unavailable", recoverability: "change_input", details: { limitation: "query_discovery_generation_expired" } };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(failure), { status: 409 }));
    await expect(new DoctorApi("/api/v1", request).readRunningProfileSnapshot(sr3514ConnectedContractGolden.running_profile_snapshot_request)).rejects.toStrictEqual(failure);
    expect(request).toHaveBeenCalledTimes(2);
    expect(request.mock.calls[1][0]).toMatch(/\/profiles\/running-snapshot$/);
  });



  for (const execution of [
    {
      name: "standard Profile",
      path: "/connections/c1/runtime-profile",
      invoke: (api: DoctorApi) => api.analyzeRuntimeProfile({ connection_id: "c1", candidate_sql: "SELECT 1" }),
    },
  ]) {
    it.each([
      ["empty", new Response(null, { status: 200 })],
      ["truncated JSON", new Response('{"state"', { status: 200 })],
      ["missing key fields", new Response(JSON.stringify({ state: "fetched" }), { status: 200 })],
    ])(`keeps the ${execution.name} %s execution result unknown without replay`, async (_kind, response) => {
      const request = vi.fn()
        .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
        .mockResolvedValueOnce(response);

      await expect(execution.invoke(new DoctorApi("/api/v1", request))).rejects.toStrictEqual({
        code: "execution_response_unknown", stage: "query_execution", message_id: "execution_response_unknown", recoverability: "status_unknown", details: {},
      });
      expect(request.mock.calls.map(call => call[0])).toEqual(["/api/v1/capabilities", `/api/v1${execution.path}`]);
    });
  }

  it.each([
    {
      name: "standard Profile null state and report fields",
      path: "/connections/c1/runtime-profile",
      invoke: (api: DoctorApi) => api.analyzeRuntimeProfile({ connection_id: "c1", candidate_sql: "SELECT 1" }),
      payload: { state: null, input_origin: null, outcome: null },
    },
  ])("keeps invalid typed $name execution result unknown without replay", async ({ path, invoke, payload }) => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(payload), { status: 200 }));

    await expect(invoke(new DoctorApi("/api/v1", request))).rejects.toStrictEqual({
      code: "execution_response_unknown", stage: "query_execution", message_id: "execution_response_unknown", recoverability: "status_unknown", details: {},
    });
    expect(request.mock.calls.map(call => call[0])).toEqual(["/api/v1/capabilities", `/api/v1${path}`]);
  });


  it.each([new TypeError("connection lost after submit"), new DOMException("Cancelled", "AbortError")])("does not replay standard execution after ambiguous transport failure %s", async failure => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockRejectedValueOnce(failure);
    const body = { connection_id: "c1", candidate_sql: "SELECT 1" };
    const result = new DoctorApi("/api/v1", request).analyzeRuntimeProfile(body);
    await expect(result).rejects.toStrictEqual({ code: "execution_response_unknown", stage: "query_execution", message_id: "execution_response_unknown", recoverability: "status_unknown", details: {} });
    expect(request.mock.calls.map(call => call[0])).toEqual([
      "/api/v1/capabilities", "/api/v1/connections/c1/runtime-profile",
    ]);
    expect(JSON.parse(request.mock.calls[1][1].body)).toStrictEqual(body);
  });

  it("keeps a capability abort before standard execution outside the unknown-response lock", async () => {
    const abort = new DOMException("capabilities cancelled", "AbortError");
    const request = vi.fn().mockRejectedValueOnce(abort);

    await expect(new DoctorApi("/api/v1", request).analyzeRuntimeProfile({
      connection_id: "c1", candidate_sql: "SELECT 1",
    })).rejects.toBe(abort);
    expect(request.mock.calls.map(call => call[0])).toEqual(["/api/v1/capabilities"]);
  });

  it("preserves a certified standard preflight rejection without replay", async () => {
    const failure = {
      code: "statement_not_allowed", stage: "security", message_id: "statement_not_allowed", recoverability: "change_input", details: {},
    };
    const body = { connection_id: "c1", candidate_sql: "DELETE FROM t" };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(failure), { status: 422 }));

    await expect(new DoctorApi("/api/v1", request).analyzeRuntimeProfile(body)).rejects.toStrictEqual(failure);
    expect(request.mock.calls.map(call => call[0])).toEqual([
      "/api/v1/capabilities", "/api/v1/connections/c1/runtime-profile",
    ]);
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("calls the browser fetch function without rebinding its receiver", async () => {
    const request = vi.fn().mockResolvedValue(new Response(JSON.stringify(capabilities), { status: 200 }));
    vi.stubGlobal("fetch", request);

    await new DoctorApi("/api/v1").capabilities();

    expect(request).toHaveBeenCalledWith("/api/v1/capabilities", expect.any(Object));
  });

  it("loads a nonce into memory and sends it for JSON writes", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);

    await api.capabilities();
    await api.postJson("/analyses", { source_kind: "auto", content: "profile" });

    expect(request.mock.calls[1][0]).toBe("/api/v1/analyses");
    expect(request.mock.calls[1][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities", "Content-Type": "application/json" });
  });

  it("releases a connection on unload with one keepalive CSRF DELETE and ignores its failure", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockRejectedValueOnce(new TypeError("page unloaded"));
    const api = new DoctorApi("/api/v1", request);

    api.releaseConnectionOnUnload("conn/1");
    expect(request).not.toHaveBeenCalled();

    await api.capabilities();
    expect(() => api.releaseConnectionOnUnload("conn/1")).not.toThrow();
    await Promise.resolve();

    expect(request).toHaveBeenCalledTimes(2);
    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn%2F1");
    expect(request.mock.calls[1][1]).toMatchObject({ method: "DELETE", keepalive: true, headers: { "X-Doctor-CSRF": "nonce-from-capabilities" } });
  });

  it("keeps multipart content-type browser-managed while still sending CSRF", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const form = new FormData();
    form.append("profile", new Blob(["profile"]), "profile.txt");

    await api.postForm("/analyses", form);

    expect(request.mock.calls[1][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities" });
    expect(request.mock.calls[1][1].headers).not.toHaveProperty("Content-Type");
  });

  it("unwraps the backend-owned analysis report without retaining source input", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ meta: { request_id: "local" }, outcome: { report: sr3514JsonMinimal } }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);

    const report = await api.analyzeJson({ source_kind: "auto", content: "profile" });

    expect(report).toStrictEqual(sr3514JsonMinimal);
    expect(request.mock.calls[1][0]).toBe("/api/v1/analyses");
    expect(request.mock.calls[1][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities", "Content-Type": "application/json" });
  });

  it("posts the raw-pair multipart comparison contract and unwraps the backend comparison", async () => {
    const comparison = { comparison_key: "backend-owned", state: "not_comparable" };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ meta: { request_id: "local" }, comparison }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const form = new FormData();
    form.append("baseline_profile", new Blob(["baseline"]), "baseline.txt");
    form.append("baseline_source_kind", "text_profile");
    form.append("candidate_profile", new Blob(["candidate"]), "candidate.txt");
    form.append("candidate_source_kind", "text_profile");

    const result = await api.compareForm(form);

    expect(result).toStrictEqual(comparison);
    expect(request.mock.calls[1][0]).toBe("/api/v1/comparisons");
    expect(request.mock.calls[1][1].body).toBe(form);
    expect(request.mock.calls[1][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities" });
    expect(request.mock.calls[1][1].headers).not.toHaveProperty("Content-Type");
  });

  it("rejects a malformed successful comparison response", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ meta: { request_id: "local" } }), { status: 200 }));

    await expect(new DoctorApi("/api/v1", request).compareForm(new FormData())).rejects.toMatchObject({ code: "request_failed" });
  });

  it("uses generated connection request and session shapes with the nonce", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ connection_id: "opaque", state: "ready", idle_timeout_seconds: 900, detected_version: null, time_zone: null, capabilities: { version_probe: "available", time_zone: "unknown", audit: "unknown", profile_list: "available", profile_fetch: "available", explain_costs: "available", plan_advisor: "unknown" }, limitations: [] }), { status: 200 }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    const api = new DoctorApi("/api/v1", request);

    const session = await api.createConnection({ host: "127.0.0.1", port: 9030, username: "readonly", password: "input-only", catalog: null, database: null, audit_table: null, tls_mode: "docker_loopback_plaintext" });
    await api.closeConnection(session.connection_id);

    expect(session.state).toBe("ready");
    expect(request.mock.calls[1][0]).toBe("/api/v1/connections");
    expect(request.mock.calls[1][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities" });
    expect(request.mock.calls[2][0]).toBe("/api/v1/connections/opaque");
    expect(request.mock.calls[2][1]).toMatchObject({ method: "DELETE" });
    expect(request.mock.calls[2][1].headers).toMatchObject({ "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" });
  });

  it("uses only fixed connected query, SQL-inspection, and profile-fetch routes", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.query_response), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.query_sql_response), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.profile_fetch_response), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);

    await api.searchConnectedQueries(sr3514ConnectedContractGolden.query_request);
    await api.fetchConnectedQuerySql(sr3514ConnectedContractGolden.query_sql_request);
    await api.fetchConnectedProfile(sr3514ConnectedContractGolden.profile_fetch_request);

    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn_opaque_3514/queries/search");
    expect(request.mock.calls[2][0]).toBe("/api/v1/connections/conn_opaque_3514/queries/sql");
    expect(request.mock.calls[3][0]).toBe("/api/v1/connections/conn_opaque_3514/profiles/fetch");
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(sr3514ConnectedContractGolden.query_request));
    expect(request.mock.calls[2][1].body).toBe(JSON.stringify(sr3514ConnectedContractGolden.query_sql_request));
    expect(request.mock.calls[3][1].body).toBe(JSON.stringify(sr3514ConnectedContractGolden.profile_fetch_request));
    expect(request.mock.calls[1][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities" });
    expect(request.mock.calls[2][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities" });
    expect(request.mock.calls[3][1].headers).toMatchObject({ "X-Doctor-CSRF": "nonce-from-capabilities" });
  });

  it("reads Advisor guides with one fixed read-only GET and rejects a malformed body", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.advisor_response), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ capability: "available" }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ code: "capability_unavailable", stage: "query_source", message_id: "capability_unavailable", recoverability: "change_input", details: {} }), { status: 409 }));
    const api = new DoctorApi("/api/v1", request);
    const controller = new AbortController();

    await expect(api.readAdvisorGuides(sr3514ConnectedContractGolden.advisor_request, controller.signal)).resolves.toEqual(sr3514ConnectedContractGolden.advisor_response);
    expect(request.mock.calls[0][0]).toBe("/api/v1/connections/conn_opaque_3514/advisor-guides");
    expect(request.mock.calls[0][1]).toEqual({ headers: { Accept: "application/json" }, signal: controller.signal });
    await expect(api.readAdvisorGuides(sr3514ConnectedContractGolden.advisor_request)).rejects.toMatchObject({ code: "request_failed" });
    await expect(api.readAdvisorGuides(sr3514ConnectedContractGolden.advisor_request)).rejects.toMatchObject({ code: "capability_unavailable" });
    // A GET needs no nonce, so it never triggers a capability write-preflight.
    expect(request).toHaveBeenCalledTimes(3);
    expect(request.mock.calls.every((call) => call[1].method === undefined && call[1].body === undefined)).toBe(true);
  });

  it("uses only the fixed explicit audit SQL route with the generated request body", async () => {
    const scope = { window: { start_seconds: 100, end_seconds: 200 }, state: "finished" as const, user: "audit-user", native_digest: null, catalog: "demo", database: "demo", resource_group: null, statement_filter: "all" as const };
    const sql = { connection_id: "conn_opaque_3514", discovery_generation: 7, scope, query_id: "00000000-0000-0000-0000-000000000001" };
    const controller = new AbortController();
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ query_id: sql.query_id, discovery_generation: 7, sql: "SELECT server_owned_text" }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);

    await api.readAuditSql(sql, controller.signal);

    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn_opaque_3514/audit/sql");
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(sql));
    expect(request.mock.calls[1][1].signal).toBe(controller.signal);
    expect(request.mock.calls.slice(1).map((call) => call[0]).join(" ")).not.toContain("SELECT");
    expect(request.mock.calls.slice(1).every((call) => call[1].headers["X-Doctor-CSRF"] === "nonce-from-capabilities")).toBe(true);
  });

  it("posts only generated connected query references to the fixed server-side comparison route", async () => {
    const comparison = sr3514ConnectedContractGolden.profile_comparison_response;
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(comparison), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const body = sr3514ConnectedContractGolden.profile_comparison_request;

    await expect(api.compareConnectedProfiles(body)).resolves.toStrictEqual(comparison);

    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn_opaque_3514/profiles/compare");
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(body));
    expect(request.mock.calls[1][1].body).not.toMatch(/raw|profile_text|report|analysis_key|source_hash/i);
    expect(request.mock.calls[1][1].headers).toMatchObject({ "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" });
  });

  it("forwards an AbortSignal for a connected comparison request", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.profile_comparison_response), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const controller = new AbortController();

    await api.compareConnectedProfiles(sr3514ConnectedContractGolden.profile_comparison_request, controller.signal);

    expect(request.mock.calls[1][1].signal).toBe(controller.signal);
  });

  it.each([{ nodeIds: [] }, { nodeIds: [27, 0] }])("reads official text once with exact identity and node filter $nodeIds", async ({ nodeIds }) => {
    const readingRequest: OfficialProfileReadRequest = {
      connection_id: "connection/one", query_id: "00000000-0000-0000-0000-000000003514",
      identity: { kind: "discovery", discovery_generation: 7 }, node_ids: nodeIds,
    };
    const readingResponse: OfficialProfileReadResponse = { identity: readingRequest.identity,
      kind: "analyze_profile_text",
      query: { ...sr3514ConnectedContractGolden.query_response.queries[0], connection_id: readingRequest.connection_id, query_id: readingRequest.query_id, discovery_generation: 7 },
      detected_version: { value: "3.5.14", source_ref: "connection:current_version" },
      node_ids: nodeIds,
      read_at_utc: "2026-09-06T00:00:00Z", profile_observed_at_utc: null,
      source_hash: "backend-owned-source-hash", text: "\u001b[38;2;250;128;114m<script>plain text only</script>\u001b[0m\n  JOIN node 27\r",
      display_text: "<script>plain text only</script>\n  JOIN node 27\\u{000D}",
      display_hash: "backend-owned-display-hash", display_normalization: "ansi_sgr_strip_controls_escape_v1",
      source_refs: ["analyze_profile:official_text"], limitations: ["profile_observation_time_unknown"],
    };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(readingResponse), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const controller = new AbortController();

    await expect(api.readOfficialProfile(readingRequest, controller.signal)).resolves.toStrictEqual(readingResponse);

    expect(request.mock.calls.map((call) => call[0])).toEqual([
      "/api/v1/capabilities", "/api/v1/connections/connection%2Fone/profiles/official-analysis",
    ]);
    expect(request.mock.calls[0][1].signal).toBe(controller.signal);
    expect(request.mock.calls[1][1]).toMatchObject({
      method: "POST", body: JSON.stringify(readingRequest), signal: controller.signal,
      headers: { Accept: "application/json", "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" },
    });
    expect(readingRequest.node_ids).toEqual(nodeIds);
  });

  it.each([
    { status: 422, code: "official_profile_unavailable", stage: "analyze_profile", message: "official_profile_permission_denied", limitation: null },
    { status: 409, code: "profile_unavailable", stage: "query_source", message: "profile_unavailable", limitation: "query_discovery_generation_expired" },
    { status: 502, code: "connection_failed", stage: "analyze_profile", message: "connection_failed", limitation: "official_profile_output_budget_exceeded" },
  ])("preserves official reading error $status without retry or another analysis method", async ({ status, code, stage, message, limitation }) => {
    const failure = {
      code, stage, message_id: message, recoverability: status === 502 ? "restart" : "change_input",
      details: limitation ? { limitation } : {},
    };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ...failure, text: "discard-error-text-canary" }), { status }));
    const api = new DoctorApi("/api/v1", request);
    const readingRequest: OfficialProfileReadRequest = {
      connection_id: "opaque", query_id: "00000000-0000-0000-0000-000000003514", identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [],
    };

    await expect(api.readOfficialProfile(readingRequest)).rejects.toStrictEqual(failure);

    expect(request.mock.calls.map((call) => call[0])).toEqual([
      "/api/v1/capabilities", "/api/v1/connections/opaque/profiles/official-analysis",
    ]);
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(readingRequest));
  });

  it.each(["capabilities", "official-analysis"])("propagates an abort during %s without retry or fallback", async (abortStage) => {
    const controller = new AbortController();
    const abort = new DOMException("reading cancelled", "AbortError");
    const request = vi.fn().mockImplementation((input: string, init: RequestInit) => {
      if (abortStage === "official-analysis" && input.endsWith("/capabilities")) {
        return Promise.resolve(new Response(JSON.stringify(capabilities), { status: 200 }));
      }
      expect(init.signal).toBe(controller.signal);
      return new Promise<Response>((_resolve, reject) => {
        init.signal!.addEventListener("abort", () => reject(abort), { once: true });
      });
    });
    const api = new DoctorApi("/api/v1", request);
    if (abortStage === "official-analysis") await api.capabilities();
    const reading = api.readOfficialProfile({
      connection_id: "opaque", query_id: "00000000-0000-0000-0000-000000003514", identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [27],
    }, controller.signal);
    const rejected = expect(reading).rejects.toBe(abort);
    controller.abort();
    await rejected;

    expect(request.mock.calls.map((call) => call[0])).toEqual(abortStage === "capabilities"
      ? ["/api/v1/capabilities"]
      : ["/api/v1/capabilities", "/api/v1/connections/opaque/profiles/official-analysis"]);
  });

  it.each(["capabilities", "official-analysis"])("propagates cancellation while reading the %s response body", async (abortStage) => {
    const controller = new AbortController();
    const abort = new DOMException("body reading cancelled", "AbortError");
    let bodyReadStarted!: () => void;
    const bodyStarted = new Promise<void>((resolve) => { bodyReadStarted = resolve; });
    const response = new Response(new ReadableStream<Uint8Array>({
      start(stream) {
        controller.signal.addEventListener("abort", () => stream.error(abort), { once: true });
      },
    }), { status: 200 });
    const readJson = response.json.bind(response);
    vi.spyOn(response, "json").mockImplementation(() => {
      bodyReadStarted();
      return readJson();
    });
    const request = vi.fn();
    if (abortStage === "official-analysis") {
      request.mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }));
    }
    request.mockResolvedValueOnce(response);
    const api = new DoctorApi("/api/v1", request);
    const reading = api.readOfficialProfile({
      connection_id: "opaque", query_id: "00000000-0000-0000-0000-000000003514", identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [27],
    }, controller.signal);
    const rejected = expect(reading).rejects.toBe(abort);
    await bodyStarted;
    controller.abort();
    await rejected;

    expect(response.bodyUsed).toBe(true);
    expect(request.mock.calls.map((call) => call[0])).toEqual(abortStage === "capabilities"
      ? ["/api/v1/capabilities"]
      : ["/api/v1/capabilities", "/api/v1/connections/opaque/profiles/official-analysis"]);
    expect(request.mock.calls.every((call) => call[1].signal === controller.signal)).toBe(true);
  });

  it.each([new SyntaxError("invalid JSON"), new TypeError("unreadable body")])("keeps the existing non-abort body failure mapping for %s", async (error) => {
    const response = new Response(null, { status: 422 });
    vi.spyOn(response, "json").mockRejectedValue(error);
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(response);
    const api = new DoctorApi("/api/v1", request);

    await expect(api.readOfficialProfile({
      connection_id: "opaque", query_id: "00000000-0000-0000-0000-000000003514", identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [],
    })).rejects.toStrictEqual({ code: "request_failed", stage: "request", message_id: "request_failed", recoverability: "retry_same", details: {} });
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("keeps the legacy Costs client path on the fixed plan-only route", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.explain_response), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const explainRequest = { input_kind: "user_sql" as const, connection_id: "conn_opaque_3514", mode: "costs" as const, candidate_sql: "SELECT 1" };
    const controller = new AbortController();

    const response = await api.explainCosts(explainRequest, controller.signal);

    expect(response).toStrictEqual(sr3514ConnectedContractGolden.explain_response);
    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn_opaque_3514/explain");
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(explainRequest));
    expect(request.mock.calls[1][1].headers).toMatchObject({ "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" });
    expect(request.mock.calls[1][1].signal).toBe(controller.signal);
    expect(request.mock.calls[1][1].body).not.toContain("EXPLAIN ANALYZE");
    expect(request).toHaveBeenCalledTimes(2);
  });

  it.each([
    ["logical", { input_kind: "user_sql" as const, connection_id: "conn_opaque_3514", mode: "logical" as const, candidate_sql: "SELECT 1" }],
    ["verbose", { input_kind: "plan_sql_reference" as const, connection_id: "conn_opaque_3514", mode: "verbose" as const, query_id: "00000000-0000-0000-0000-000000003514", discovery_generation: 7, plan_sql_ref: "plan-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }],
  ])("posts one %s Explain request without retrying", async (_mode, explainRequest) => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(sr3514ConnectedContractGolden.explain_response), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);

    await expect(api.explain(explainRequest)).resolves.toStrictEqual(sr3514ConnectedContractGolden.explain_response);

    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn_opaque_3514/explain");
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(explainRequest));
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("posts one runtime request to the fixed runtime-profile route without putting SQL in the URL", async () => {
    const attempt = "00000000-0000-4000-8000-000000000045";
    const response = { ...sr3514ConnectedContractGolden.profile_fetch_response, input_origin: "connected_sql_runtime" as const,
      query: { ...sr3514ConnectedContractGolden.profile_fetch_response.query, discovery_generation: null, discovery_source: null },
      official_profile_eligibility: { execution_attempt_id: attempt, method: "standard_profile" } };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(response), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    const runtimeRequest = {
      connection_id: "conn_opaque_3514",
      candidate_sql: "SELECT bucket, COUNT(*) FROM rows GROUP BY bucket",
      execution_attempt_id: attempt,
    };
    const controller = new AbortController();

    await expect(api.analyzeRuntimeProfile(runtimeRequest, controller.signal)).resolves.toStrictEqual(response);

    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/conn_opaque_3514/runtime-profile");
    expect(request.mock.calls[1][0]).not.toContain("SELECT");
    expect(request.mock.calls[1][1].body).toBe(JSON.stringify(runtimeRequest));
    expect(request.mock.calls[1][1].signal).toBe(controller.signal);
    expect(request.mock.calls[1][1].headers).toMatchObject({ "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" });
  });

  it.each(["missing", "attempt", "method", "connection", "query", "generation", "source"])("treats mismatched standard %s as unknown without retrying execution", async (fault) => {
    const attempt = "00000000-0000-4000-8000-000000000045";
    const response = { ...sr3514ConnectedContractGolden.profile_fetch_response, input_origin: "connected_sql_runtime",
      query: { ...sr3514ConnectedContractGolden.profile_fetch_response.query, discovery_generation: null as number | null, discovery_source: null as string | null },
      official_profile_eligibility: { execution_attempt_id: attempt, method: "standard_profile" } as object | undefined };
    if (fault === "missing") response.official_profile_eligibility = undefined;
    if (fault === "attempt") response.official_profile_eligibility = { execution_attempt_id: "other", method: "standard_profile" };
    if (fault === "method") response.official_profile_eligibility = { execution_attempt_id: attempt, method: "native_explain_analyze" };
    if (fault === "connection") response.query.connection_id = "other";
    if (fault === "query") response.query.query_id = "";
    if (fault === "generation") response.query.discovery_generation = 1;
    if (fault === "source") response.query.discovery_source = "profile_list";
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(response), { status: 200 }));
    await expect(new DoctorApi("/api/v1", request).analyzeRuntimeProfile({
      connection_id: "conn_opaque_3514", candidate_sql: "SELECT 1", execution_attempt_id: attempt,
    })).rejects.toStrictEqual({
      code: "execution_response_unknown", stage: "query_execution", message_id: "execution_response_unknown", recoverability: "status_unknown", details: {},
    });
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("downloads the backend export bytes and reads only allowlisted manifest metadata", async () => {
    const body = JSON.stringify(sr3514ExportAnalysis);
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(body, { status: 200, headers: { "Content-Type": "application/json; charset=utf-8" } }));
    const api = new DoctorApi("/api/v1", request);

    const download = await api.exportReport({ kind: "analysis", snapshot: sr3514JsonMinimal });

    expect(download.filename).toBe("sr-doctor-report.json");
    expect(download.kind).toBe("analysis");
    expect(download.manifestHash).toBe(sr3514ExportAnalysis.manifest.manifest_hash);
    expect(await download.blob.text()).toBe(body);
    expect(request.mock.calls[1][0]).toBe("/api/v1/exports");
    expect(request.mock.calls[1][1].headers).toMatchObject({ "Content-Type": "application/json", "X-Doctor-CSRF": "nonce-from-capabilities" });
  });

  it("rejects a malformed successful export without saving or rebuilding it", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ manifest: { manifest_hash: "bad" } }), { status: 200, headers: { "Content-Type": "application/json" } }));

    await expect(new DoctorApi("/api/v1", request).exportReport({ kind: "analysis", snapshot: sr3514JsonMinimal })).rejects.toMatchObject({ code: "invalid_export_response" });
  });

  it("rejects an external API base before any request", () => {
    expect(() => new DoctorApi("https://example.com/api/v1")).toThrow("same-origin");
    expect(() => new DoctorApi("//evil.example/api/v1")).toThrow("same-origin");
    expect(() => new DoctorApi("/\\evil.example/api/v1")).toThrow("same-origin");
  });

  it("maps an empty non-success response to a stable local failure", async () => {
    const request = vi.fn().mockResolvedValue(new Response(null, { status: 404 }));

    await expect(new DoctorApi("/api/v1", request).capabilities()).rejects.toMatchObject({ code: "request_failed" });
  });

  it("preserves the backend allowlisted limitation reason without exposing response text", async () => {
    const failure = {
      request_id: "req-1",
      code: "profile_unavailable",
      stage: "query_source",
      message_id: "profile_unavailable",
      recoverability: "change_input",
      affected_side: null,
      source_refs: [],
      details: { limitation: "query_discovery_generation_expired" },
    };
    const request = vi.fn().mockResolvedValue(new Response(JSON.stringify(failure), { status: 409 }));

    await expect(new DoctorApi("/api/v1", request).capabilities()).rejects.toMatchObject({
      code: "profile_unavailable",
      details: { limitation: "query_discovery_generation_expired" },
    });
  });

  it.each([
    ["wrong connection", { connection_id: "other" }],
    ["official authority", { authority: "official" }],
    ["missing rows", { rows: null }],
  ])("rejects an unofficial fingerprint response with %s", async (_label, patch) => {
    const scope = { window: { start_seconds: 100, end_seconds: 200 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" as const };
    const read = { connection_id: "connection/one", request: { scope, sort: "cumulative_query_time_desc" as const, limit: 20 } };
    const body = { connection_id: read.connection_id, authority: "product_unofficial", rows: [], coverage: {}, top: {}, limitations: [] };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(body), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ...body, ...patch }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    await expect(api.readAuditFingerprintFamilies(read)).resolves.toStrictEqual(body);
    await expect(api.readAuditFingerprintFamilies(read)).rejects.toMatchObject({ code: "invalid_audit_fingerprint_response" });
    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/connection%2Fone/audit/fingerprint-families");
  });

  it("rejects a similar-SQL response bound to another connection or missing its lists", async () => {
    const scope = { window: { start_seconds: 100, end_seconds: 200 }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: "all" as const };
    const read = { connection_id: "connection/one", request: { scope, sort: "cumulative_query_time_desc" as const, limit: 20, dimensions: { tables: true, join: true, predicates: true }, detached_member_keys: [], refined_cluster_keys: [] } };
    const body = { connection_id: read.connection_id, clusters: [], independent: [], unavailable: [], coverage: {}, dimensions: {} };
    const request = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(capabilities), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(body), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ...body, connection_id: "other" }), { status: 200 }));
    const api = new DoctorApi("/api/v1", request);
    await expect(api.readAuditSimilarSql(read)).resolves.toStrictEqual(body);
    await expect(api.readAuditSimilarSql(read)).rejects.toMatchObject({ code: "invalid_audit_similar_response" });
    expect(request.mock.calls[1][0]).toBe("/api/v1/connections/connection%2Fone/audit/similar-sql");
  });
});
