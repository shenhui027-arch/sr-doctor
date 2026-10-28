import type {
  ConnectionCreateRequest,
  ConnectionSessionState,
  ConnectionSessionView,
} from "../../../../contracts/generated/doctor-contracts";

export type ConnectionDraft = ConnectionCreateRequest;
export type { TlsMode } from "../../../../contracts/generated/doctor-contracts";

export type ConnectionState =
  | { phase: "connecting" }
  | { phase: ConnectionSessionState; session: ConnectionSessionView | null; message: string | null };

export const initialConnectionState: ConnectionState = { phase: "closed", session: null, message: null };

export type ConnectionAction =
  | { type: "connect" }
  | { type: "session"; session: ConnectionSessionView }
  | { type: "expire"; message: string }
  | { type: "fail"; message: string }
  | { type: "close" };

export function sessionStatusLabel(state: ConnectionState): string {
  if ((state.phase === "ready" || state.phase === "unsupported") && state.session) {
    const version = state.session.detected_version?.value ?? "version unknown";
    return `${state.phase === "ready" ? "Connected" : "Limited"} / ${version}`;
  }
  if (state.phase === "connecting") return "Connecting / probe pending";
  if (state.phase === "expired") return "Expired / reconnect required";
  if (state.phase === "failed") return "Connection failed";
  return "Offline / no session";
}

export function connectionReducer(state: ConnectionState, action: ConnectionAction): ConnectionState {
  switch (action.type) {
    case "connect":
      return { phase: "connecting" };
    case "session":
      return { phase: action.session.state, session: action.session, message: null };
    case "expire":
      return { phase: "expired", session: null, message: action.message };
    case "fail":
      return { phase: "failed", session: null, message: action.message };
    case "close":
      return { phase: "closed", session: null, message: "Connection closed; credentials were removed from the server session." };
  }
}

export function isLoopbackHost(host: string): boolean {
  const value = host.trim().toLowerCase();
  return value === "127.0.0.1" || value === "::1";
}

export function connectionDraftError(draft: ConnectionDraft): string | null {
  if (!draft.host.trim() || !Number.isInteger(draft.port) || draft.port < 1 || draft.port > 65535 || !draft.username.trim()) {
    return "Host, port, and username are required.";
  }
  if (draft.tls_mode === "docker_loopback_plaintext" && !isLoopbackHost(draft.host)) {
    return "Plaintext is available only for a loopback Docker lab.";
  }
  return null;
}
