import type { AnalysisSnapshotReport, ConnectedQueryRef, ConnectionSessionView, GraphNode, ProfileFetchResponse } from "../../../../contracts/generated/doctor-contracts";
import { officialIdentityKey, type OfficialBinding } from "./official-profile-read";

/** Host-owned current identities, never inferred from Profile text or node labels. */
export function admitOfficialProfileBinding(input: {
  session: ConnectionSessionView | null;
  query: ConnectedQueryRef | null;
  evidence: ProfileFetchResponse | null;
  report: AnalysisSnapshotReport | null;
  scope: { kind: "whole_query" } | { kind: "plan_node"; node: GraphNode };
}): OfficialBinding | null {
  const { session, query, evidence, report, scope } = input;
  if (session?.state !== "ready" || !query || evidence?.state !== "fetched" || !report
    || evidence.outcome.report !== report || !query.connection_id || !query.query_id
    || session.connection_id !== query.connection_id || evidence.query.connection_id !== query.connection_id
    || evidence.query.query_id !== query.query_id || evidence.query.discovery_generation !== query.discovery_generation) return null;

  let identity: OfficialBinding["identity"];
  if (evidence.input_origin === "connected_profile_fetch" && !evidence.official_profile_eligibility
    && query.discovery_generation !== null) {
    identity = { kind: "discovery", discovery_generation: query.discovery_generation };
  } else if (evidence.input_origin === "connected_sql_runtime" && query.discovery_generation === null
    && query.discovery_source === null && evidence.query.discovery_source === null
    && evidence.official_profile_eligibility?.method === "standard_profile") {
    identity = { kind: "verified_standard_attempt", execution_attempt_id: evidence.official_profile_eligibility.execution_attempt_id };
  } else return null;
  if (officialIdentityKey(identity) === null) return null;

  const nodeIds: number[] = [];
  if (scope.kind === "plan_node") {
    const node = scope.node;
    if (!report.graph_view.nodes.includes(node) || node.entity_kind !== "plan_node"
      || node.plan_node_id === null || !/^\d+$/.test(node.plan_node_id)) return null;
    const id = Number(node.plan_node_id);
    if (!Number.isSafeInteger(id) || id > 2147483647) return null;
    nodeIds.push(id);
  }
  return Object.freeze({ connection_id: query.connection_id, query_id: query.query_id,
    identity: Object.freeze(identity), node_ids: Object.freeze(nodeIds) });
}
