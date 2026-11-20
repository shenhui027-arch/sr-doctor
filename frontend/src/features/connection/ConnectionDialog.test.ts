import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { LocaleProvider as LocaleProviderWithChildren, type LocalizedText } from "../../i18n/locale";
import type { ComponentProps, FunctionComponent, ReactNode } from "react";

// createElement(LocaleProvider, props, child) passes the child as the third
// argument; this alias lets TypeScript accept that form in tests.
const LocaleProvider = LocaleProviderWithChildren as FunctionComponent<
  Omit<ComponentProps<typeof LocaleProviderWithChildren>, "children"> & { children?: ReactNode }
>;
import {
  connectionFailedCopy,
  connectionFailureMessage,
  ConnectionStateView,
  isConnectionExpiredFailure,
  localizedConnectionMessage,
} from "./ConnectionDialog";

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
      "remote_timeout", "session_limit", "saved_connection_unavailable",
    ];
    for (const code of codes) {
      expect(localizedConnectionMessage(code, en)).not.toBe(code);
      expect(localizedConnectionMessage(code, zh)).not.toBe(code);
    }
  });

  it("maps connection errors by code in English instead of falling back to generic Connection failed (regression)", () => {
    const genericEn = "Connection failed. Check the connection details and network, then retry.";

    // Known backend error codes render specific copy in English.
    const authHtml = renderToStaticMarkup(
      createElement(
        LocaleProvider,
        { initialLocale: "en" },
        createElement(ConnectionStateView, { state: { phase: "failed", session: null, message: "authentication_failed" } }),
      ),
    );
    expect(authHtml).toContain("StarRocks rejected the supplied credentials.");
    expect(authHtml).not.toContain(genericEn);

    // Client validation keys whose English copy equals the key also render their specific message in English.
    for (const validationKey of [
      "Host, port, and username are required.",
      "Plaintext is available only for a loopback Docker lab.",
    ]) {
      const html = renderToStaticMarkup(
        createElement(
          LocaleProvider,
          { initialLocale: "en" },
          createElement(ConnectionStateView, { state: { phase: "failed", session: null, message: validationKey } }),
        ),
      );
      expect(html).toContain(validationKey);
      expect(html).not.toContain(genericEn);
    }

    // Unknown codes fall back to the generic copy without leaking internal details.
    expect(connectionFailedCopy("unmapped_internal_driver_reason", en)).toBe(genericEn);
  });
});

it("shows connection status and version without session handles, TTL or capability enums", () => {
  const session = { ...sr3514ConnectedContractGolden.connection, connection_id: "private-session-handle", idle_timeout_seconds: 900 };
  const html = renderToStaticMarkup(createElement(ConnectionStateView, { state: { phase: "ready", session, message: null } }));
  expect(html).toContain("3.5.14");
  for (const hidden of ["private-session-handle", "900", "audit_user_filter", "410", "idle"]) expect(html).not.toContain(hidden);
  const failure = renderToStaticMarkup(createElement(ConnectionStateView, { state: { phase: "failed", session: null, message: "unknown_backend_private_detail" } }));
  expect(failure).not.toContain("unknown_backend_private_detail");
  expect(failure).toContain("Connection failed. Check the connection details and network, then retry.");
});
