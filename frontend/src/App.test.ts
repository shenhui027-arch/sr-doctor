import { focusQueryWorkspaceHeading } from "./App";
import appSource from "./App.tsx?raw";
import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { admitRunningDoctorEvidence, acceptsExecutionFailure, acceptsExplainResult, advisorReadTarget, AuditFallbackBanner, canShowAuditWorkload, currentAdvisorResponse, doctorQueryForAcceptedProfile, EXECUTION_RESPONSE_UNKNOWN, executionResponseUnknownLocksSession, invalidateExecutionControllers, ownsCurrentExplainResult, profileFetchBelongsToCurrentSession, shouldDiscardReportOnDiscoveryChange, startExecutionController, type ExplainOwner } from "./App";
import { sr3514ConnectedContractGolden } from "../../contracts/generated/connected-contract-fixtures";
import { LocaleProvider } from "./i18n/locale";
import { requestFailureMessage } from "./App";
import { advanceSqlExecutionAttempt, canDispatchSqlExecution, createSqlExecutionAttempt, emptySqlWorkspaceDraft, sqlWorkspaceDraftForConnection } from "./app-sql-workspace-state";

describe("execution error lifecycle", () => {
  it.each(["explain", "runtime"] as const)("keeps independent %s execution locked across Audit discovery", (kind) => {
    const explain = { current: null as AbortController | null };
    const runtime = { current: null as AbortController | null };
    const controller = startExecutionController(kind, explain, runtime);
    expect(invalidateExecutionControllers(explain, runtime, false)).toBe(false);
    expect(controller?.signal.aborted).toBe(false);
    expect(startExecutionController("runtime", explain, runtime)).toBeNull();
    expect(invalidateExecutionControllers(explain, runtime, true)).toBe(true);
    expect(controller?.signal.aborted).toBe(true);
    expect(explain.current).toBeNull();
    expect(runtime.current).toBeNull();
  });
  it.each([
    { code: "execution_attempt_already_submitted", message_id: "execution_attempt_already_submitted", recoverability: "change_input" },
    { code: "remote_execution_unknown", message_id: "remote_execution_unknown", recoverability: "status_unknown" },
    { code: "remote_timeout", message_id: "remote_timeout", recoverability: "restart" },
  ])("locks uncertain prior Runtime execution rather than accepting another attempt: $code", failure => {
    expect(requestFailureMessage(failure, "runtime")).toBe(EXECUTION_RESPONSE_UNKNOWN);
  });
  it("locks a post-dispatch runtime abort as unknown", () => {
    expect(requestFailureMessage(new DOMException("request stopped", "AbortError"), "runtime")).toBe(EXECUTION_RESPONSE_UNKNOWN);
  });
  it("does not lock Runtime for a read-only Explain restart or abort", () => {
    expect(requestFailureMessage({ code: "remote_timeout", message_id: "remote_timeout", recoverability: "restart" }, "explain")).toBe("remote_timeout");
    expect(requestFailureMessage(new DOMException("explain cancelled", "AbortError"), "explain")).toBe("explain cancelled");
  });
  it("retains definite validation and expiry errors", () => {
    expect(requestFailureMessage({ message_id: "statement_not_allowed", recoverability: "change_input" }, "runtime")).toBe("statement_not_allowed");
    expect(requestFailureMessage({ message_id: "connection_expired", recoverability: "restart" }, "runtime")).toBe("connection_expired");
    expect(requestFailureMessage({ message_id: "connection_session_expired", recoverability: "restart" }, "runtime")).toBe("connection_session_expired");
    expect(requestFailureMessage(new Error("local_error"), "runtime")).toBe("local_error");
  });
});

const owner = (connectionId: string, generation: number): ExplainOwner => ({ connectionId, generation });

