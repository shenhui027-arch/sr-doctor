import type { AuditStatementFilter, ConnectedQueryRef, ConnectionSessionView, OfficialProfileReadResponse, RunningProfileCandidateListResponse, RunningProfileSnapshotResponse } from "../../../../contracts/generated/doctor-contracts";
import type { ApiFailure, DoctorApi } from "../../lib/api";
import { admitRunningOfficialBinding, runningSnapshotMatchesBinding } from "../execution/running-official-binding";
import { createOfficialProfileReader, type OfficialBinding, type OfficialReadState } from "../execution/official-profile-read";

export type RunningProfileEntryApi = Pick<DoctorApi, "readRunningProfileCandidates" | "readRunningProfileSnapshot" | "readOfficialProfile">;

type RequestState<T> =
  | { readonly status: "idle" }
  | { readonly status: "loading" }
  | { readonly status: "success"; readonly response: T }
  | { readonly status: "failed"; readonly error: ApiFailure };

export type RetainedRunningProfileSuccess = Readonly<{
  readonly query: ConnectedQueryRef;
  readonly discoveryGeneration: number;
  readonly snapshot: RunningProfileSnapshotResponse | null;
  readonly official: Readonly<{ readonly binding: OfficialBinding; readonly response: OfficialProfileReadResponse }> | null;
}>;

export type RunningProfileEntryState = Readonly<{
  readonly connectionId: string | null;
  readonly connectionState: ConnectionSessionView["state"] | null;
  readonly list: RequestState<RunningProfileCandidateListResponse>;
  readonly selected: ConnectedQueryRef | null;
  readonly snapshot: RequestState<RunningProfileSnapshotResponse>;
  readonly official: OfficialReadState;
  readonly retained: RetainedRunningProfileSuccess | null;
}>;

export type RunningProfileEntryController = Readonly<{
  snapshot(): RunningProfileEntryState;
  /** Hiding this tool's own statements is the default, as in the Profile list. */
  readCandidates(statementFilter?: AuditStatementFilter): Promise<void>;
  select(candidate: ConnectedQueryRef): void;
  deselect(): void;
  readSnapshot(): Promise<void>;
  readOfficial(): Promise<void>;
  reset(): void;
  dispose(): void;
}>;

