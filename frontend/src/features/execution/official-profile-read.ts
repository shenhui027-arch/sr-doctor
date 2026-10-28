import type { OfficialProfileReadRequest, OfficialProfileReadResponse } from "../../../../contracts/generated/doctor-contracts";
import type { ApiFailure, DoctorApi } from "../../lib/api";

/** Already admitted by the host using the ready session and fetched Doctor evidence.
 * Node provenance belongs to that host; this kernel never derives it from text.
 */
export type OfficialBinding = Readonly<Omit<OfficialProfileReadRequest, "node_ids">> & { readonly node_ids: readonly number[] };
export type OfficialReadState =
  | { readonly status: "closed" }
  | { readonly status: "unread" | "loading"; readonly binding: OfficialBinding }
  | { readonly status: "success"; readonly binding: OfficialBinding; readonly response: OfficialProfileReadResponse }
  | { readonly status: "unavailable" | "failed"; readonly binding: OfficialBinding; readonly error: ApiFailure;
      /** Original failure scope when a connection/generation recovery block spans scopes. */
      readonly failureBinding?: OfficialBinding };

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function officialIdentityKey(value: unknown): string | null {
  if (!record(value) || Object.keys(value).length !== 2) return null;
  if (value.kind === "discovery" && Number.isSafeInteger(value.discovery_generation)
    && typeof value.discovery_generation === "number" && value.discovery_generation > 0) {
    return JSON.stringify([value.kind, value.discovery_generation]);
  }
  if (value.kind === "verified_standard_attempt" && typeof value.execution_attempt_id === "string"
    && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.execution_attempt_id)) {
    return JSON.stringify([value.kind, value.execution_attempt_id]);
  }
  return null;
}

export function sameOfficialBinding(a: OfficialBinding | null, b: OfficialBinding | null): boolean {
  return a !== null && b !== null && a.connection_id === b.connection_id && a.query_id === b.query_id
    && officialIdentityKey(a.identity) !== null && officialIdentityKey(a.identity) === officialIdentityKey(b.identity)
    && a.node_ids.length === b.node_ids.length
    && a.node_ids.every((id, index) => id === b.node_ids[index]);
}

function copyBinding(value: OfficialBinding | null): OfficialBinding | null {
  if (!value || !value.connection_id || !value.query_id || officialIdentityKey(value.identity) === null
    || value.node_ids.length > 1
    || value.node_ids.some((id) => !Number.isInteger(id) || id < 0 || id > 2147483647)) return null;
  return Object.freeze({ connection_id: value.connection_id, query_id: value.query_id,
    identity: Object.freeze({ ...value.identity }), node_ids: Object.freeze([...value.node_ids]) });
}

/** Envelope checks only: no normalization, hashing or Profile/metric interpretation. */
export function officialResponseMatches(value: unknown, binding: OfficialBinding): value is OfficialProfileReadResponse {
  if (!record(value) || !record(value.query) || !Array.isArray(value.node_ids)) return false;
  return value.kind === "analyze_profile_text"
    && value.query.connection_id === binding.connection_id && value.query.query_id === binding.query_id
    && officialIdentityKey(value.identity) !== null
    && officialIdentityKey(value.identity) === officialIdentityKey(binding.identity)
    && value.query.discovery_generation === (binding.identity.kind === "discovery" ? binding.identity.discovery_generation : null)
    && value.node_ids.length === binding.node_ids.length
    && value.node_ids.every((id, index) => id === binding.node_ids[index])
    && typeof value.text === "string" && typeof value.source_hash === "string"
    && typeof value.display_text === "string" && typeof value.display_hash === "string"
    && value.display_normalization === "ansi_sgr_strip_controls_escape_v1"
    && typeof value.read_at_utc === "string"
    && (value.profile_observed_at_utc === null || typeof value.profile_observed_at_utc === "string")
    && (value.detected_version === null || record(value.detected_version))
    && Array.isArray(value.source_refs) && value.source_refs.every((item) => typeof item === "string")
    && Array.isArray(value.limitations) && value.limitations.every((item) => typeof item === "string");
}

function failure(cause: unknown): ApiFailure {
  // DoctorApi returns ApiFailure; retain its exact typed reason and details.
  if (record(cause) && typeof cause.code === "string" && typeof cause.stage === "string"
    && typeof cause.message_id === "string" && typeof cause.recoverability === "string" && record(cause.details)) {
    return cause as ApiFailure;
  }
  return { code: "request_failed", stage: "request", message_id: "request_failed", recoverability: "retry_same", details: {} };
}