describe("Explain owner state", () => {
  it("keeps one SQL draft for a current connection and clears it across A to B to A", () => {
    const draftA = { ...emptySqlWorkspaceDraft("connection-a"), value: "SELECT\n  48" };

    expect(sqlWorkspaceDraftForConnection(draftA, "connection-a")).toBe(draftA);
    expect(sqlWorkspaceDraftForConnection(draftA, "connection-b")).toEqual(emptySqlWorkspaceDraft("connection-b"));

    const draftB = emptySqlWorkspaceDraft("connection-b");
    expect(sqlWorkspaceDraftForConnection(draftB, "connection-a")).toEqual(emptySqlWorkspaceDraft("connection-a"));
  });
  it("uses audit workspaces only for a ready session with available audit capability", () => {
    const session = sr3514ConnectedContractGolden.connection;
    expect(canShowAuditWorkload(null)).toBe(false);
    expect(canShowAuditWorkload(session)).toBe(true);
    expect(canShowAuditWorkload({ ...session, state: "expired" })).toBe(false);
    expect(canShowAuditWorkload({ ...session, capabilities: { ...session.capabilities, audit: "unavailable" } })).toBe(false);
  });
  it("renders the actual Profile/Compare fallback reason when a ready connection cannot read Audit", () => {
    const session = { ...sr3514ConnectedContractGolden.connection, capabilities: { ...sr3514ConnectedContractGolden.connection.capabilities, audit: "unavailable" as const } };
    const fallback = renderToStaticMarkup(createElement(LocaleProvider, {
      initialLocale: "en",
      children: createElement(AuditFallbackBanner, { session, workspace: "Queries" }),
    }));
    const available = renderToStaticMarkup(createElement(LocaleProvider, {
      initialLocale: "en",
      children: createElement(AuditFallbackBanner, { session: sr3514ConnectedContractGolden.connection, workspace: "Queries" }),
    }));
    expect(fallback).toContain("Audit queries are not available on this connection.");
    expect(fallback).toContain("You can still select an existing Profile or import one to analyze.");
    expect(available).toBe("");
  });
  it("renders only the result owned by the current connection and generation", () => {
    expect(ownsCurrentExplainResult(owner("connection-a", 3), owner("connection-a", 3))).toBe(true);
    expect(ownsCurrentExplainResult(owner("connection-a", 3), owner("connection-b", 3))).toBe(false);
    expect(ownsCurrentExplainResult(owner("connection-a", 3), owner("connection-a", 4))).toBe(false);
    expect(ownsCurrentExplainResult(owner("connection-a", 3), null)).toBe(false);
    expect(ownsCurrentExplainResult(null, null)).toBe(false);
  });

  it("rejects an aborted or superseded Explain completion", () => {
    const controller = new AbortController();
    expect(acceptsExplainResult(owner("connection-a", 3), 3, controller.signal)).toBe(true);
    expect(acceptsExplainResult(owner("connection-a", 3), 4, controller.signal)).toBe(false);
    controller.abort();
    expect(acceptsExplainResult(owner("connection-a", 3), 3, controller.signal)).toBe(false);
    expect(acceptsExecutionFailure(owner("connection-a", 3), 3)).toBe(true);
    expect(acceptsExecutionFailure(owner("connection-a", 3), 4)).toBe(false);
  });

  it("invalidates discovery-owned reports but retains independent executed reports on discovery rotation", () => {
    expect(shouldDiscardReportOnDiscoveryChange("connected_profile_fetch")).toBe(true);
    expect(shouldDiscardReportOnDiscoveryChange("candidate_profile_user_external_rerun")).toBe(true);
    expect(shouldDiscardReportOnDiscoveryChange("offline_import")).toBe(false);
    expect(shouldDiscardReportOnDiscoveryChange("connected_sql_runtime")).toBe(false);
    expect(shouldDiscardReportOnDiscoveryChange(null)).toBe(false);
  });

  it("derives the Doctor official identity only from the accepted fetched Profile", () => {
    const profile = sr3514ConnectedContractGolden.profile_fetch_response;
    const session = sr3514ConnectedContractGolden.connection;
    expect(doctorQueryForAcceptedProfile(profile, session)).toBe(profile.query);
    expect(doctorQueryForAcceptedProfile({ ...profile, query: { ...profile.query, discovery_generation: null } }, session)).toBeNull();
    expect(doctorQueryForAcceptedProfile(profile, { ...session, connection_id: "rotated" })).toBeNull();
  });
  it("accepts complete fetched Profile evidence only from the current ready session", () => {
    const profile = sr3514ConnectedContractGolden.profile_fetch_response;
    const session = sr3514ConnectedContractGolden.connection;
    expect(profileFetchBelongsToCurrentSession(profile, session)).toBe(true);
    expect(profileFetchBelongsToCurrentSession(profile, { ...session, connection_id: "new-session" })).toBe(false);
    expect(profileFetchBelongsToCurrentSession(profile, { ...session, state: "expired" })).toBe(false);
  });
});

