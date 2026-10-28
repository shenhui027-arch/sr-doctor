import type { ExplainMode } from "../../contracts/generated/doctor-contracts";

/** App owns this editable intent; the SQL workspace only renders it. */
export type SqlWorkspaceDraft = Readonly<{
  connectionId: string | null;
  value: string;
  planMode: ExplainMode;
}>;

export type SqlExecutionPhase = "confirming" | "pending" | "unknown";

/**
 * An attempt is created before the request is dispatched, so the exact request
 * identity is stable across confirmation, pending and unknown views.
 */
export type SqlExecutionAttempt = Readonly<{
  executionAttemptId: string;
  connectionId: string;
  sqlSnapshot: string;
  planMode: ExplainMode;
  phase: SqlExecutionPhase;
}>;

export function emptySqlWorkspaceDraft(connectionId: string | null): SqlWorkspaceDraft {
  return { connectionId, value: "", planMode: "costs" };
}

export function sqlWorkspaceDraftForConnection(draft: SqlWorkspaceDraft, connectionId: string | null): SqlWorkspaceDraft {
  return draft.connectionId === connectionId ? draft : emptySqlWorkspaceDraft(connectionId);
}

export function createSqlExecutionAttempt(draft: SqlWorkspaceDraft, executionAttemptId: string): SqlExecutionAttempt | null {
  if (!draft.connectionId || !draft.value.trim() || !executionAttemptId) return null;
  return {
    executionAttemptId,
    connectionId: draft.connectionId,
    // Whitespace, comments, and newlines are part of the user-confirmed input.
    sqlSnapshot: draft.value,
    planMode: draft.planMode,
    phase: "confirming",
  };
}

export function attemptForConnection(attempt: SqlExecutionAttempt | null, connectionId: string | null): SqlExecutionAttempt | null {
  return attempt !== null && attempt.connectionId === connectionId ? attempt : null;
}

export function advanceSqlExecutionAttempt(attempt: SqlExecutionAttempt, phase: Exclude<SqlExecutionPhase, "confirming">): SqlExecutionAttempt {
  const allowed = attempt.phase === "confirming" ? phase === "pending" : attempt.phase === "pending" ? phase === "unknown" : false;
  return allowed ? { ...attempt, phase } : attempt;
}

export function canDispatchSqlExecution(attempt: SqlExecutionAttempt | null): attempt is SqlExecutionAttempt {
  return attempt !== null && attempt.phase === "confirming";
}
