import { describe, expect, it, vi } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { ConnectedQueryRef, ConnectionSessionView, OfficialProfileReadResponse, RunningProfileCandidateListResponse, RunningProfileSnapshotResponse } from "../../../../contracts/generated/doctor-contracts";
import { createRunningProfileEntryController, type RunningProfileEntryApi } from "./running-profile-entry-controller";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function runningList(generation = 2, candidates?: ConnectedQueryRef[]): RunningProfileCandidateListResponse {
  const fixture = structuredClone(sr3514ConnectedContractGolden.running_profile_candidate_list_response);
  const rows = candidates ?? fixture.candidates.map((candidate) => ({ ...candidate, discovery_generation: generation }));
  return { ...fixture, discovery_generation: generation, returned: rows.length, candidates: rows };
}

function snapshotFor(candidate: ConnectedQueryRef): RunningProfileSnapshotResponse {
  const response = structuredClone(sr3514ConnectedContractGolden.running_profile_snapshot_response);
  return { ...response, query: { ...candidate } };
}

function officialFor(candidate: ConnectedQueryRef): OfficialProfileReadResponse {
  return {
    kind: "analyze_profile_text",
    query: { ...candidate },
    identity: { kind: "discovery", discovery_generation: candidate.discovery_generation! },
    node_ids: [], detected_version: null, read_at_utc: "2026-09-08T00:00:03Z", profile_observed_at_utc: null,
    text: "official profile", source_hash: "official-source", display_text: "official profile", display_hash: "official-display",
    display_normalization: "ansi_sgr_strip_controls_escape_v1", source_refs: [], limitations: [],
  };
}

function harness(listResponses: RunningProfileCandidateListResponse[] = [runningList()]) {
  let session: ConnectionSessionView | null = structuredClone(sr3514ConnectedContractGolden.connection);
  const api: RunningProfileEntryApi = {
    readRunningProfileCandidates: vi.fn(async () => listResponses.shift() ?? runningList(3, [])),
    readRunningProfileSnapshot: vi.fn(async (request) => snapshotFor(runningList(request.discovery_generation).candidates[0])),
    readOfficialProfile: vi.fn(async (request) => officialFor({ ...runningList(request.identity.kind === "discovery" ? request.identity.discovery_generation : 0).candidates[0], query_id: request.query_id })),
  };
  const onDiscoveryChanged = vi.fn();
  const onSessionExpired = vi.fn();
  const controller = createRunningProfileEntryController({ api, getSession: () => session, onDiscoveryChanged, onSessionExpired });
  return { api, controller, onDiscoveryChanged, onSessionExpired, setSession(value: ConnectionSessionView | null) { session = value; } };
}