export function createOfficialProfileReader(options: {
  read: DoctorApi["readOfficialProfile"];
  /** Live render-time identity, or null when the session/evidence is inadmissible. */
  getCurrentBinding(): OfficialBinding | null;
  onChange?(): void;
  onSessionExpired?(): void;
}) {
  let state: OfficialReadState = { status: "closed" };
  let token = 0;
  let controller: AbortController | null = null;
  type BlockedState = Extract<OfficialReadState, { readonly status: "unavailable" | "failed" }>;
  // Retain recovery requirements across close/open and A -> B -> A, for this
  // controller's lifetime. Nothing is persisted or shared with another reader.
  const retryBlocks = new Map<string, BlockedState>();
  const connectionKey = (binding: OfficialBinding) => JSON.stringify([binding.connection_id]);
  const generationKey = (binding: OfficialBinding) => JSON.stringify([binding.connection_id, officialIdentityKey(binding.identity)]);
  const bindingKey = (binding: OfficialBinding) => JSON.stringify([binding.connection_id, binding.query_id, officialIdentityKey(binding.identity), binding.node_ids]);

  function rememberFailure(value: BlockedState): void {
    if (value.error.recoverability === "retry_same") return;
    const key = value.error.recoverability === "restart" ? connectionKey(value.binding)
      : value.error.code === "profile_unavailable" && value.error.details.limitation === "query_discovery_generation_expired"
        ? generationKey(value.binding) : bindingKey(value.binding);
    retryBlocks.set(key, value);
  }

  function invalidate(): void {
    ++token;
    controller?.abort();
    controller = null;
    state = { status: "closed" };
    options.onChange?.();
  }

  function snapshot(): OfficialReadState {
    // Call in render, not just in an effect: stale props never expose old text.
    return state.status !== "closed" && sameOfficialBinding(state.binding, copyBinding(options.getCurrentBinding()))
      ? state : { status: "closed" };
  }

  function open(): void {
    invalidate();
    const binding = copyBinding(options.getCurrentBinding());
    if (binding) {
      // Reopening the same identity is not the input change requested by the API.
      const blocked = retryBlocks.get(connectionKey(binding)) ?? retryBlocks.get(generationKey(binding)) ?? retryBlocks.get(bindingKey(binding));
      state = blocked ? sameOfficialBinding(blocked.binding, binding) ? blocked
        : { ...blocked, binding, failureBinding: blocked.binding } : { status: "unread", binding };
      options.onChange?.();
    }
  }

  async function read(): Promise<void> {
    const visible = snapshot();
    if (visible.status === "closed" || visible.status === "loading") return;
    if ((visible.status === "failed" || visible.status === "unavailable")
      && visible.error.recoverability !== "retry_same") return;
    const binding = visible.binding;
    const requestToken = ++token;
    controller?.abort();
    const pending = new AbortController();
    controller = pending;
    state = { status: "loading", binding };
    options.onChange?.();
    const current = () => token === requestToken && !pending.signal.aborted && state.status !== "closed"
      && sameOfficialBinding(binding, copyBinding(options.getCurrentBinding()));
    try {
      if (!current()) return;
      const response = await options.read({ ...binding, node_ids: [...binding.node_ids] }, pending.signal);
      if (!current()) return;
      if (!officialResponseMatches(response, binding)) {
        state = { status: "failed", binding, error: { code: "invalid_official_profile_response", stage: "request",
          message_id: "invalid_official_profile_response", recoverability: "change_input", details: {} } };
      } else state = { status: "success", binding, response };
    } catch (cause) {
      if (!current()) return;
      if (record(cause) && cause.name === "AbortError") {
        state = { status: "unread", binding };
      } else {
        const error = failure(cause);
        state = { status: error.code === "official_profile_unavailable" ? "unavailable" : "failed", binding, error };
        rememberFailure(state);
        if (error.code === "connection_expired" || error.message_id === "connection_expired"
          || error.message_id === "connection_session_expired") options.onSessionExpired?.();
      }
    } finally {
      if (current() && (state.status === "failed" || state.status === "unavailable")
        && state.error.recoverability !== "retry_same") rememberFailure(state);
      if (controller === pending) controller = null;
      if (token === requestToken) options.onChange?.();
    }
  }

  // Host MUST invalidate on every identity/scope transition, including A -> B -> A.
  // snapshot is a render-time guard, not a replacement for that lifecycle signal.
  // Host calls open after a local scope change; neither open nor invalidate requests data.
  return { open, read, close: invalidate, invalidate, snapshot };
}
