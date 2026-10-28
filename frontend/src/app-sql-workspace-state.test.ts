import { describe, expect, it } from "vitest";
import { advanceSqlExecutionAttempt, attemptForConnection, canDispatchSqlExecution, createSqlExecutionAttempt, emptySqlWorkspaceDraft, sqlWorkspaceDraftForConnection } from "./app-sql-workspace-state";

describe("App-owned SQL workspace attempt", () => {
  it("freezes SQL, plan mode, and session before dispatch", () => {
    const draft = { ...emptySqlWorkspaceDraft("connection-a"), planMode: "verbose" as const, value: " SELECT 48 \n" };
    const attempt = createSqlExecutionAttempt(draft, "attempt-a");

    expect(attempt).toEqual({ executionAttemptId: "attempt-a", connectionId: "connection-a", planMode: "verbose", sqlSnapshot: " SELECT 48 \n", phase: "confirming" });
    expect(canDispatchSqlExecution(attempt)).toBe(true);
    expect(canDispatchSqlExecution(advanceSqlExecutionAttempt(attempt!, "pending"))).toBe(false);
  });

  it("keeps draft and attempt isolated when the connection changes or the attempt becomes unknown", () => {
    const draft = { ...emptySqlWorkspaceDraft("connection-a"), value: "\n  /* keep comment */\nSELECT 1  \n" };
    const attempt = createSqlExecutionAttempt(draft, "attempt-a")!;

    expect(sqlWorkspaceDraftForConnection(draft, "connection-b")).toEqual(emptySqlWorkspaceDraft("connection-b"));
    expect(attemptForConnection(attempt, "connection-b")).toBeNull();
    const unknown = advanceSqlExecutionAttempt(advanceSqlExecutionAttempt(attempt, "pending"), "unknown");
    expect(unknown).toMatchObject({ executionAttemptId: "attempt-a", sqlSnapshot: "\n  /* keep comment */\nSELECT 1  \n", phase: "unknown" });
    // A late response cannot reopen a locally stopped wait.
    expect(advanceSqlExecutionAttempt(unknown, "pending")).toBe(unknown);
  });

  it("does not create an attempt from an empty SQL draft", () => {
    expect(createSqlExecutionAttempt(emptySqlWorkspaceDraft("connection-a"), "attempt-a")).toBeNull();
  });
});