describe("RunningProfileEntry controller", () => {
  it("asks the server to hide this tool's own statements by default, and can ask for all", async () => {
    const h = harness([runningList(), runningList(3)]);
    await h.controller.readCandidates();
    expect(h.api.readRunningProfileCandidates).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 25, statement_filter: "exclude_doctor_and_non_query" }), expect.anything());
    await h.controller.readCandidates("all");
    expect(h.api.readRunningProfileCandidates).toHaveBeenLastCalledWith(expect.objectContaining({ statement_filter: "all" }), expect.anything());
  });

  it("starts at zero I/O, then keeps bounded list, snapshot, and official reads independent", async () => {
    const h = harness();
    expect(h.api.readRunningProfileCandidates).not.toHaveBeenCalled();
    expect(h.api.readRunningProfileSnapshot).not.toHaveBeenCalled();
    expect(h.api.readOfficialProfile).not.toHaveBeenCalled();

    await h.controller.readCandidates();
    expect(h.api.readRunningProfileCandidates).toHaveBeenCalledTimes(1);
    expect(h.api.readRunningProfileCandidates).toHaveBeenLastCalledWith({ connection_id: "conn_opaque_3514", limit: 25, statement_filter: "exclude_doctor_and_non_query" }, expect.any(AbortSignal));
    expect(h.onDiscoveryChanged).toHaveBeenCalledTimes(1);
    const list = h.controller.snapshot().list;
    const selected = list.status === "success" ? list.response.candidates[0] : null;
    expect(selected).not.toBeNull();
    h.controller.select(selected!);

    await h.controller.readSnapshot();
    expect(h.api.readRunningProfileSnapshot).toHaveBeenCalledTimes(1);
    expect(h.api.readOfficialProfile).not.toHaveBeenCalled();
    expect(h.controller.snapshot().snapshot.status).toBe("success");

    await h.controller.readOfficial();
    expect(h.api.readOfficialProfile).toHaveBeenCalledTimes(1);
    expect(h.api.readRunningProfileSnapshot).toHaveBeenCalledTimes(1);
    expect(h.api.readOfficialProfile).toHaveBeenLastCalledWith({
      connection_id: "conn_opaque_3514",
      query_id: "00000000-0000-0000-0000-000000003515",
      identity: { kind: "discovery", discovery_generation: 2 },
      node_ids: [],
    }, expect.any(AbortSignal));
  });

  it("rejects same-tick duplicate raw or official gestures without resending", async () => {
    const h = harness();
    const raw = deferred<RunningProfileSnapshotResponse>();
    const official = deferred<OfficialProfileReadResponse>();
    h.api.readRunningProfileSnapshot = vi.fn(() => raw.promise);
    h.api.readOfficialProfile = vi.fn(() => official.promise);
    await h.controller.readCandidates();
    const selected = (h.controller.snapshot().list as Extract<ReturnType<typeof h.controller.snapshot>["list"], { status: "success" }>).response.candidates[0];
    h.controller.select(selected);

    const firstSnapshot = h.controller.readSnapshot();
    const secondSnapshot = h.controller.readSnapshot();
    expect(h.api.readRunningProfileSnapshot).toHaveBeenCalledTimes(1);
    raw.resolve(snapshotFor(selected));
    await Promise.all([firstSnapshot, secondSnapshot]);

    const firstOfficial = h.controller.readOfficial();
    const secondOfficial = h.controller.readOfficial();
    expect(h.api.readOfficialProfile).toHaveBeenCalledTimes(1);
    official.resolve(officialFor(selected));
    await Promise.all([firstOfficial, secondOfficial]);
  });

  it("drops a late snapshot after A to B to A exact-object selection transitions", async () => {
    const first = runningList();
    const a = first.candidates[0];
    const b: ConnectedQueryRef = { ...a, query_id: "00000000-0000-0000-0000-000000003516", source_refs: ["profile_list:row:1"] };
    const h = harness([{ ...first, returned: 2, candidates: [a, b] }]);
    const late = deferred<RunningProfileSnapshotResponse>();
    h.api.readRunningProfileSnapshot = vi.fn(() => late.promise);
    await h.controller.readCandidates();
    const rows = (h.controller.snapshot().list as Extract<ReturnType<typeof h.controller.snapshot>["list"], { status: "success" }>).response.candidates;
    h.controller.select(rows[0]);
    const pending = h.controller.readSnapshot();
    h.controller.select(rows[1]);
    h.controller.select(rows[0]);
    late.resolve(snapshotFor(rows[0]));
    await pending;

    expect(h.controller.snapshot().selected).toBe(rows[0]);
    expect(h.controller.snapshot().snapshot.status).toBe("idle");
    expect(h.api.readRunningProfileSnapshot).toHaveBeenCalledTimes(1);
  });

  it("closes the selected query's panel and drops a snapshot that lands after closing", async () => {
    const first = runningList(1);
    const h = harness([first]);
    const late = deferred<RunningProfileSnapshotResponse>();
    h.api.readRunningProfileSnapshot = vi.fn(() => late.promise);
    await h.controller.readCandidates();
    const row = (h.controller.snapshot().list as Extract<ReturnType<typeof h.controller.snapshot>["list"], { status: "success" }>).response.candidates[0];
    h.controller.select(row);
    const pending = h.controller.readSnapshot();
    h.controller.deselect();
    late.resolve(snapshotFor(row));
    await pending;

    expect(h.controller.snapshot().selected).toBeNull();
    expect(h.controller.snapshot().snapshot.status).toBe("idle");
    // Selecting it again starts from a clean panel.
    h.controller.select(row);
    expect(h.controller.snapshot().selected).toBe(row);
    expect(h.controller.snapshot().snapshot.status).toBe("idle");
  });

  it("retains only a completed prior-generation snapshot after a manual refresh, and closes live authority", async () => {
    const first = runningList(2);
    const second = runningList(3, []);
    const h = harness([first, second]);
    await h.controller.readCandidates();
    const selected = (h.controller.snapshot().list as Extract<ReturnType<typeof h.controller.snapshot>["list"], { status: "success" }>).response.candidates[0];
    h.controller.select(selected);
    await h.controller.readSnapshot();
    await h.controller.readCandidates();

    const state = h.controller.snapshot();
    expect(state.list).toMatchObject({ status: "success", response: { discovery_generation: 3, candidates: [] } });
    expect(state.selected).toBeNull();
    expect(state.official).toEqual({ status: "closed" });
    expect(state.retained).toMatchObject({ discoveryGeneration: 2, query: { query_id: selected.query_id }, snapshot: { query: { query_id: selected.query_id } } });
  });

  it("drops a pending list when the host resets for a session transition", async () => {
    const h = harness();
    const pending = deferred<RunningProfileCandidateListResponse>();
    h.api.readRunningProfileCandidates = vi.fn(() => pending.promise);
    const work = h.controller.readCandidates();
    h.setSession({ ...structuredClone(sr3514ConnectedContractGolden.connection), state: "expired" });
    h.controller.reset();
    pending.resolve(runningList());
    await work;

    expect(h.controller.snapshot()).toMatchObject({ list: { status: "idle" }, selected: null, snapshot: { status: "idle" }, official: { status: "closed" } });
    expect(h.onDiscoveryChanged).not.toHaveBeenCalled();
  });
});



