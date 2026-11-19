import type { ConnectionCreateRequest, TlsMode } from "../../../../contracts/generated/doctor-contracts";

/**
 * Task 71: what this browser remembers about a connection. Never the password.
 *
 * - The live session's opaque ID, in this tab's sessionStorage, written only as the page is
 *   hidden and read back on load, so a reload re-attaches instead of asking for the password.
 *   The server still holds the credentials and drops them on disconnect or idle expiry.
 * - The host, port, username and transport of the last successful connection, in
 *   localStorage, so the form comes back filled in.
 *
 * Storage can be unavailable (private windows, blocked site data); every access is best effort.
 */
const RESUME_KEY = "sr-doctor.connection-session";
const FIELDS_KEY = "sr-doctor.connection";
const TLS_MODES: readonly TlsMode[] = ["plaintext", "verify_full"];

export type RememberedConnection = Pick<ConnectionCreateRequest, "host" | "port" | "username" | "tls_mode">;

export function readResumableSession(): string | null {
  try {
    const value = window.sessionStorage.getItem(RESUME_KEY);
    return value && value.length <= 128 ? value : null;
  } catch {
    return null;
  }
}

export function rememberResumableSession(connectionId: string): void {
  try { window.sessionStorage.setItem(RESUME_KEY, connectionId); } catch { /* storage unavailable */ }
}

/** Cleared while the page is live, so a duplicated tab never shares this tab's session. */
export function forgetResumableSession(): void {
  try { window.sessionStorage.removeItem(RESUME_KEY); } catch { /* storage unavailable */ }
}

export function readRememberedConnection(): RememberedConnection | null {
  try {
    const raw = window.localStorage.getItem(FIELDS_KEY);
    if (!raw) return null;
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) return null;
    const { host, port, username, tls_mode: tlsMode } = value as Record<string, unknown>;
    if (typeof host !== "string" || !host.trim() || host.length > 255) return null;
    if (typeof port !== "number" || !Number.isInteger(port) || port < 1 || port > 65535) return null;
    if (typeof username !== "string" || !username.trim() || username.length > 256) return null;
    if (!TLS_MODES.includes(tlsMode as TlsMode)) return null;
    return { host, port, username, tls_mode: tlsMode as TlsMode };
  } catch {
    return null;
  }
}

export function rememberConnection(draft: ConnectionCreateRequest): void {
  // Only these four fields are written: the password never reaches storage.
  const value: RememberedConnection = { host: draft.host.trim(), port: draft.port, username: draft.username.trim(), tls_mode: draft.tls_mode };
  try { window.localStorage.setItem(FIELDS_KEY, JSON.stringify(value)); } catch { /* storage unavailable */ }
}