describe("Connected execution race", () => {
  const refs = () => ({
    explain: { current: null as AbortController | null },
    runtime: { current: null as AbortController | null },
  });

  it("starts only one Runtime request for same-tick duplicate submissions", () => {
    const controllers = refs();
    let started = 0;
    const submit = () => {
      const controller = startExecutionController("runtime", controllers.explain, controllers.runtime);
      if (controller) started += 1;
    };

    submit();
    submit();

    expect(started).toBe(1);
    expect(controllers.runtime.current).not.toBeNull();
  });

  it("turns a confirmed attempt pending before a second confirmation can claim a POST slot", () => {
    const controllers = refs();
    let attempt = createSqlExecutionAttempt({ ...emptySqlWorkspaceDraft("connection-a"), value: "\nSELECT 1 -- preserve\n" }, "attempt-a")!;
    let posts = 0;
    const confirm = () => {
      if (!canDispatchSqlExecution(attempt)) return;
      const controller = startExecutionController("runtime", controllers.explain, controllers.runtime);
      if (!controller) return;
      attempt = advanceSqlExecutionAttempt(attempt, "pending");
      posts += 1;
    };

    confirm();
    confirm();

    expect(posts).toBe(1);
    expect(attempt).toMatchObject({ phase: "pending", sqlSnapshot: "\nSELECT 1 -- preserve\n" });
  });

  it("keeps execution_response_unknown terminal for the same session", () => {
    const controllers = refs();
    const lockedConnectionId = "connection-a";
    expect(EXECUTION_RESPONSE_UNKNOWN).toBe("execution_response_unknown");
    expect(executionResponseUnknownLocksSession(lockedConnectionId, "connection-a")).toBe(true);
    expect(executionResponseUnknownLocksSession(lockedConnectionId, "connection-b")).toBe(false);
    expect(executionResponseUnknownLocksSession(null, "connection-a")).toBe(false);

    const explain = startExecutionController("explain", controllers.explain, controllers.runtime, true);
    const runtime = startExecutionController("runtime", controllers.explain, controllers.runtime, true);

    expect(explain).toBeNull();
    expect(runtime).toBeNull();
    expect(controllers.explain.current).toBeNull();
    expect(controllers.runtime.current).toBeNull();

    const replacement = startExecutionController(
      "runtime",
      controllers.explain,
      controllers.runtime,
      executionResponseUnknownLocksSession(lockedConnectionId, "connection-b"),
    );
    expect(replacement).not.toBeNull();
  });

  it("keeps an in-flight Runtime request when Explain is requested", () => {
    const controllers = refs();
    const runtime = startExecutionController("runtime", controllers.explain, controllers.runtime);
    const explain = startExecutionController("explain", controllers.explain, controllers.runtime);

    expect(runtime).not.toBeNull();
    expect(explain).toBeNull();
    expect(runtime?.signal.aborted).toBe(false);
    expect(controllers.runtime.current).toBe(runtime);
  });

  it("rejects an old completion after the connected session is replaced", () => {
    const controllers = refs();
    const oldController = startExecutionController("runtime", controllers.explain, controllers.runtime);
    const oldOwner = owner("connection-a", 3);
    invalidateExecutionControllers(controllers.explain, controllers.runtime);

    expect(oldController?.signal.aborted).toBe(true);
    expect(acceptsExplainResult(oldOwner, 4, new AbortController().signal)).toBe(false);
    expect(acceptsExecutionFailure(oldOwner, 4)).toBe(false);
    expect(ownsCurrentExplainResult(oldOwner, owner("connection-b", 4))).toBe(false);
  });
});

