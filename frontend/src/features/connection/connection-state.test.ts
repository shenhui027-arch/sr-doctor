import { describe, expect, it } from "vitest";
import { connectionDraftError, connectionReducer, initialConnectionState, isLoopbackHost, sessionStatusLabel } from "./connection-state";

describe("connection state", () => {
  it("keeps plaintext limited to loopback Docker labs", () => {
    expect(isLoopbackHost("127.0.0.1")).toBe(true);
    expect(isLoopbackHost("::1")).toBe(true);
    expect(isLoopbackHost("localhost")).toBe(false);
    expect(isLoopbackHost("127.0.0.2")).toBe(false);
    expect(isLoopbackHost("db.example.internal")).toBe(false);
    expect(connectionDraftError({ host: "db.example.internal", port: 9030, username: "readonly", password: "secret", catalog: null, database: null, audit_table: null, tls_mode: "docker_loopback_plaintext" }))
      .toBe("Plaintext is available only for a loopback Docker lab.");
  });

  it("permits an empty password for an explicit loopback Docker lab", () => {
    expect(connectionDraftError({ host: "127.0.0.1", port: 9030, username: "root", password: "", catalog: null, database: null, audit_table: null, tls_mode: "docker_loopback_plaintext" }))
      .toBeNull();
  });

  it("makes connection expiry and close distinct visible states", () => {
    const connected = connectionReducer(initialConnectionState, {
      type: "session",
      session: {
        connection_id: "opaque-session",
        state: "ready",
        tls_mode: "docker_loopback_plaintext",
        idle_timeout_seconds: 900,
        detected_version: null,
        time_zone: null,
        capabilities: { version_probe: "available", time_zone: "unknown", audit: "unavailable", audit_user_filter: "unknown", audit_statement_filter: "unknown", profile_list: "available", profile_fetch: "available", explain_costs: "available", plan_advisor: "forbidden" },
        limitations: [],
      },
    });
    const expired = connectionReducer(connected, { type: "expire", message: "idle TTL reached" });
    const closed = connectionReducer(expired, { type: "close" });

    expect(expired).toMatchObject({ phase: "expired", message: "idle TTL reached" });
    expect(closed).toMatchObject({ phase: "closed" });
    expect(sessionStatusLabel(connected)).toBe("Connected / version unknown");
    expect(sessionStatusLabel(expired)).toBe("Expired / reconnect required");
  });
});