it.each(["snapshot", "official"] as const)("ignores late %s success/failure after refresh and aborts transport", async (kind) => {
  for (const fail of [false, true]) {
    const first = runningList();
    const h = harness([first, runningList(3, [])]);
    const pending = deferred<any>();
    const request = vi.fn(() => pending.promise);
    if (kind === "snapshot") h.api.readRunningProfileSnapshot = request;
    else h.api.readOfficialProfile = request;
    await h.controller.readCandidates(); h.controller.select(first.candidates[0]);
    const work = kind === "snapshot" ? h.controller.readSnapshot() : h.controller.readOfficial();
    await h.controller.readCandidates();
    expect((request.mock.calls as unknown as [unknown, AbortSignal][])[0][1].aborted).toBe(true);
    if (fail) pending.reject({ code: "connection_expired" });
    else pending.resolve(kind === "snapshot" ? snapshotFor(first.candidates[0]) : officialFor(first.candidates[0]));
    await work;
    expect(h.controller.snapshot()).toMatchObject({ selected: null, snapshot: { status: "idle" }, official: { status: "closed" }, retained: null });
    expect(h.onSessionExpired).not.toHaveBeenCalled();
    expect(request).toHaveBeenCalledTimes(1);
  }
});

it("reads official first, concurrently reads snapshot, and retains immutable independent successes", async () => {
  const first = runningList(); const selected = first.candidates[0];
  const h = harness([first, runningList(3, [])]);
  const raw = deferred<RunningProfileSnapshotResponse>(); const official = deferred<OfficialProfileReadResponse>();
  h.api.readRunningProfileSnapshot = vi.fn(() => raw.promise);
  h.api.readOfficialProfile = vi.fn(() => official.promise);
  await h.controller.readCandidates(); h.controller.select(selected);
  const a = h.controller.readOfficial();
  expect(h.api.readRunningProfileSnapshot).not.toHaveBeenCalled();
  const b = h.controller.readSnapshot();
  const rawValue = snapshotFor(selected); const officialValue = officialFor(selected);
  official.resolve(officialValue); raw.resolve(rawValue); await Promise.all([a, b]);
  expect(h.controller.snapshot().official.status).toBe("success");
  expect(h.controller.snapshot().snapshot.status).toBe("success");
  h.controller.select(selected);
  expect(h.controller.snapshot().snapshot.status).toBe("success");
  await h.controller.readCandidates();
  const retained = h.controller.snapshot().retained!;
  expect(retained.official?.response.display_text).toBe("official profile");
  expect(Object.isFrozen(retained.snapshot?.outcome.report)).toBe(true);
  rawValue.query.query_id = "mutated"; officialValue.display_text = "mutated";
  expect(retained.snapshot?.query.query_id).toBe(selected.query_id);
  expect(retained.official?.response.display_text).toBe("official profile");
  await h.controller.readSnapshot(); await h.controller.readOfficial();
  expect(h.api.readRunningProfileSnapshot).toHaveBeenCalledTimes(1);
  expect(h.api.readOfficialProfile).toHaveBeenCalledTimes(1);
});