describe("Running Doctor retained artifact admission", () => {
  it("admits the exact typed artifact without exposing fetched authority", () => {
    const snapshot = structuredClone(sr3514ConnectedContractGolden.running_profile_snapshot_response);
    const session = structuredClone(sr3514ConnectedContractGolden.connection);
    session.connection_id = snapshot.query.connection_id;
    const evidence = admitRunningDoctorEvidence(snapshot, session);
    expect(evidence?.kind).toBe("running_snapshot");
    expect(evidence?.value).toBe(snapshot);
    expect(snapshot.profile_lifecycle).toBe(snapshot.outcome.report.profile_context.query_state);
  });
  it("rejects disconnected, cross-session and incoherent retained artifacts", () => {
    const snapshot = structuredClone(sr3514ConnectedContractGolden.running_profile_snapshot_response);
    const session = structuredClone(sr3514ConnectedContractGolden.connection);
    expect(admitRunningDoctorEvidence(snapshot, null)).toBeNull();
    session.connection_id = "other";
    expect(admitRunningDoctorEvidence(snapshot, session)).toBeNull();
    session.connection_id = snapshot.query.connection_id;
    snapshot.analysis_key = "wrong";
    expect(admitRunningDoctorEvidence(snapshot, session)).toBeNull();
  });
});
it("preserves connected evidence when opening Import Profile (source wiring guard)", async () => {
  const source = (await import("./App.tsx?raw")).default;
  expect(source).toContain('const openImport = () => { setImporting(true); setWorkspace("Doctor"); };');
  expect(source).toMatch(/<button className="button accent header-import" onClick=\{openImport\}[^\n]*\{text\("Import profile"/);
});


it("keeps every query-picking mode in one switch and execution state owned by App", () => {
  expect(appSource).toContain("const queryModes: { id: QueryMode; label: string; available: boolean }[] = [");
  expect(appSource).toContain("sql={currentExecutionAttempt?.sqlSnapshot ?? sqlDraft.value}");
  expect(appSource).not.toContain("nativeExplainAnalyze");
  expect(appSource).not.toContain('"Overview"');
});


it("focuses the target heading only for an explicit view switch and returns Doctor to the list it came from", () => {
  const heading = { tabIndex: 0, focus: vi.fn() };
  const querySelector = vi.fn(() => heading);
  focusQueryWorkspaceHeading({ querySelector } as unknown as HTMLElement);
  expect(querySelector).toHaveBeenCalledWith("h1, h2");
  expect(heading.tabIndex).toBe(-1);
  expect(heading.focus).toHaveBeenCalledOnce();
  expect(() => focusQueryWorkspaceHeading(null)).not.toThrow();
  expect(appSource).toContain('const queryFocusRequested = useRef(false)');
  expect(appSource).toContain('if (!queryFocusRequested.current) return;');
  expect(appSource).toContain('onReturnToQueries: () => setWorkspace("Queries")');
  expect(appSource).toContain('<div className="queries-shell" hidden={workspace !== "Queries"}>');
});

it("releases only a ready connection session when the page is hidden for unload", () => {
  expect(appSource).toContain('const readySessionId = connectionSession?.state === "ready" ? connectionSession.connection_id : null;');
  expect(appSource).toContain("if (!readySessionId) return;");
  expect(appSource).toContain('window.addEventListener("pagehide", release);');
  expect(appSource).toContain('return () => window.removeEventListener("pagehide", release);');
});

describe("Official Advisor ownership", () => {
  const session = sr3514ConnectedContractGolden.connection;
  const advisor = sr3514ConnectedContractGolden.advisor_response;

  it("requests guides only for a ready session whose Plan Advisor probe is available", () => {
    expect(advisorReadTarget(null)).toBeNull();
    expect(advisorReadTarget(session)).toBe(session.connection_id);
    expect(advisorReadTarget({ ...session, state: "expired" })).toBeNull();
    for (const state of ["unavailable", "forbidden", "unknown"] as const) {
      expect(advisorReadTarget({ ...session, capabilities: { ...session.capabilities, plan_advisor: state } })).toBeNull();
    }
  });
  it("shows a guide only to the session that read it", () => {
    const owned = { connectionId: session.connection_id, value: advisor };
    expect(currentAdvisorResponse(owned, session)).toBe(advisor);
    expect(currentAdvisorResponse(null, session)).toBeNull();
    expect(currentAdvisorResponse(owned, null)).toBeNull();
    expect(currentAdvisorResponse(owned, { ...session, connection_id: "conn_other" })).toBeNull();
    expect(currentAdvisorResponse(owned, { ...session, capabilities: { ...session.capabilities, plan_advisor: "forbidden" } })).toBeNull();
  });
  it("wires the owned guide into Recommendations without feeding product rules (source wiring guard)", () => {
    expect(appSource).toContain("advisorResponse={currentAdvisorResponse(advisor, advisorSession)}");
    // Without an analysis the page keeps its start hint; Advisor is supplementary.
    expect(appSource).toContain('const advisorSession = connection.phase === "ready" && doctorReport ? connectionSession : null;');
    expect(appSource).toContain('if (workspace !== "Recommendations" || !advisorTarget || advisorRequestedRef.current === advisorTarget) return;');
    expect(appSource).toContain("doctorApi.readAdvisorGuides({ connection_id: advisorTarget }, controller.signal)");
    // Product findings stay report-owned; Advisor never reaches Doctor or Compare.
    expect(appSource.match(/advisorResponse=/g)).toHaveLength(1);
    expect(appSource).not.toMatch(/<(?:DoctorWorkspace|CompareWorkspace)[^>]*advisor/);
  });
});
