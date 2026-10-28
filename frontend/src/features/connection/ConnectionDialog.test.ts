import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { ConnectionStateView } from "./ConnectionDialog";
import { describe, expect, it } from "vitest";
import type { LocalizedText } from "../../i18n/locale";
import { connectionFailureMessage, isConnectionExpiredFailure, localizedConnectionMessage } from "./ConnectionDialog";

const en: LocalizedText = (english) => english;
const zh: LocalizedText = (_english, chinese) => chinese;

describe("ConnectionDialog failures", () => {
  it("keeps structured message IDs and recognizes authoritative expiry", () => {
    const failure = { message_id: "connection_expired" };
    expect(connectionFailureMessage(failure, "fallback")).toBe("connection_expired");
    expect(connectionFailureMessage({ code: "invalid_request", message_id: "json_request_invalid" }, "fallback")).toBe("invalid_request");
    expect(isConnectionExpiredFailure(failure)).toBe(true);
    expect(isConnectionExpiredFailure({ message_id: "remote_timeout" })).toBe(false);
  });

  it("renders every connector failure code in English and Chinese", () => {
    const codes = [
      "connection_expired", "authentication_failed", "permission_denied", "tls_failed",
      "connection_failed", "invalid_request", "tls_required", "capability_unavailable",
      "unsupported_version", "profile_unavailable", "statement_not_allowed", "explain_rejected",
      "remote_timeout", "session_limit",
    ];
    for (const code of codes) {
      expect(localizedConnectionMessage(code, en)).not.toBe(code);
      expect(localizedConnectionMessage(code, zh)).not.toBe(code);
    }
  });
});


it("shows connection status and version without session handles, TTL or capability enums", () => {
  const session = { ...sr3514ConnectedContractGolden.connection, connection_id: "private-session-handle", idle_timeout_seconds: 900 };
  const html = renderToStaticMarkup(createElement(ConnectionStateView, { state: { phase: "ready", session, message: null } }));
  expect(html).toContain("3.5.14");
  for (const hidden of ["private-session-handle", "900", "audit_user_filter", "410", "idle"]) expect(html).not.toContain(hidden);
  const failure = renderToStaticMarkup(createElement(ConnectionStateView, { state: { phase: "failed", session: null, message: "unknown_backend_private_detail" } }));
  expect(failure).not.toContain("unknown_backend_private_detail");
});