it("deduplicates list same tick, rejects clones, and isolates retained material across sessions", async () => {
  const first = runningList(); const h = harness([first]);
  const pending = deferred<RunningProfileCandidateListResponse>();
  h.api.readRunningProfileCandidates = vi.fn(() => pending.promise);
  const a = h.controller.readCandidates(); const b = h.controller.readCandidates();
  expect(h.api.readRunningProfileCandidates).toHaveBeenCalledTimes(1);
  pending.resolve(first); await Promise.all([a, b]);
  h.controller.select({ ...first.candidates[0] }); await h.controller.readOfficial();
  expect(h.api.readOfficialProfile).not.toHaveBeenCalled();
  h.controller.select(first.candidates[0]); await h.controller.readSnapshot();
  h.setSession({ ...structuredClone(sr3514ConnectedContractGolden.connection), connection_id: "foreign" });
  expect(h.controller.snapshot().retained).toBeNull();
  expect(h.controller.snapshot().snapshot.status).toBe("idle");
  h.controller.reset();
  await h.controller.readSnapshot(); await h.controller.readOfficial();
  expect(h.api.readRunningProfileSnapshot).toHaveBeenCalledTimes(1);
  expect(h.api.readOfficialProfile).not.toHaveBeenCalled();
});

it.each(["success", "failure"])("drops official A-B-A late %s", async (result) => {
  const list = runningList(); const a = list.candidates[0]; const b = { ...a, query_id: "other" };
  list.candidates.push(b); const h = harness([list]);
  const pending = deferred<OfficialProfileReadResponse>(); h.api.readOfficialProfile = vi.fn(() => pending.promise);
  await h.controller.readCandidates(); h.controller.select(a); const work = h.controller.readOfficial();
  h.controller.select(b); h.controller.select(a);
  if (result === "success") pending.resolve(officialFor(a)); else pending.reject(new Error("late"));
  await work; expect(h.controller.snapshot().official.status).toBe("closed");
});

it.each(["permission_denied", "unsupported", "connection_expired"])("preserves typed %s failure with no fallback", async (code) => {
  const list = runningList(); const h = harness([list]);
  const error = { code, stage: "request", message_id: code, recoverability: "restart", details: {} };
  h.api.readRunningProfileSnapshot = vi.fn(async () => { throw error; });
  await h.controller.readCandidates(); h.controller.select(list.candidates[0]); await h.controller.readSnapshot();
  expect(h.controller.snapshot().snapshot).toEqual({ status: "failed", error });
  expect(h.api.readOfficialProfile).not.toHaveBeenCalled();
  expect(h.onSessionExpired).toHaveBeenCalledTimes(code === "connection_expired" ? 1 : 0);
});

it("rejects mismatched snapshot envelope without admitting a Doctor artifact", async () => {
  const list = runningList(); const h = harness([list]);
  const wrong = snapshotFor(list.candidates[0]); wrong.query.discovery_generation = 99;
  h.api.readRunningProfileSnapshot = vi.fn(async () => wrong);
  await h.controller.readCandidates(); h.controller.select(list.candidates[0]); await h.controller.readSnapshot();
  expect(h.controller.snapshot().snapshot).toMatchObject({ status: "failed", error: { code: "invalid_running_snapshot_response" } });
});
