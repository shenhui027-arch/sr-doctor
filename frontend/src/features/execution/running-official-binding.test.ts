import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { ConnectedQueryRef, ConnectionSessionView, RunningProfileCandidateListResponse, RunningProfileSnapshotResponse } from "../../../../contracts/generated/doctor-contracts";
import { admitRunningOfficialBinding, runningSnapshotMatchesBinding } from "./running-official-binding";

function fixture() {
  const session: ConnectionSessionView = structuredClone(sr3514ConnectedContractGolden.connection);
  const selected: ConnectedQueryRef = { ...structuredClone(sr3514ConnectedContractGolden.profile_fetch_response.query),
    connection_id: session.connection_id, discovery_source: "profile_list" as const,
    discovery_generation: 42, status: "Running" };
  const list: RunningProfileCandidateListResponse = {
    source: "profile_list", discovery_generation: 42, limit: 20, returned: 1,
    at_limit: false, local_truncated: false, rejected: 0,
    read_at_utc: "2026-09-08T00:00:00Z", profile_observed_at_utc: null,
    candidates: [selected], limitations: [], statement_filter: { state: "not_applied" },
  };
  return { session, list, selected };
}

describe("Running official discovery admission", () => {
  it.each(["Running", "running", "rUNNING"])("accepts preserved source status %s", (status) => {
    const input = fixture(); input.selected.status = status;
    expect(admitRunningOfficialBinding(input)).not.toBeNull();
  });
  it("admits whole query without requiring a snapshot or issuing a request", () => {
    const input = fixture();
    const result = admitRunningOfficialBinding(input)!;
    expect(result).toEqual({ connection_id: input.session.connection_id, query_id: input.selected.query_id,
      identity: { kind: "discovery", discovery_generation: 42 }, node_ids: [] });
    expect(Object.isFrozen(result)).toBe(true);
    expect(Object.isFrozen(result.identity)).toBe(true);
    expect(Object.isFrozen(result.node_ids)).toBe(true);
    input.selected.query_id = "changed";
    expect(result.query_id).not.toBe("changed");
  });
  it.each(["session", "list", "selected"] as const)("rejects missing %s", (key) => {
    expect(admitRunningOfficialBinding({ ...fixture(), [key]: null })).toBeNull();
  });
  it.each(["expired", "closed", "failed", "unsupported"] as const)("rejects %s session", (state) => {
    const input = fixture(); input.session.state = state;
    expect(admitRunningOfficialBinding(input)).toBeNull();
  });
  it.each(["Finished", "Error", "unknown", ""])("rejects status %s", (status) => {
    const input = fixture(); input.selected.status = status;
    expect(admitRunningOfficialBinding(input)).toBeNull();
  });
  it.each([0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1])("rejects generation %s", (generation) => {
    const input = fixture(); input.list.discovery_generation = input.selected.discovery_generation = generation;
    expect(admitRunningOfficialBinding(input)).toBeNull();
  });
  it("rejects cloned and stale selections even with matching query IDs", () => {
    const input = fixture();
    expect(admitRunningOfficialBinding({ ...input, selected: { ...input.selected } })).toBeNull();
    input.list = { ...input.list, candidates: [{ ...input.selected }] };
    expect(admitRunningOfficialBinding(input)).toBeNull();
  });
  it.each(["connection", "generation", "source", "list_source", "empty_id"])("rejects %s mismatch", (fault) => {
    const input = fixture();
    if (fault === "connection") input.session.connection_id = "another";
    if (fault === "generation") input.list.discovery_generation++;
    if (fault === "source") input.selected.discovery_source = "audit";
    if (fault === "list_source") input.list.source = "audit";
    if (fault === "empty_id") input.selected.query_id = "";
    expect(admitRunningOfficialBinding(input)).toBeNull();
  });
});

describe("Running snapshot envelope admission", () => {
  function snapshotFixture() {
    const input = fixture();
    const response: RunningProfileSnapshotResponse = structuredClone(sr3514ConnectedContractGolden.running_profile_snapshot_response);
    response.query = { ...input.selected };
    return { response, binding: admitRunningOfficialBinding(input)! };
  }
  it.each(["running", "finished", "error", "unknown"] as const)("preserves independently parsed %s lifecycle", (state) => {
    const { response, binding } = snapshotFixture();
    response.profile_lifecycle = response.outcome.report.profile_context.query_state = state;
    expect(runningSnapshotMatchesBinding(response, binding)).toBe(true);
  });
  it("accepts source casing without deriving lifecycle from it", () => {
    const { response, binding } = snapshotFixture();
    response.query.status = "running"; response.list_observed_state = "rUNNING";
    expect(runningSnapshotMatchesBinding(response, binding)).toBe(true);
  });
  it.each(["connection", "query", "generation", "source", "status", "list_status", "origin", "hash", "key", "lifecycle"])("rejects %s mismatch", (fault) => {
    const { response, binding } = snapshotFixture();
    if (fault === "connection") response.query.connection_id = "other";
    if (fault === "query") response.query.query_id = "other";
    if (fault === "generation") response.query.discovery_generation = 43;
    if (fault === "source") response.query.discovery_source = "audit";
    if (fault === "status") response.query.status = "Finished";
    if (fault === "list_status") response.list_observed_state = "Finished";
    if (fault === "origin") response.input_origin = "connected_sql_runtime";
    if (fault === "hash") response.source_hash = "other";
    if (fault === "key") response.analysis_key = "other";
    if (fault === "lifecycle") response.profile_lifecycle = response.outcome.report.profile_context.query_state === "running" ? "finished" : "running";
    expect(runningSnapshotMatchesBinding(response, binding)).toBe(false);
  });
  it("rejects absent or node-scoped authority", () => {
    const { response, binding } = snapshotFixture();
    expect(runningSnapshotMatchesBinding(response, null)).toBe(false);
    expect(runningSnapshotMatchesBinding(response, { ...binding, node_ids: [1] })).toBe(false);
  });
  it.each(["id", "version", "manifest_hash", "length", "order"])("rejects duplicated receipt %s drift", (field) => {
    const { response, binding } = snapshotFixture();
    const parts = response.analysis_receipt.components;
    expect(parts.length).toBeGreaterThan(1);
    if (field === "length") parts.pop();
    else if (field === "order") parts.reverse();
    else if (field === "id") parts[0].id = "changed";
    else if (field === "version") parts[0].version = "changed";
    else parts[0].manifest_hash = "changed";
    expect(runningSnapshotMatchesBinding(response, binding)).toBe(false);
  });
});