export function createRunningProfileEntryController(options: {
  api: RunningProfileEntryApi;
  getSession(): ConnectionSessionView | null;
  onChange?(): void;
  onDiscoveryChanged?(): void;
  onSessionExpired?(): void;
}): RunningProfileEntryController {
  let epoch = 0;
  let listPending = false;
  let snapshotPending = false;
  let officialPending = false;
  let officialEpoch = 0;
  let listController: AbortController | null = null;
  let snapshotController: AbortController | null = null;
  let state: Omit<RunningProfileEntryState, "official"> = emptyState();

  const reader = createOfficialProfileReader({
    read: (request, signal) => options.api.readOfficialProfile(request, signal),
    getCurrentBinding: () => binding(),
    onChange: options.onChange,
    onSessionExpired: options.onSessionExpired,
  });

  function emptyState(): Omit<RunningProfileEntryState, "official"> {
    return {
      connectionId: null,
      connectionState: null,
      list: { status: "idle" },
      selected: null,
      snapshot: { status: "idle" },
      retained: null,
    };
  }

  function currentSession(): ConnectionSessionView | null {
    const session = options.getSession();
    return session?.state === "ready" ? session : null;
  }

  function binding(): OfficialBinding | null {
    return admitRunningOfficialBinding({ session: currentSession(), list: state.list.status === "success" ? state.list.response : null, selected: state.selected });
  }

  function isCurrent(requestEpoch: number, controller: AbortController, owner: OfficialBinding | null = null): boolean {
    if (requestEpoch !== epoch || controller.signal.aborted) return false;
    if (!owner) return true;
    const live = binding();
    return live !== null && sameBinding(live, owner);
  }

  function update(next: Omit<RunningProfileEntryState, "official">): void {
    state = next;
    options.onChange?.();
  }

  function invalidateActions(): void {
    ++epoch;
    snapshotController?.abort();
    snapshotController = null;
    snapshotPending = false;
    ++officialEpoch;
    officialPending = false;
    reader.invalidate();
  }

  function captureRetained(): RetainedRunningProfileSuccess | null {
    if (state.list.status !== "success" || !state.selected) return state.retained;
    const snapshot = state.snapshot.status === "success" ? state.snapshot.response : null;
    const official = reader.snapshot();
    if (!snapshot && official.status !== "success") return state.retained;
    return immutableCopy({ query: state.selected,
      discoveryGeneration: state.list.response.discovery_generation,
      snapshot,
      official: official.status === "success" ? Object.freeze({ binding: official.binding, response: official.response }) : null,
    });
  }

  function invalidResponse(): ApiFailure {
    return { code: "invalid_running_snapshot_response", stage: "request", message_id: "invalid_running_snapshot_response", recoverability: "change_input", details: {} };
  }

  async function readCandidates(statementFilter: AuditStatementFilter = "exclude_doctor_and_non_query"): Promise<void> {
    const session = currentSession();
    if (!session || listPending) return;
    listPending = true;
    const retained = captureRetained();
    ++epoch;
    listController?.abort();
    snapshotController?.abort();
    snapshotController = null;
    snapshotPending = false;
    ++officialEpoch;
    officialPending = false;
    reader.invalidate();
    const controller = new AbortController();
    listController = controller;
    const requestEpoch = epoch;
    update({
      connectionId: session.connection_id,
      connectionState: session.state,
      list: { status: "loading" },
      selected: null,
      snapshot: { status: "idle" },
      retained,
    });
    try {
      const response = await options.api.readRunningProfileCandidates({ connection_id: session.connection_id, limit: 25, statement_filter: statementFilter }, controller.signal);
      if (!isCurrent(requestEpoch, controller) || currentSession()?.connection_id !== session.connection_id) return;
      // The generated client owns DTO decoding; the UI only accepts the current response object.
      options.onDiscoveryChanged?.();
      update({
        connectionId: session.connection_id,
        connectionState: session.state,
        list: { status: "success", response },
        selected: null,
        snapshot: { status: "idle" },
        retained,
      });
    } catch (cause) {
      if (!isCurrent(requestEpoch, controller) || currentSession()?.connection_id !== session.connection_id) return;
      const error = apiFailure(cause);
      update({ ...state, list: { status: "failed", error }, selected: null, snapshot: { status: "idle" } });
      if (isConnectionExpired(error)) options.onSessionExpired?.();
    } finally {
      if (listController === controller) listController = null;
      if (requestEpoch === epoch) listPending = false;
    }
  }

  function select(candidate: ConnectedQueryRef): void {
    if (!currentSession() || state.list.status !== "success" || !state.list.response.candidates.includes(candidate) || candidate === state.selected) return;
    const retained = captureRetained();
    invalidateActions();
    update({ ...state, selected: candidate, snapshot: { status: "idle" }, retained });
  }

  /** Closes the selected query's panel; a successful read stays retained, as on reselection. */
  function deselect(): void {
    if (!state.selected) return;
    const retained = captureRetained();
    invalidateActions();
    update({ ...state, selected: null, snapshot: { status: "idle" }, retained });
  }

  async function readSnapshot(): Promise<void> {
    const owner = binding();
    if (!owner || snapshotPending) return;
    snapshotPending = true;
    const controller = new AbortController();
    snapshotController?.abort();
    snapshotController = controller;
    const requestEpoch = ++epoch;
    update({ ...state, snapshot: { status: "loading" } });
    try {
      const response = await options.api.readRunningProfileSnapshot({
        connection_id: owner.connection_id,
        query_id: owner.query_id,
        discovery_generation: owner.identity.kind === "discovery" ? owner.identity.discovery_generation : 0,
      }, controller.signal);
      if (!isCurrent(requestEpoch, controller, owner)) return;
      update({ ...state, snapshot: runningSnapshotMatchesBinding(response, owner)
        ? { status: "success", response }
        : { status: "failed", error: invalidResponse() } });
    } catch (cause) {
      if (!isCurrent(requestEpoch, controller, owner)) return;
      const error = apiFailure(cause);
      update({ ...state, snapshot: { status: "failed", error } });
      if (isConnectionExpired(error)) options.onSessionExpired?.();
    } finally {
      if (snapshotController === controller) snapshotController = null;
      if (requestEpoch === epoch) snapshotPending = false;
    }
  }

  async function readOfficial(): Promise<void> {
    if (!binding() || officialPending) return;
    officialPending = true;
    const actionEpoch = ++officialEpoch;
    reader.open();
    try {
      await reader.read();
    } finally {
      if (actionEpoch === officialEpoch) officialPending = false;
    }
  }

  function reset(): void {
    ++epoch;
    listController?.abort();
    snapshotController?.abort();
    listController = null;
    snapshotController = null;
    listPending = false;
    snapshotPending = false;
    ++officialEpoch;
    officialPending = false;
    reader.invalidate();
    update(emptyState());
  }

  function dispose(): void {
    ++epoch;
    listController?.abort();
    snapshotController?.abort();
    listController = null;
    snapshotController = null;
    ++officialEpoch;
    officialPending = false;
    reader.invalidate();
  }

  function snapshot(): RunningProfileEntryState {
    const session = options.getSession();
    if (!session || session.state !== "ready" || state.connectionId !== session.connection_id || state.connectionState !== session.state) {
      return Object.freeze<RunningProfileEntryState>({ ...emptyState(), official: { status: "closed" } });
    }
    return Object.freeze<RunningProfileEntryState>({ ...state, official: reader.snapshot() });
  }

  return Object.freeze({ snapshot, readCandidates, select, deselect, readSnapshot, readOfficial, reset, dispose });
}

function sameBinding(left: OfficialBinding, right: OfficialBinding): boolean {
  return left.connection_id === right.connection_id && left.query_id === right.query_id
    && left.identity.kind === right.identity.kind
    && left.identity.kind === "discovery" && right.identity.kind === "discovery"
    && left.identity.discovery_generation === right.identity.discovery_generation
    && left.node_ids.length === right.node_ids.length && left.node_ids.every((nodeId, index) => nodeId === right.node_ids[index]);
}

function apiFailure(cause: unknown): ApiFailure {
  if (typeof cause === "object" && cause !== null) {
    const value = cause as Partial<ApiFailure>;
    if (typeof value.code === "string" && typeof value.stage === "string" && typeof value.message_id === "string"
      && typeof value.recoverability === "string" && typeof value.details === "object" && value.details !== null) return value as ApiFailure;
  }
  return { code: "request_failed", stage: "request", message_id: "request_failed", recoverability: "retry_same", details: {} };
}

function isConnectionExpired(error: ApiFailure): boolean {
  return error.code === "connection_expired" || error.message_id === "connection_expired" || error.message_id === "connection_session_expired";
}

// Historical display data must not share mutable transport objects.
function immutableCopy<T>(value: T): T {
  const copy: T = structuredClone(value);
  function freeze(value: unknown): void {
    if (typeof value !== "object" || value === null || Object.isFrozen(value)) return;
    Object.values(value).forEach(freeze);
    Object.freeze(value);
  }
  freeze(copy);
  return copy;
}
