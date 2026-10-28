import { describe, expect, it, vi } from "vitest";
import type { OfficialProfileReadResponse } from "../../../../contracts/generated/doctor-contracts";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { ApiFailure } from "../../lib/api";
import { createOfficialProfileReader, officialResponseMatches, type OfficialBinding } from "./official-profile-read";

const binding: OfficialBinding = { connection_id: "connection-1", query_id: "a5fff049-a91f-11f1-a261-f642d1535403", identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [] };
function response(owner = binding): OfficialProfileReadResponse {
  return { kind: "analyze_profile_text", query: { ...sr3514ConnectedContractGolden.query_response.queries[0],
    connection_id: owner.connection_id, query_id: owner.query_id, discovery_generation: owner.identity.kind === "discovery" ? owner.identity.discovery_generation : null },
    identity: owner.identity, node_ids: [...owner.node_ids], detected_version: null, read_at_utc: "2026-09-06T00:00:00Z", profile_observed_at_utc: null,
    text: "\u001b[31m<script>x</script>\r", source_hash: "original-hash", display_text: "<script>x</script>\\u{000D}",
    display_hash: "display-hash", display_normalization: "ansi_sgr_strip_controls_escape_v1", source_refs: [], limitations: [] };
}
function deferred() {
  let resolve!: (value: OfficialProfileReadResponse) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<OfficialProfileReadResponse>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function error(code: string, recoverability = "retry_same"): ApiFailure {
  return { code, message_id: code, stage: "query_source", recoverability, details: { evidence: "unchanged" } };
}
function harness(initial: OfficialBinding | null = binding) {
  let live = initial;
  const read = vi.fn<(request: unknown, signal?: AbortSignal) => Promise<OfficialProfileReadResponse>>();
  const onSessionExpired = vi.fn();
  const reader = createOfficialProfileReader({ read, getCurrentBinding: () => live, onSessionExpired });
  return { read, reader, onSessionExpired, change(next: OfficialBinding | null) { live = next; } };
}

describe("official profile non-visual reader", () => {
  const standard: OfficialBinding = { ...binding, identity: { kind: "verified_standard_attempt", execution_attempt_id: "00000000-0000-4000-8000-000000000045" } };
  it("freezes standard authority and reads only on an explicit action", async () => {
    const h = harness(standard); h.reader.open();
    expect(h.read).not.toHaveBeenCalled();
    const state = h.reader.snapshot();
    expect(state.status).toBe("unread");
    if (state.status !== "closed") expect(Object.isFrozen(state.binding.identity)).toBe(true);
    h.read.mockResolvedValue(response(standard)); await h.reader.read();
    expect(h.reader.snapshot().status).toBe("success");
    expect(h.read).toHaveBeenCalledTimes(1);
    expect(h.read.mock.calls[0][0]).toEqual(standard);
  });
  it("rejects wrong or mixed response authority even for the same Query ID", () => {
    expect(officialResponseMatches(response(binding), standard)).toBe(false);
    expect(officialResponseMatches({ ...response(standard), identity: { ...standard.identity, discovery_generation: 7 } }, standard)).toBe(false);
    expect(officialResponseMatches({ ...response(standard), identity: { kind: "verified_standard_attempt", execution_attempt_id: "00000000-0000-4000-8000-000000000046" } }, standard)).toBe(false);
  });
  it("does not reuse discovery-generation recovery blocks for a verified execution", async () => {
    const h = harness(); h.reader.open();
    h.read.mockRejectedValueOnce({ ...error("profile_unavailable", "change_input"), details: { limitation: "query_discovery_generation_expired" } });
    await h.reader.read();
    h.change(standard); h.reader.open();
    expect(h.reader.snapshot().status).toBe("unread");
    h.read.mockResolvedValue(response(standard)); await h.reader.read();
    expect(h.reader.snapshot().status).toBe("success");
  });
  it("drops an old standard response after attempt A to B to A", async () => {
    const h = harness(standard); const pending = deferred();
    h.read.mockReturnValue(pending.promise); h.reader.open(); const running = h.reader.read();
    h.change({ ...standard, identity: { kind: "verified_standard_attempt", execution_attempt_id: "00000000-0000-4000-8000-000000000046" } });
    h.reader.invalidate(); h.change(standard); h.reader.open();
    pending.resolve(response(standard)); await running;
    expect(h.reader.snapshot().status).toBe("unread");
    expect(h.read).toHaveBeenCalledTimes(1);
  });
  it("open, scope change and close issue zero reads; explicit read sends the exact binding once", async () => {
    const h = harness();
    h.reader.open();
    const node = { ...binding, node_ids: [27] };
    h.change(node); h.reader.open();
    expect(h.read).not.toHaveBeenCalled();
    h.read.mockResolvedValue(response(node));
    await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
    expect(h.read.mock.calls[0][0]).toEqual(node);
    expect(h.read.mock.calls[0][1]).toBeInstanceOf(AbortSignal);
    h.reader.close();
    expect(h.reader.snapshot()).toEqual({ status: "closed" });
    await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
  });

  it("retains backend raw/display bytes and metadata without parsing, including empty success", async () => {
    const h = harness(); const value = response(); value.text = "";
    h.read.mockResolvedValue(value); h.reader.open(); await h.reader.read();
    const state = h.reader.snapshot();
    expect(state.status).toBe("success");
    if (state.status === "success") expect(state.response).toBe(value);
  });

  it("clears old success synchronously on reread and admits only the latest token", async () => {
    const h = harness(); h.reader.open(); h.read.mockResolvedValueOnce(response()); await h.reader.read();
    const old = deferred(); const latest = deferred();
    h.read.mockReturnValueOnce(old.promise).mockReturnValueOnce(latest.promise);
    const first = h.reader.read();
    expect(h.reader.snapshot()).toEqual({ status: "loading", binding });
    h.reader.invalidate(); h.reader.open();
    const second = h.reader.read();
    expect(h.read.mock.calls[1][1]?.aborted).toBe(true);
    const value = { ...response(), text: "latest" }; latest.resolve(value); await second;
    old.resolve({ ...response(), text: "obsolete" }); await first;
    expect(h.reader.snapshot()).toMatchObject({ status: "success", response: { text: "latest" } });
  });

  it("duplicate read while loading is a no-op, not abort and resend", async () => {
    const h = harness(); const pending = deferred(); h.read.mockReturnValue(pending.promise);
    h.reader.open(); const first = h.reader.read(); await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
    expect(h.read.mock.calls[0][1]?.aborted).toBe(false);
    pending.resolve(response()); await first;
    expect(h.reader.snapshot().status).toBe("success");
  });

  it("host invalidation prevents A to B to A resurrection", async () => {
    const h = harness(); const pending = deferred(); h.read.mockReturnValue(pending.promise);
    h.reader.open(); const first = h.reader.read();
    h.change({ ...binding, identity: { kind: "discovery" as const, discovery_generation: 8 } }); h.reader.invalidate();
    h.change(binding); h.reader.open();
    pending.resolve(response()); await first;
    expect(h.reader.snapshot()).toEqual({ status: "unread", binding });
    expect(h.read.mock.calls[0][1]?.aborted).toBe(true);
  });

  it.each(["close", "invalidate"] as const)("%s aborts and drops late success", async (action) => {
    const h = harness(); const pending = deferred(); h.read.mockReturnValue(pending.promise);
    h.reader.open(); const work = h.reader.read(); h.reader[action]();
    expect(h.read.mock.calls[0][1]?.aborted).toBe(true);
    pending.resolve(response()); await work;
    expect(h.reader.snapshot()).toEqual({ status: "closed" });
  });

  it.each([null, { ...binding, identity: { kind: "discovery" as const, discovery_generation: 8 } }, { ...binding, connection_id: "other" },
    { ...binding, query_id: "other" }, { ...binding, node_ids: [27] }])("hides changed live identity before cleanup: %j", async (next) => {
    const h = harness(); h.read.mockResolvedValue(response()); h.reader.open(); await h.reader.read();
    h.change(next);
    expect(h.reader.snapshot()).toEqual({ status: "closed" });
    await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(1);
  });

  it.each(["close", "identity"])("stale session-expired errors never invoke callbacks after %s", async (action) => {
    const h = harness(); const pending = deferred(); h.read.mockReturnValue(pending.promise);
    h.reader.open(); const work = h.reader.read();
    if (action === "close") h.reader.close(); else h.change({ ...binding, identity: { kind: "discovery" as const, discovery_generation: 8 } });
    pending.reject(error("connection_expired")); await work;
    expect(h.onSessionExpired).not.toHaveBeenCalled();
    expect(h.reader.snapshot()).toEqual({ status: "closed" });
  });

  it("live identity changes also discard late success", async () => {
    const h = harness(); const pending = deferred(); h.read.mockReturnValue(pending.promise);
    h.reader.open(); const work = h.reader.read(); h.change(null);
    pending.resolve(response()); await work;
    expect(h.reader.snapshot()).toEqual({ status: "closed" });
  });

  it.each(["official_profile_empty", "official_profile_permission_denied", "official_profile_lifecycle_unknown", "official_profile_remote_unsupported"])("preserves typed unavailable %s", async (message_id) => {
    const h = harness(); const cause = { ...error("official_profile_unavailable", "change_input"), stage: "analyze_profile", message_id };
    h.read.mockRejectedValue(cause); h.reader.open(); await h.reader.read();
    expect(h.reader.snapshot()).toEqual({ status: "unavailable", binding, error: cause });
    await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
  });

  it.each([
    { ...error("official_profile_unavailable", "change_input"), stage: "analyze_profile", message_id: "official_profile_empty" },
    { ...error("remote_timeout", "restart") },
    { ...error("profile_unavailable", "change_input"), details: { limitation: "query_discovery_generation_expired" } },
  ])("real envelope blocks same binding retry and reopening: %j", async (cause) => {
    const h = harness(); h.read.mockRejectedValueOnce(cause); h.reader.open(); await h.reader.read();
    await h.reader.read(); h.reader.close(); h.reader.open(); await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
    expect(h.reader.snapshot()).toMatchObject({ error: cause });
    const next = cause.recoverability === "restart" ? { ...binding, connection_id: "new-connection" }
      : cause.code === "profile_unavailable" ? { ...binding, identity: { kind: "discovery" as const, discovery_generation: 8 } } : { ...binding, node_ids: [27] };
    h.change(next); h.reader.open(); expect(h.read).toHaveBeenCalledTimes(1);
    h.read.mockResolvedValueOnce(response(next)); await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(2);
    expect(h.reader.snapshot()).toMatchObject({ status: "success", binding: next });
  });

  it.each(["remote_timeout", "connection_failed", "connection_expired"])("restart %s survives scope/query/generation changes until connection replacement", async (code) => {
    const h = harness(); const cause = error(code, "restart");
    h.read.mockRejectedValueOnce(cause); h.reader.open(); await h.reader.read();
    for (const next of [{ ...binding, node_ids: [27] }, { ...binding, query_id: "another-query" },
      { ...binding, identity: { kind: "discovery" as const, discovery_generation: 8 } }, binding]) {
      h.change(next); h.reader.invalidate(); h.reader.open(); await h.reader.read();
      expect(h.reader.snapshot()).toMatchObject({ status: "failed", binding: next, error: cause });
      expect(h.read).toHaveBeenCalledTimes(1);
    }
    const next = { ...binding, connection_id: "replacement" };
    h.change(next); h.reader.open(); h.read.mockResolvedValueOnce(response(next)); await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(2);
    h.change(binding); h.reader.open(); await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(2);
  });

  it("expired discovery requires a fresh generation, not a changed node or query", async () => {
    const h = harness(); const cause = { ...error("profile_unavailable", "change_input"), details: { limitation: "query_discovery_generation_expired" } };
    h.read.mockRejectedValueOnce(cause); h.reader.open(); await h.reader.read();
    const node = { ...binding, node_ids: [27] }; h.change(node); h.reader.open(); await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
    expect(h.reader.snapshot()).toMatchObject({ binding: node, failureBinding: binding, error: cause });
    h.change({ ...binding, query_id: "another" }); h.reader.open(); await h.reader.read();
    expect(h.read).toHaveBeenCalledTimes(1);
    const fresh = { ...binding, identity: { kind: "discovery" as const, discovery_generation: 8 } }; h.change(fresh); h.reader.open();
    h.read.mockResolvedValueOnce(response(fresh)); await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(2);
  });

  it("ordinary change_input permits node scope change but cannot be bypassed by A to B to A", async () => {
    const h = harness(); const cause = { ...error("official_profile_unavailable", "change_input"), message_id: "official_profile_empty", stage: "analyze_profile" };
    h.read.mockRejectedValueOnce(cause); h.reader.open(); await h.reader.read();
    const node = { ...binding, node_ids: [27] }; h.change(node); h.reader.open();
    h.read.mockResolvedValueOnce(response(node)); await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(2);
    h.change(binding); h.reader.open(); await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(2);
    expect(h.reader.snapshot()).toMatchObject({ status: "unavailable", error: cause });
  });

  it("A to B to A invalidation discards late restart without poisoning the new request", async () => {
    const h = harness(); const pending = deferred(); h.read.mockReturnValueOnce(pending.promise);
    h.reader.open(); const old = h.reader.read();
    h.change({ ...binding, node_ids: [27] }); h.reader.invalidate();
    h.change(binding); h.reader.open();
    pending.reject(error("connection_expired", "restart")); await old;
    expect(h.onSessionExpired).not.toHaveBeenCalled();
    h.read.mockResolvedValueOnce(response()); await h.reader.read();
    expect(h.reader.snapshot().status).toBe("success");
  });

  it("session callback invalidation does not lose the accepted restart requirement", async () => {
    const read = vi.fn().mockRejectedValue(error("connection_expired", "restart"));
    const reader = createOfficialProfileReader({ read, getCurrentBinding: () => binding, onSessionExpired: () => reader.invalidate() });
    reader.open(); await reader.read(); reader.open(); await reader.read();
    expect(read).toHaveBeenCalledTimes(1);
  });

  it.each(["official_profile_unavailable", "request_failed"])("retry_same permits only explicit retry for %s", async (code) => {
    const h = harness(); h.read.mockRejectedValueOnce(error(code)).mockResolvedValueOnce(response());
    h.reader.open(); await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(1);
    await h.reader.read(); expect(h.read).toHaveBeenCalledTimes(2);
    expect(h.reader.snapshot().status).toBe("success");
  });

  it.each(["query_discovery_generation_expired", "connection_failed", "budget_exceeded"])("does not reclassify failed %s or retry automatically", async (code) => {
    const h = harness(); const cause = error(code, "change_input"); h.read.mockRejectedValue(cause);
    h.reader.open(); await h.reader.read(); await h.reader.read();
    expect(h.reader.snapshot()).toEqual({ status: "failed", binding, error: cause });
    expect(h.read).toHaveBeenCalledTimes(1);
  });

  it("current session expiration invokes the existing callback exactly once", async () => {
    const h = harness(); h.read.mockRejectedValue({ ...error("connection_failed"), message_id: "connection_session_expired" });
    h.reader.open(); await h.reader.read(); expect(h.onSessionExpired).toHaveBeenCalledTimes(1);
  });

  it("AbortError is silent and unknown transport failures retain the API generic mapping", async () => {
    const h = harness(); h.read.mockRejectedValueOnce({ name: "AbortError" }).mockRejectedValueOnce(new TypeError("network"));
    h.reader.open(); await h.reader.read(); expect(h.reader.snapshot()).toEqual({ status: "unread", binding });
    await h.reader.read(); expect(h.reader.snapshot()).toMatchObject({ status: "failed", error: { code: "request_failed", stage: "request", recoverability: "retry_same" } });
  });

  it.each([null, { ...binding, identity: { kind: "discovery" as const, discovery_generation: 1.5 } }, { ...binding, identity: { kind: "discovery" as const, discovery_generation: 0 } },
    { ...binding, node_ids: [-1] }, { ...binding, node_ids: [2147483648] }, { ...binding, node_ids: [1.5] },
    { ...binding, node_ids: [1, 2] }])("inadmissible host binding has no action: %j", async (value) => {
    const h = harness(value); h.reader.open(); await h.reader.read();
    expect(h.reader.snapshot()).toEqual({ status: "closed" }); expect(h.read).not.toHaveBeenCalled();
  });

  it("snapshots binding arrays rather than retaining caller-mutated request identity", () => {
    const nodes: number[] = []; const h = harness({ ...binding, node_ids: nodes }); h.reader.open();
    nodes.push(27); expect(h.reader.snapshot()).toEqual({ status: "closed" });
  });

  it.each([
    { kind: "raw_profile" }, { query: { ...response().query, discovery_generation: 8 } },
    { query: { ...response().query, connection_id: "other" } }, { query: { ...response().query, query_id: "other" } },
    { node_ids: [27] }, { display_normalization: "unknown" }, { display_text: undefined },
    { display_hash: undefined }, { source_hash: undefined }, { read_at_utc: undefined },
    { profile_observed_at_utc: undefined }, { detected_version: undefined }, { source_refs: [123] }, { limitations: undefined },
  ])("rejects mismatched or incomplete response %j", async (patch) => {
    const h = harness(); const invalid = { ...response(), ...patch };
    expect(officialResponseMatches(invalid, binding)).toBe(false);
    h.read.mockResolvedValue(invalid as OfficialProfileReadResponse); h.reader.open(); await h.reader.read();
    expect(h.reader.snapshot()).toMatchObject({ status: "failed", error: { code: "invalid_official_profile_response" } });
    expect(h.reader.snapshot()).not.toHaveProperty("response");
  });
});
