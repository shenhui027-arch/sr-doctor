import { describe, expect, it } from "vitest";
import { auditErrorLabel, isConnectionExpiredFailure } from "./AuditWorkloadWorkspace";
import source from "./AuditWorkloadWorkspace.tsx?raw";

describe("Audit discarded-session timeout recovery", () => {
  it("routes the first typed transport timeout to recovery without changing its error", () => {
    const failure = Object.freeze({ code: "remote_timeout", stage: "audit_overview_primary", message_id: "remote_timeout", recoverability: "restart", details: {} });
    expect(isConnectionExpiredFailure(failure)).toBe(true);
    expect(auditErrorLabel(failure)).toBe("remote_timeout");
    expect(failure.code).toBe("remote_timeout");
  });
  it.each(["connection_expired", "connection_session_expired"])("keeps existing %s recovery", message_id => {
    expect(isConnectionExpiredFailure({ message_id })).toBe(true);
  });
  it.each([
    { code: "analysis_timeout", message_id: "analysis_timeout", recoverability: "retry_same" },
    { code: "request_failed", message_id: "remote_timeout", recoverability: "retry_same" },
    { code: "permission_denied", message_id: "permission_denied", recoverability: "restart" },
    { name: "AbortError" }, new Error("remote_timeout"), null,
  ])("does not expire a session from an unproven timeout or generic failure", failure => {
    expect(isConnectionExpiredFailure(failure)).toBe(false);
  });
  it("all existing Audit catch paths retain their error label before recovery", () => {
    const paths = source.match(/set(?:Sql)?Error\(auditErrorLabel\(cause\)\);\s*(?:setProfileError\([^\n]+\);\s*)?if \(isConnectionExpiredFailure\(cause\)\) onSessionExpired\?\.\(sessionEndReason\(cause\)\);/g);
    expect(paths?.length).toBe(5);
  });
});
