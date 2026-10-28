import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import type { ConnectionSessionView, GraphNode, ProfileFetchResponse } from "../../../../contracts/generated/doctor-contracts";
import { admitOfficialProfileBinding } from "./official-profile-binding";

function fixture() {
  const evidence: ProfileFetchResponse = structuredClone(sr3514ConnectedContractGolden.profile_fetch_response);
  const session: ConnectionSessionView = structuredClone(sr3514ConnectedContractGolden.connection);
  const report = evidence.outcome.report!;
  const node: GraphNode = { node_id: "plan:27", entity_kind: "plan_node", plan_node_id: "27",
    fragment_id: null, pipeline_id: null, plan_node_type: null, display_name: null,
    source_ordinal: null, native_operator_refs: [], subordinate_operator_refs: [],
    metric_observation_refs: [], execution_state: null, source_quality: "exact", source_refs: ["fixture"] };
  report.graph_view.nodes = [node];
  return { session,
    query: { ...evidence.query }, evidence, report, scope: { kind: "whole_query" as const } };
}

describe("official Profile host admission", () => {
  function standardFixture() {
    const input = fixture();
    input.evidence.input_origin = "connected_sql_runtime";
    input.query.discovery_generation = input.evidence.query.discovery_generation = null;
    input.query.discovery_source = input.evidence.query.discovery_source = null;
    input.evidence.official_profile_eligibility = { execution_attempt_id: "00000000-0000-4000-8000-000000000045", method: "standard_profile" };
    return input;
  }
  it("admits verified standard execution without inventing discovery", () => {
    const input = standardFixture();
    const result = admitOfficialProfileBinding(input)!;
    expect(result.identity).toEqual({ kind: "verified_standard_attempt", execution_attempt_id: input.evidence.official_profile_eligibility!.execution_attempt_id });
    expect(Object.isFrozen(result.identity)).toBe(true);
    expect(input.query.discovery_generation).toBeNull();
  });
  it.each(["missing", "malformed", "origin", "generation", "report", "session"])("rejects standard execution with %s mismatch", (fault) => {
    const input = standardFixture();
    if (fault === "missing") delete input.evidence.official_profile_eligibility;
    if (fault === "malformed") input.evidence.official_profile_eligibility!.execution_attempt_id = "not-an-attempt";
    if (fault === "origin") input.evidence.input_origin = "connected_profile_fetch";
    if (fault === "generation") input.query.discovery_generation = input.evidence.query.discovery_generation = 1;
    if (fault === "report") input.report = { ...input.report };
    if (fault === "session") input.session.connection_id = "another-session";
    expect(admitOfficialProfileBinding(input)).toBeNull();
  });
  it("admits an immutable whole-query binding without reading anything", () => {
    const input = fixture();
    const binding = admitOfficialProfileBinding(input)!;
    expect(binding).toEqual({ connection_id: input.query.connection_id, query_id: input.query.query_id,
      identity: { kind: "discovery", discovery_generation: input.query.discovery_generation }, node_ids: [] });
    expect(Object.isFrozen(binding)).toBe(true);
    expect(Object.isFrozen(binding.node_ids)).toBe(true);
    input.query.query_id = "changed";
    expect(binding.query_id).not.toBe("changed");
  });
  it.each(["0", "27", "2147483647", "00027"])("admits decimal plan ID %s", (id) => {
    const input = fixture();
    const node = input.report.graph_view.nodes[0];
    node.plan_node_id = id;
    expect(admitOfficialProfileBinding({ ...input, scope: { kind: "plan_node", node } })?.node_ids).toEqual([Number(id)]);
  });
  it.each([null, "", "-1", "+1", "1.0", "1e2", "0x10", " 1", "1\n", "2147483648", "999999999999999999999"])("rejects plan ID %s", (id) => {
    const input = fixture();
    const node = input.report.graph_view.nodes[0];
    node.plan_node_id = id;
    expect(admitOfficialProfileBinding({ ...input, scope: { kind: "plan_node", node } })).toBeNull();
  });
  it("rejects foreign nodes even with identical IDs and fields", () => {
    const input = fixture();
    expect(admitOfficialProfileBinding({ ...input, scope: { kind: "plan_node", node: { ...input.report.graph_view.nodes[0] } } })).toBeNull();
  });
  it.each(["fragment", "pipeline", "operator_group", "sink"] as const)("rejects %s scope", (kind) => {
    const input = fixture();
    const node = input.report.graph_view.nodes[0];
    node.entity_kind = kind;
    expect(admitOfficialProfileBinding({ ...input, scope: { kind: "plan_node", node } })).toBeNull();
  });
  it.each(["expired", "closed", "failed", "unsupported"] as const)("rejects %s session", (state) => {
    const input = fixture();
    input.session.state = state;
    expect(admitOfficialProfileBinding(input)).toBeNull();
  });
  it.each(["session", "query", "evidence", "report"] as const)("rejects absent %s", (key) => {
    expect(admitOfficialProfileBinding({ ...fixture(), [key]: null })).toBeNull();
  });
  it.each([null, 0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1])("rejects generation %s", (generation) => {
    const input = fixture();
    input.query.discovery_generation = input.evidence.query.discovery_generation = generation;
    expect(admitOfficialProfileBinding(input)).toBeNull();
  });
  it.each(["connection_id", "query_id", "discovery_generation"] as const)("rejects mismatched evidence %s", (key) => {
    const input = fixture();
    if (key === "discovery_generation") input.evidence.query[key] = input.query[key]! + 1;
    else input.evidence.query[key] += "other";
    expect(admitOfficialProfileBinding(input)).toBeNull();
  });
  it("rejects another session and another report object", () => {
    const input = fixture();
    expect(admitOfficialProfileBinding({ ...input, session: { ...input.session, connection_id: "other" } })).toBeNull();
    expect(admitOfficialProfileBinding({ ...input, report: { ...input.report } })).toBeNull();
  });
  it("rejects offline and non-fetched evidence", () => {
    const input = fixture();
    expect(admitOfficialProfileBinding({ ...input, evidence: { ...input.evidence, input_origin: "offline_import" } })).toBeNull();
    const malformed = { ...input.evidence, state: "unavailable" } as unknown as ProfileFetchResponse;
    expect(admitOfficialProfileBinding({ ...input, evidence: malformed })).toBeNull();
  });
});
