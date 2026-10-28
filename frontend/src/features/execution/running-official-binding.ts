import type { ConnectedQueryRef, ConnectionSessionView, RunningProfileCandidateListResponse, RunningProfileSnapshotResponse } from "../../../../contracts/generated/doctor-contracts";
import { officialIdentityKey, type OfficialBinding } from "./official-profile-read";

/** Current list membership grants an explicit whole-query read, not fetched evidence. */
export function admitRunningOfficialBinding(input: {
  session: ConnectionSessionView | null;
  list: RunningProfileCandidateListResponse | null;
  selected: ConnectedQueryRef | null;
}): OfficialBinding | null {
  const { session, list, selected } = input;
  if (session?.state !== "ready" || !list || !selected
    || list.source !== "profile_list" || !list.candidates.includes(selected)
    || selected.discovery_source !== list.source || selected.status.toLowerCase() !== "running"
    || !selected.connection_id || !selected.query_id
    || selected.connection_id !== session.connection_id
    || selected.discovery_generation !== list.discovery_generation) return null;
  const identity = { kind: "discovery" as const, discovery_generation: list.discovery_generation };
  if (officialIdentityKey(identity) === null) return null;
  return Object.freeze({ connection_id: selected.connection_id, query_id: selected.query_id,
    identity: Object.freeze(identity), node_ids: Object.freeze([]) });
}

/** Envelope admission only. The parser lifecycle is not the older list observation. */
export function runningSnapshotMatchesBinding(
  response: RunningProfileSnapshotResponse,
  binding: OfficialBinding | null,
): boolean {
  if (!binding || binding.identity.kind !== "discovery" || binding.node_ids.length !== 0
    || officialIdentityKey(binding.identity) === null) return false;
  const report = response.outcome.report;
  return response.query.connection_id === binding.connection_id
    && response.query.query_id === binding.query_id
    && response.query.discovery_generation === binding.identity.discovery_generation
    && response.query.discovery_source === "profile_list"
    && response.query.status.toLowerCase() === "running" && response.list_observed_state.toLowerCase() === "running"
    && response.input_origin === "connected_profile_fetch"
    && report !== null
    && response.profile_lifecycle === report.profile_context.query_state
    && response.source_hash === report.input.source_hash
    && response.source_kind === report.input.detected_kind
    && response.analysis_key === report.analysis_key
    && response.analysis_receipt.components.length === report.analysis_receipt.components.length
    && response.analysis_receipt.components.every((component, index) => {
      const expected = report.analysis_receipt.components[index];
      return component.id === expected.id && component.version === expected.version
        && component.manifest_hash === expected.manifest_hash;
    });
}
