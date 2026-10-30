import { Fragment, lazy, Suspense, useCallback, useEffect, useReducer, useRef, useState } from "react";
import type { JSX } from "react";
import type { AnalysisSnapshotReport, Comparison, ConnectedProfileComparisonResponse, ConnectedQueryRef, ConnectionSessionView, ExplainMode, ExplainRequest, ExplainSnapshot, InputOrigin, PlanAdvisorResponse, ProfileFetchResponse, RunningProfileSnapshotResponse } from "../../contracts/generated/doctor-contracts";
import { ConnectionDialog } from "./features/connection/ConnectionDialog";
import { connectionReducer, initialConnectionState } from "./features/connection/connection-state";
import { DoctorWorkspace } from "./features/doctor/DoctorWorkspace";
import { ReportExportButton } from "./features/export/ReportExportButton";
import { ContextBar } from "./features/shell/ContextBar";
import { HeaderStatusText } from "./features/shell/HeaderStatusText";
import { FirstRunPage } from "./features/start/FirstRunPage";
import { runningSnapshotMatchesBinding } from "./features/execution/running-official-binding";
import { RecommendationsPanel, recommendedFindings } from "./features/recommendations/RecommendationsPanel";
import { QueriesWorkspace } from "./features/queries/QueriesWorkspace";
import { QueryCapabilityChips } from "./features/queries/QueryCapabilityChips";
import { SqlAnalysisWorkspace, type SqlAnalysisWorkspaceStage } from "./features/plan/SqlAnalysisWorkspace";
import { advanceSqlExecutionAttempt, attemptForConnection, canDispatchSqlExecution, createSqlExecutionAttempt, emptySqlWorkspaceDraft, type SqlExecutionAttempt, type SqlWorkspaceDraft } from "./app-sql-workspace-state";
import { useLocale, type LocalizedText } from "./i18n/locale";
import { doctorApi } from "./lib/api";
import { CollectionCapsule, monitorGeneration, useHistoryStatus, type MonitoringTab } from "./features/history/history-status";
import type { LoadFocus } from "./features/loads/LoadWorkspace";

// Connected audit UI is not needed for offline Profile diagnosis.
const AuditWorkloadWorkspace = lazy(() => import("./features/audit/AuditWorkloadWorkspace").then((module) => ({ default: module.AuditWorkloadWorkspace })));
// Compare and the monitoring pages are opened on demand; they stay out of the first load.
const CompareWorkspace = lazy(() => import("./features/compare/CompareWorkspace").then((module) => ({ default: module.CompareWorkspace })));
const PendingComparison = lazy(() => import("./features/compare/CompareWorkspace").then((module) => ({ default: module.PendingComparison })));
const OfflineCompareForm = lazy(() => import("./features/compare/OfflineCompareForm").then((module) => ({ default: module.OfflineCompareForm })));
const HistoryWorkspace = lazy(() => import("./features/history/HistoryWorkspace").then((module) => ({ default: module.HistoryWorkspace })));
const AlertsWorkspace = lazy(() => import("./features/history/MonitoringWorkspaces").then((module) => ({ default: module.AlertsWorkspace })));
const GovernanceWorkspace = lazy(() => import("./features/history/MonitoringWorkspaces").then((module) => ({ default: module.GovernanceWorkspace })));
const LoadWorkspace = lazy(() => import("./features/loads/LoadWorkspace").then((module) => ({ default: module.LoadWorkspace })));
const RunningProfileEntry = lazy(() => import("./features/queries/RunningProfileEntry").then((module) => ({ default: module.RunningProfileEntry })));

const workspaces = ["Queries", "Doctor", "Recommendations", "Compare"] as const;
/** v1.0 monitoring views sit after the numbered flow, unnumbered. */
const monitoringWorkspaces = ["Trends", "Alerts", "Governance", "Loads"] as const;
export type Workspace = (typeof workspaces)[number] | (typeof monitoringWorkspaces)[number];
/** One switch for every way to pick a query; nothing else lives in the header. */
export type QueryMode = "audit" | "profiles" | "running" | "sql";

export type ExplainOwner = { connectionId: string; generation: number };
type OwnedExplain<T> = { owner: ExplainOwner; value: T; mode: ExplainMode };
export type DoctorEvidence = { kind: "profile_fetch"; value: ProfileFetchResponse } | { kind: "running_snapshot"; value: RunningProfileSnapshotResponse };

/** Retained artifact admission only; this never grants a new remote read. */
export function admitRunningDoctorEvidence(snapshot: RunningProfileSnapshotResponse, session: ConnectionSessionView | null): DoctorEvidence | null {
  if (session?.state !== "ready" || snapshot.query.connection_id !== session.connection_id
    || !snapshot.query.query_id || snapshot.query.discovery_generation === null) return null;
  const matches = runningSnapshotMatchesBinding(snapshot, {
    connection_id: session.connection_id, query_id: snapshot.query.query_id,
    identity: { kind: "discovery", discovery_generation: snapshot.query.discovery_generation }, node_ids: [],
  });
  return matches ? { kind: "running_snapshot", value: snapshot } : null;
}

export function profileFetchBelongsToCurrentSession(profile: ProfileFetchResponse, session: ConnectionSessionView | null): boolean {
  return session?.state === "ready" && profile.query.connection_id === session.connection_id;
}

/**
 * The Doctor identity is an accepted fetched Profile, not the current row in a
 * query list. The official reader performs the stricter report/object binding
 * check later; this helper only prevents an incomplete fetch from becoming
 * current Doctor state.
 */
export function doctorQueryForAcceptedProfile(profile: ProfileFetchResponse, session: ConnectionSessionView | null): ConnectedQueryRef | null {
  if (!profileFetchBelongsToCurrentSession(profile, session)
    || profile.state !== "fetched"
    || !profile.query.connection_id
    || !profile.query.query_id
    || profile.query.discovery_generation === null
    || !Number.isSafeInteger(profile.query.discovery_generation)
    || profile.query.discovery_generation <= 0) return null;
  return profile.query;
}

/** Advisor guides belong to the connected FE that produced them, not to a Profile. */
export type OwnedAdvisorResponse = { connectionId: string; value: PlanAdvisorResponse };

/** A pulled Profile of one session, chosen as the before side of a connected comparison. */
type CompareBaseline = { connectionId: string; query: ConnectedQueryRef; report: AnalysisSnapshotReport | null };

/** Only a ready session whose probe found `SHOW PLAN ADVISOR` readable may request guides. */
export function advisorReadTarget(session: ConnectionSessionView | null): string | null {
  return session?.state === "ready" && session.capabilities.plan_advisor === "available" ? session.connection_id : null;
}

export function currentAdvisorResponse(owned: OwnedAdvisorResponse | null, session: ConnectionSessionView | null): PlanAdvisorResponse | null {
  const target = advisorReadTarget(session);
  return owned !== null && target !== null && owned.connectionId === target ? owned.value : null;
}

export function ownsCurrentExplainResult(owner: ExplainOwner | null, current: ExplainOwner | null): boolean {
  return owner !== null && current !== null && owner.connectionId === current.connectionId && owner.generation === current.generation;
}

export function acceptsExplainResult(owner: ExplainOwner, generation: number, signal: AbortSignal): boolean {
  return !signal.aborted && owner.generation === generation;
}

export function acceptsExecutionFailure(owner: ExplainOwner, generation: number): boolean {
  return owner.generation === generation;
}

export const EXECUTION_RESPONSE_UNKNOWN = "execution_response_unknown";

export function executionResponseUnknownLocksSession(lockedConnectionId: string | null, currentConnectionId: string | null): boolean {
  return lockedConnectionId !== null && currentConnectionId !== null && lockedConnectionId === currentConnectionId;
}

export type ExecutionControllerRef = { current: AbortController | null };
export type ExecutionKind = "explain" | "runtime";

/**
 * Claims the single connected execution slot without letting a Runtime request
 * be superseded by a plan request. A repeated Runtime submit returns null
 * before a second controller (and therefore a second attempt) is created.
 */
export function startExecutionController(kind: ExecutionKind, explainControllerRef: ExecutionControllerRef, runtimeControllerRef: ExecutionControllerRef, unknownLocked = false): AbortController | null {
  if (unknownLocked) return null;
  if (kind === "runtime" && (runtimeControllerRef.current || explainControllerRef.current)) return null;
  if (kind === "explain" && runtimeControllerRef.current) return null;
  if (kind === "explain") explainControllerRef.current?.abort();
  const controller = new AbortController();
  (kind === "explain" ? explainControllerRef : runtimeControllerRef).current = controller;
  return controller;
}

export function invalidateExecutionControllers(explainControllerRef: ExecutionControllerRef, runtimeControllerRef: ExecutionControllerRef, invalidate = true): boolean {
  if (!invalidate) return false;
  explainControllerRef.current?.abort();
  explainControllerRef.current = null;
  runtimeControllerRef.current?.abort();
  runtimeControllerRef.current = null;
  return true;
}

export function focusQueryWorkspaceHeading(container: HTMLElement | null): void {
  const heading = container?.querySelector<HTMLElement>("h1, h2");
  if (!heading) return;
  heading.tabIndex = -1;
  heading.focus();
}

export function App(): JSX.Element {
  const { locale, setLocale, text } = useLocale();
  const [workspace, setWorkspace] = useState<Workspace>("Queries");
  const [focusNodeId, setFocusNodeId] = useState<string | null>(null);
  const [queryMode, setQueryMode] = useState<QueryMode | null>(null);
  const queryViewRef = useRef<HTMLDivElement>(null);
  const queryFocusRequested = useRef(false);
  const [importing, setImporting] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [connection, dispatchConnection] = useReducer(connectionReducer, initialConnectionState);
  const [capabilityError, setCapabilityError] = useState<string | null>(null);
  const [maxInputBytes, setMaxInputBytes] = useState<number | null>(null);
  const [report, setReport] = useState<AnalysisSnapshotReport | null>(null);
  const [reportOrigin, setReportOrigin] = useState<InputOrigin | null>(null);
  const [doctorEvidence, setDoctorEvidence] = useState<DoctorEvidence | null>(null);
  const [currentDoctorQuery, setCurrentDoctorQuery] = useState<ConnectedQueryRef | null>(null);
  const executionDependsOnDiscoveryRef = useRef(false);
  const [comparison, setComparison] = useState<Comparison | null>(null);
  const [connectedComparison, setConnectedComparison] = useState<ConnectedProfileComparisonResponse | null>(null);
  // The baseline of a connected before/after comparison: a pulled Profile of this session.
  // The backend compares it with a candidate discovered in the same query list.
  const [compareBaseline, setCompareBaseline] = useState<CompareBaseline | null>(null);
  const [swapError, setSwapError] = useState<string | null>(null);
  const [explainSnapshot, setExplainSnapshot] = useState<OwnedExplain<ExplainSnapshot> | null>(null);
  const [explainError, setExplainError] = useState<OwnedExplain<string> | null>(null);
  const [explaining, setExplaining] = useState<ExplainOwner | null>(null);
  const [sqlDraft, setSqlDraft] = useState<SqlWorkspaceDraft>(() => emptySqlWorkspaceDraft(null));
  const [executionAttempt, setExecutionAttempt] = useState<SqlExecutionAttempt | null>(null);
  const [advisor, setAdvisor] = useState<OwnedAdvisorResponse | null>(null);
  const advisorRequestedRef = useRef<string | null>(null);
  const connectionTriggerRef = useRef<HTMLButtonElement>(null);
  const explainControllerRef = useRef<AbortController | null>(null);
  const runtimeControllerRef = useRef<AbortController | null>(null);
  const executionUnknownSessionRef = useRef<string | null>(null);
  const explainRequestGeneration = useRef(0);
  const connectionSession = connection.phase === "connecting" ? null : connection.session;
  const readySessionId = connectionSession?.state === "ready" ? connectionSession.connection_id : null;
  useEffect(() => {
    if (!readySessionId) return;
    // A reload or closed tab would otherwise hold one of the few local sessions until it expires.
    const release = () => doctorApi.releaseConnectionOnUnload(readySessionId);
    window.addEventListener("pagehide", release);
    return () => window.removeEventListener("pagehide", release);
  }, [readySessionId]);
  const currentExplainOwner = connection.phase === "ready" && connectionSession ? { connectionId: connectionSession.connection_id, generation: explainRequestGeneration.current } : null;
  const ownsSnapshot = ownsCurrentExplainResult(explainSnapshot?.owner ?? null, currentExplainOwner);
  const ownsError = ownsCurrentExplainResult(explainError?.owner ?? null, currentExplainOwner);
  const currentExplainSnapshot = ownsSnapshot ? explainSnapshot?.value ?? null : null;
  const currentExplainSnapshotMode = ownsSnapshot ? explainSnapshot?.mode ?? null : null;
  const currentExplainError = executionResponseUnknownLocksSession(executionUnknownSessionRef.current, connectionSession?.connection_id ?? null)
    ? EXECUTION_RESPONSE_UNKNOWN
    : ownsError ? explainError?.value ?? null : null;
  const currentExplainErrorMode = ownsError ? explainError?.mode ?? null : null;
  const currentExecutionAttempt = attemptForConnection(executionAttempt, connectionSession?.connection_id ?? null);
  const sqlWorkspaceStage: SqlAnalysisWorkspaceStage = currentExecutionAttempt === null
    ? "draft"
    : currentExecutionAttempt.phase === "confirming" ? "confirm"
      : currentExecutionAttempt.phase === "unknown" ? "unknown" : "pending";
  const doctorReport = doctorEvidence?.value.outcome.report ?? report;
  const doctorReportOrigin = doctorEvidence?.value.input_origin ?? reportOrigin;
  const history = useHistoryStatus(readySessionId);
  const [monitoringTab, setMonitoringTab] = useState<MonitoringTab>("trends");
  const [trendFocus, setTrendFocus] = useState<string | null>(null);
  const [governanceFocus, setGovernanceFocus] = useState<string | null>(null);
  // The cluster the monitoring views show: the user's pick, else the one this
  // session is connected to (the status poll's target). A new session resets it.
  const [pickedTarget, setPickedTarget] = useState<string | null>(null);
  useEffect(() => { setPickedTarget(null); }, [readySessionId]);
  const monitorTarget = pickedTarget ?? history.status?.target ?? null;
  const [knownTargets, setKnownTargets] = useState<string[]>([]);
  const openTrends = (templateKey: string, target: string) => { if (target) setPickedTarget(target); setTrendFocus(templateKey); setMonitoringTab("trends"); setWorkspace("Trends"); };
  const openGovernanceItem = (itemId: string) => { setGovernanceFocus(itemId); setWorkspace("Governance"); };
  const openAlerts = Number(history.status?.open_alerts ?? "0");
  // Paused Routine Load jobs as the load workspace last read them; a new session starts unknown.
  const [pausedLoads, setPausedLoads] = useState<{ connectionId: string; count: number } | null>(null);
  const [loadFocus, setLoadFocus] = useState<LoadFocus | null>(null);
  const openLoads = (focus: LoadFocus) => { setLoadFocus(focus); setWorkspace("Loads"); };
  const pausedLoadCount = pausedLoads && pausedLoads.connectionId === readySessionId ? pausedLoads.count : 0;
  const connected = (connection.phase === "ready" || connection.phase === "unsupported") && connectionSession !== null;
  const auditReady = canShowAuditWorkload(connectionSession);
  const activeQueryMode: QueryMode = queryMode ?? (auditReady ? "audit" : "profiles");

  const switchQueryMode = (mode: QueryMode) => {
    queryFocusRequested.current = true;
    setQueryMode(mode);
  };
  useEffect(() => {
    if (!queryFocusRequested.current) return;
    queryFocusRequested.current = false;
    focusQueryWorkspaceHeading(queryViewRef.current);
  }, [activeQueryMode]);
  useEffect(() => { if (queryMode === "audit" && !auditReady) setQueryMode(null); }, [auditReady, queryMode]);

  const invalidateConnectedArtifacts = useCallback((sessionInvalidated = false) => {
    // Audit discovery does not own a pasted-SQL plan or an executing SELECT.
    // Cancelling either here would lose its result and release the one-shot lock.
    if (invalidateExecutionControllers(explainControllerRef, runtimeControllerRef, sessionInvalidated || executionDependsOnDiscoveryRef.current)) {
      explainRequestGeneration.current += 1;
      setExplainSnapshot(null);
      setExplainError(null);
      setExplaining(null);
    }
    if (sessionInvalidated || shouldDiscardReportOnDiscoveryChange(reportOrigin)) {
      setDoctorEvidence(null);
      setCurrentDoctorQuery(null);
    }
    if ((sessionInvalidated && reportOrigin !== "offline_import") || shouldDiscardReportOnDiscoveryChange(reportOrigin)) {
      setReport(null);
      setReportOrigin(null);
    }
    if (connectedComparison) {
      setComparison(null);
      setConnectedComparison(null);
    }
  }, [connectedComparison, reportOrigin]);

  const expireConnection = useCallback((reason?: "remote_timeout") => {
    invalidateConnectedArtifacts(true);
    dispatchConnection({ type: "expire", message: reason ?? "connection_expired" });
  }, [invalidateConnectedArtifacts]);

  // One read-only guide read per ready session, only once Recommendations shows an analysis.
  // Without a report the page keeps its start hint (open queries / import) instead of an Advisor-only view.
  const advisorSession = connection.phase === "ready" && doctorReport ? connectionSession : null;
  const advisorTarget = advisorReadTarget(advisorSession);
  useEffect(() => {
    if (workspace !== "Recommendations" || !advisorTarget || advisorRequestedRef.current === advisorTarget) return;
    advisorRequestedRef.current = advisorTarget;
    const controller = new AbortController();
    let settled = false;
    doctorApi.readAdvisorGuides({ connection_id: advisorTarget }, controller.signal)
      .then((value) => { settled = true; if (!controller.signal.aborted) setAdvisor({ connectionId: advisorTarget, value }); })
      .catch((cause: unknown) => {
        settled = true;
        if (controller.signal.aborted) return;
        // Any other failure leaves the guide absent; it never blocks product recommendations.
        const message = requestFailureMessage(cause, "explain");
        if (message === "connection_expired" || message === "connection_session_expired") expireConnection();
      });
    return () => {
      controller.abort();
      // A read abandoned before it settled may be retried on the next visit.
      if (!settled && advisorRequestedRef.current === advisorTarget) advisorRequestedRef.current = null;
    };
  }, [advisorTarget, expireConnection, workspace]);

  useEffect(() => {
    const controller = new AbortController();
    doctorApi.capabilities(controller.signal).then((capabilities) => setMaxInputBytes(capabilities.limits.single_input_bytes ?? null)).catch((error: unknown) => {
      if (!controller.signal.aborted) setCapabilityError(error instanceof Error ? error.message : "local_api_unavailable");
    });
    return () => controller.abort();
  }, []);

  useEffect(() => {
    executionUnknownSessionRef.current = null;
    setSqlDraft(emptySqlWorkspaceDraft(connectionSession?.connection_id ?? null));
    setExecutionAttempt(null);
    explainRequestGeneration.current += 1;
    invalidateExecutionControllers(explainControllerRef, runtimeControllerRef);
    setExplainSnapshot(null);
    setExplainError(null);
    setExplaining(null);
    setDoctorEvidence(null);
    setCurrentDoctorQuery(null);
    setCompareBaseline(null);
    if (connectedComparison) {
      setComparison(null);
      setConnectedComparison(null);
    }
    return () => {
      executionUnknownSessionRef.current = null;
      explainRequestGeneration.current += 1;
      invalidateExecutionControllers(explainControllerRef, runtimeControllerRef);
    };
  }, [connection.phase, connectionSession?.connection_id]);

  const showDoctor = (next: { report: AnalysisSnapshotReport; origin: InputOrigin; evidence: DoctorEvidence | null; query: ConnectedQueryRef | null }) => {
    setImporting(false);
    setDoctorEvidence(next.evidence);
    setCurrentDoctorQuery(next.query);
    setReport(next.report);
    setReportOrigin(next.origin);
    setComparison(null);
    setConnectedComparison(null);
    setFocusNodeId(null);
    setWorkspace("Doctor");
  };

  const requestExplain = useCallback(async (request: ExplainRequest): Promise<void> => {
    const session = connectionSession;
    if (session && executionResponseUnknownLocksSession(executionUnknownSessionRef.current, session.connection_id)) return;
    if (!session || connection.phase !== "ready" || request.connection_id !== session.connection_id) {
      setExplainError(null);
      return;
    }
    const controller = startExecutionController("explain", explainControllerRef, runtimeControllerRef, executionResponseUnknownLocksSession(executionUnknownSessionRef.current, session.connection_id));
    if (!controller) return;
    executionDependsOnDiscoveryRef.current = request.input_kind === "plan_sql_reference";
    const generation = ++explainRequestGeneration.current;
    const owner = { connectionId: session.connection_id, generation };
    setExplaining(owner);
    setExplainError(null);
    setExplainSnapshot(null);
    try {
      const response = await doctorApi.explain(request, controller.signal);
      if (acceptsExplainResult(owner, explainRequestGeneration.current, controller.signal)) setExplainSnapshot({ owner, value: response.snapshot, mode: request.mode });
    } catch (cause) {
      if (acceptsExplainResult(owner, explainRequestGeneration.current, controller.signal)) {
        const message = requestFailureMessage(cause, "explain");
        if (message === EXECUTION_RESPONSE_UNKNOWN) executionUnknownSessionRef.current = owner.connectionId;
        setExplainError({ owner, value: message, mode: request.mode });
        if (message === "connection_expired" || message === "connection_session_expired") expireConnection();
      }
    } finally {
      if (explainControllerRef.current === controller) explainControllerRef.current = null;
      if (owner.generation === explainRequestGeneration.current) setExplaining(null);
    }
  }, [connection.phase, connectionSession, expireConnection]);

  const dispatchExecutionAttempt = useCallback(async (attempt: SqlExecutionAttempt): Promise<void> => {
    const session = connectionSession;
    if (session && executionResponseUnknownLocksSession(executionUnknownSessionRef.current, session.connection_id)) return;
    if (!session || connection.phase !== "ready" || attempt.connectionId !== session.connection_id || !canDispatchSqlExecution(attempt)) {
      setExplainError(null);
      return;
    }
    const controller = startExecutionController("runtime", explainControllerRef, runtimeControllerRef, executionResponseUnknownLocksSession(executionUnknownSessionRef.current, session.connection_id));
    if (!controller) return;
    executionDependsOnDiscoveryRef.current = false;
    setDoctorEvidence(null);
    setCurrentDoctorQuery(null);
    const generation = ++explainRequestGeneration.current;
    const owner = { connectionId: session.connection_id, generation };
    setExplainError(null);
    setExplainSnapshot(null);
    setExecutionAttempt((current) => current?.executionAttemptId === attempt.executionAttemptId ? advanceSqlExecutionAttempt(current, "pending") : current);
    try {
      const response = await doctorApi.analyzeRuntimeProfile({
        connection_id: session.connection_id,
        candidate_sql: attempt.sqlSnapshot,
        execution_attempt_id: attempt.executionAttemptId,
      }, controller.signal);
      if (!acceptsExecutionFailure(owner, explainRequestGeneration.current)) return;
      // The accepted response is now owned by Doctor evidence; do not leave a
      // stale execution shell behind on return to Queries.
      setExecutionAttempt(null);
      showDoctor({ report: response.outcome.report, origin: response.input_origin, evidence: { kind: "profile_fetch", value: response }, query: response.query });
    } catch (cause) {
      if (acceptsExecutionFailure(owner, explainRequestGeneration.current)) {
        const message = requestFailureMessage(cause, "runtime");
        // A malformed response, abort after dispatch, or ambiguous transport must
        // retain the frozen attempt instead of offering a second POST.
        if (message === EXECUTION_RESPONSE_UNKNOWN) {
          executionUnknownSessionRef.current = owner.connectionId;
          setExecutionAttempt((current) => current?.executionAttemptId === attempt.executionAttemptId ? advanceSqlExecutionAttempt(current, "unknown") : current);
        } else {
          // A remaining typed error is a certified preflight rejection (or
          // expiry): the draft gets its inline error and a later confirmation
          // must create a fresh attempt identity.
          setExecutionAttempt(null);
          setExplainError({ owner, value: message, mode: attempt.planMode });
        }
        if (message === "connection_expired" || message === "connection_session_expired") expireConnection();
      }
    } finally {
      if (runtimeControllerRef.current === controller) runtimeControllerRef.current = null;
    }
  }, [connection.phase, connectionSession, expireConnection]);

  const requestExecutionConfirmation = useCallback(() => {
    const session = connectionSession;
    if (!session || connection.phase !== "ready" || executionResponseUnknownLocksSession(executionUnknownSessionRef.current, session.connection_id)) return;
    const attempt = createSqlExecutionAttempt(sqlDraft, crypto.randomUUID());
    if (!attempt || attempt.connectionId !== session.connection_id) return;
    setExecutionAttempt(attempt);
  }, [connection.phase, connectionSession, sqlDraft]);
  const stopWaitingForExecution = useCallback(() => {
    const attempt = executionAttempt;
    if (!attempt || attempt.phase !== "pending" || attempt.connectionId !== connectionSession?.connection_id) return;
    // Stop only ends browser waiting. Incrementing generation prevents a late
    // response from replacing the unknown result state.
    explainRequestGeneration.current += 1;
    runtimeControllerRef.current?.abort();
    runtimeControllerRef.current = null;
    executionUnknownSessionRef.current = attempt.connectionId;
    setExecutionAttempt(advanceSqlExecutionAttempt(attempt, "unknown"));
  }, [connectionSession?.connection_id, executionAttempt]);
  const returnExecutionToDraft = useCallback(() => {
    if (executionAttempt?.phase === "unknown") return;
    setExecutionAttempt(null);
  }, [executionAttempt?.phase]);

  const acceptReport = (nextReport: AnalysisSnapshotReport) => showDoctor({ report: nextReport, origin: "offline_import", evidence: null, query: null });
  const acceptConnectedReport = (profile: ProfileFetchResponse) => {
    if (!profileFetchBelongsToCurrentSession(profile, connectionSession)) return;
    showDoctor({ report: profile.outcome.report, origin: profile.input_origin, evidence: { kind: "profile_fetch", value: profile }, query: doctorQueryForAcceptedProfile(profile, connectionSession) });
  };
  const acceptRunningReport = (snapshot: RunningProfileSnapshotResponse) => {
    const evidence = admitRunningDoctorEvidence(snapshot, connectionSession);
    // Retained Running evidence is never ordinary fetched authority.
    if (evidence) showDoctor({ report: snapshot.outcome.report, origin: snapshot.input_origin, evidence, query: null });
  };
  const acceptConnectedComparison = (nextComparison: ConnectedProfileComparisonResponse) => {
    setComparison(nextComparison.comparison);
    setConnectedComparison(nextComparison);
    setWorkspace("Compare");
  };
  // Swapping a connected comparison asks the backend again with the two executions exchanged, in the same discovery generation.
  const swapConnectedComparison = () => {
    const current = connectedComparison;
    const generation = current?.baseline.query.discovery_generation ?? null;
    if (!current || !connectionSession || generation === null || current.baseline.query.connection_id !== connectionSession.connection_id) return;
    setSwapError(null);
    doctorApi.compareConnectedProfiles({ connection_id: connectionSession.connection_id, baseline_query_id: current.candidate.query.query_id, candidate_query_id: current.baseline.query.query_id, discovery_generation: generation })
      .then(acceptConnectedComparison)
      .catch(() => setSwapError(text("The swapped comparison could not be read; refresh the query list and compare again.", "交换后的对比读取失败；请刷新查询列表后重新对比。")));
  };
  const setBaselineFromProfile = (profile: ProfileFetchResponse) => {
    const query = doctorQueryForAcceptedProfile(profile, connectionSession);
    if (!query) return;
    setCompareBaseline({ connectionId: query.connection_id, query, report: profile.outcome.report });
  };
  // Doctor shows a Profile pulled in this session: it can become the baseline directly.
  const doctorBaseline = doctorEvidence?.kind === "profile_fetch" && currentDoctorQuery && doctorReport
    ? { connectionId: currentDoctorQuery.connection_id, query: currentDoctorQuery, report: doctorReport }
    : null;
  const openImport = () => { setImporting(true); setWorkspace("Doctor"); };
  const closeConnectionDialog = useCallback(() => {
    setDialogOpen(false);
    window.requestAnimationFrame(() => connectionTriggerRef.current?.focus());
  }, []);
  const queryModes: { id: QueryMode; label: string; available: boolean }[] = [
    { id: "audit", label: text("Audit history", "审计历史"), available: auditReady },
    { id: "profiles", label: text("Profile list", "Profile 列表"), available: true },
    { id: "running", label: text("Running now", "运行中"), available: true },
    { id: "sql", label: text("SQL workspace", "SQL 工作区"), available: true },
  ];

  // The first-run page carries its own import card, so the header drops its import button (design board 05).
  const firstRun = workspace === "Queries" && !connectionSession;
  return (
    <main className="app-shell">
      <header className="app-header">
        <div className="product-lockup">
          <svg aria-hidden="true" fill="none" height="22" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" viewBox="0 0 24 24" width="22"><path d="M3 17l5-6 4 3 5-7" /><circle cx="19.5" cy="5.5" r="1.8" /><path d="M3 21h18" /></svg>
          <strong>SR Doctor</strong><span className="header-detail">{text("Profile diagnosis", "Profile 诊断")}</span>
        </div>
        {/* A numbered flow; step numbers and the count badge are CSS-generated so each button's text stays its name. */}
        <nav className="workspace-nav" aria-label={text("Workspaces", "工作区")}>
          {workspaces.map((item, index) => <Fragment key={item}>
            {index > 0 ? <svg aria-hidden="true" className="workspace-nav-sep" fill="none" height="14" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M9 6l6 6-6 6" /></svg> : null}
            <button aria-current={workspace === item && !firstRun ? "page" : undefined} data-count={item === "Recommendations" && doctorReport ? recommendedFindings(doctorReport).length : undefined} data-step={index + 1} disabled={firstRun && !doctorReport && (item === "Doctor" || item === "Recommendations")} onClick={() => { if (item === "Doctor") setImporting(false); setWorkspace(item); }} type="button">{workspaceLabel(item, text)}</button>
          </Fragment>)}
          <span aria-hidden="true" className="workspace-nav-divider" />
          {monitoringWorkspaces.map((item) => <button aria-current={workspace === item ? "page" : undefined} className="monitoring" data-count={item === "Alerts" && openAlerts > 0 ? openAlerts : item === "Loads" && pausedLoadCount > 0 ? pausedLoadCount : undefined} key={item} onClick={() => { if (item === "Loads") setLoadFocus(null); setWorkspace(item); }} type="button">{workspaceLabel(item, text)}</button>)}
        </nav>
        <div className="header-actions">
          <CollectionCapsule onOpen={() => { setMonitoringTab("collection"); setWorkspace("Trends"); }} status={history.status} />
          <button aria-describedby="connection-pill-status" aria-expanded={dialogOpen} aria-haspopup="dialog" aria-label={connected ? text("Manage connection", "管理连接") : text("Connect", "连接")} className={`connection-pill ${connectionTone(connection)}`} onClick={() => setDialogOpen(true)} ref={connectionTriggerRef} title={localizedSessionStatus(connection, text)} type="button"><span id="connection-pill-status"><HeaderStatusText value={localizedSessionStatus(connection, text)} /></span></button>
          <span className="locale-switch" aria-label={text("Language", "语言")} role="group">
            <button aria-label={text("Switch to Chinese", "切换至中文")} aria-pressed={locale === "zh"} className={locale === "zh" ? "active" : ""} onClick={() => setLocale("zh")} type="button">中</button>
            <button aria-label={text("Switch to English", "切换至英文")} aria-pressed={locale === "en"} className={locale === "en" ? "active" : ""} onClick={() => setLocale("en")} type="button">EN</button>
          </span>
          {firstRun ? null : <button className="button accent header-import" onClick={openImport} title={text("Import profile", "导入 Profile")} type="button"><svg aria-hidden="true" fill="none" height="14" stroke="currentColor" strokeLinecap="round" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M12 4v11M7 10l5 5 5-5M5 20h14" /></svg><span className="header-detail">{text("Import profile", "导入 Profile")}</span></button>}
        </div>
      </header>
      {doctorReport && workspace === "Recommendations" ? <ContextBar compact onOpenDoctor={() => setWorkspace("Doctor")} report={doctorReport} /> : null}
      {doctorReport && workspace === "Doctor" && !importing ? <ContextBar
        actions={<>
          <ReportExportButton label={text("Export report", "导出报告")} request={{ kind: "analysis", snapshot: doctorReport }} />
          {doctorBaseline
            ? <button className="button secondary" onClick={() => { setCompareBaseline(doctorBaseline); setComparison(null); setConnectedComparison(null); setWorkspace("Compare"); }} type="button">{text("Set as baseline and compare", "设为基线并对比")}</button>
            : <button className="button secondary" onClick={() => setWorkspace("Compare")} type="button">{text("Compare before / after", "前后对比")}</button>}
        </>}
        inputOrigin={doctorReportOrigin}
        observedAtUtc={currentDoctorQuery?.observed_at_utc ?? null}
        onReturnToQueries={() => setWorkspace("Queries")}
        report={doctorReport}
      /> : null}
      <section className={`workspace-content workspace-${workspace.toLowerCase()}`}>
        <Suspense fallback={<p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>}>
        {/* Kept mounted: the query list, selection and filters survive a round trip to Doctor. */}
        <div className="queries-shell" hidden={workspace !== "Queries"}>
          {capabilityError && <p className="capability-error">{capabilityError === "local_api_unavailable" ? text("Local API is unavailable.", "本地 API 不可用。") : capabilityError}</p>}
          {/* No session: offer offline import and the read-only connection side by side. */}
          {!connectionSession ? <FirstRunPage connection={connection} dispatch={dispatchConnection} maxInputBytes={maxInputBytes} onReport={acceptReport} onSessionEnded={invalidateConnectedArtifacts} onSessionExpired={expireConnection} /> : <>
          <AuditFallbackBanner session={connectionSession} workspace={workspace} />
          <div className="queries-toolbar">
            <div className="query-modes segmented" role="tablist" aria-label={text("How to pick a query", "选择查询的方式")}>
              {queryModes.filter((mode) => mode.available).map((mode) => <button aria-selected={activeQueryMode === mode.id} className={activeQueryMode === mode.id ? "active" : ""} key={mode.id} onClick={() => switchQueryMode(mode.id)} role="tab" type="button">{mode.label}</button>)}
            </div>
            <QueryCapabilityChips session={connectionSession} />
          </div>
          {/* Its own boundary: loading one view keeps the mode tabs above it in place. */}
          <div ref={queryViewRef}><Suspense fallback={<p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>}>
            {activeQueryMode === "audit" && <AuditWorkloadWorkspace baselineQueryId={compareBaseline?.connectionId === connectionSession?.connection_id ? compareBaseline?.query.query_id ?? null : null} onOpenCompare={acceptConnectedComparison} onOpenPlanCheck={(request) => { switchQueryMode("sql"); void requestExplain(request); }} onSetBaseline={setBaselineFromProfile} session={connectionSession} onConnect={() => setDialogOpen(true)} onDiscoveryChanged={invalidateConnectedArtifacts} onSessionExpired={expireConnection} onOpenDoctor={acceptConnectedReport} onOpenLoads={openLoads} />}
            {activeQueryMode === "profiles" && <QueriesWorkspace onConnect={() => setDialogOpen(true)} onImportProfile={openImport} onDiscoveryChanged={invalidateConnectedArtifacts} onSessionExpired={expireConnection} onOpenPlanCheck={(request) => { switchQueryMode("sql"); void requestExplain(request); }} onOpenCompare={acceptConnectedComparison} onOpenDoctor={acceptConnectedReport} session={connected ? connectionSession : null} />}
            {activeQueryMode === "running" && <RunningProfileEntry session={connectionSession} onDiscoveryChanged={invalidateConnectedArtifacts} onSessionExpired={expireConnection} onOpenDoctor={acceptRunningReport} />}
            {activeQueryMode === "sql" && (connectionSession?.state === "ready" ? <SqlAnalysisWorkspace
              connection={{ connectionId: connectionSession.connection_id, detectedVersion: connectionSession.detected_version?.value ?? null, tlsMode: connectionSession.tls_mode }}
              onBackToDraft={returnExecutionToDraft}
              onConfirmExecution={() => { if (currentExecutionAttempt) void dispatchExecutionAttempt(currentExecutionAttempt); }}
              onPlanModeChange={(planMode) => setSqlDraft((draft) => ({ ...draft, planMode }))}
              onRequestConfirmation={requestExecutionConfirmation}
              onSqlChange={(value) => setSqlDraft((draft) => ({ ...draft, connectionId: connectionSession.connection_id, value }))}
              onStopWaiting={stopWaitingForExecution}
              onViewPlan={(mode) => { if (sqlDraft.value.trim()) void requestExplain({ input_kind: "user_sql", connection_id: connectionSession.connection_id, mode, candidate_sql: sqlDraft.value }); }}
              planError={currentExplainError}
              planMode={currentExecutionAttempt?.planMode ?? sqlDraft.planMode}
              planRequesting={ownsCurrentExplainResult(explaining, currentExplainOwner)}
              planResultMode={currentExplainErrorMode ?? currentExplainSnapshotMode}
              planSnapshot={currentExplainSnapshot}
              sql={currentExecutionAttempt?.sqlSnapshot ?? sqlDraft.value}
              stage={sqlWorkspaceStage}
            /> : <section className="panel"><h2>{text("SQL workspace", "SQL 工作区")}</h2><p className="muted">{text("Connect to a database to view a plan or capture a Profile.", "连接数据库后可查看计划或采集 Profile。")}</p><button className="button secondary" onClick={() => setDialogOpen(true)} type="button">{text("Connect", "连接")}</button></section>)}
          </Suspense></div>
          </>}
        </div>
        {workspace === "Doctor" && <DoctorWorkspace
          focusNodeId={focusNodeId}
          importing={importing}
          officialContext={{
            session: connectionSession,
            currentQuery: currentDoctorQuery,
            evidence: doctorEvidence?.kind === "profile_fetch" ? doctorEvidence.value : null,
            onSessionExpired: expireConnection,
            onReturnToQueries: () => setWorkspace("Queries"),
            onReconnect: () => setDialogOpen(true),
          }}
          onCancelImport={() => setImporting(false)}
          onOpenRecommendations={() => setWorkspace("Recommendations")}
          onReport={acceptReport}
          report={doctorReport}
        />}
        {workspace === "Recommendations" && <RecommendationsPanel
          advisorResponse={currentAdvisorResponse(advisor, advisorSession)}
          onLocateNode={(nodeId) => { if (!doctorReport?.graph_view.nodes.some((node) => node.node_id === nodeId)) return; setFocusNodeId(nodeId); setImporting(false); setWorkspace("Doctor"); }}
          onImportProfile={openImport}
          onOpenCompare={() => setWorkspace("Compare")}
          onOpenQueries={() => setWorkspace("Queries")}
          report={doctorReport}
        />}
        {workspace === "Compare" && <>
          {!comparison && compareBaseline && <PendingComparison baseline={compareBaseline} onClear={() => setCompareBaseline(null)} onPickCandidate={() => { switchQueryMode("audit"); setWorkspace("Queries"); }} />}
          {!comparison && !compareBaseline && <OfflineCompareForm connected={connected} onComparison={(next) => { setComparison(next); setConnectedComparison(null); }} />}
          {swapError && <p className="compare-swap-error" role="alert">{swapError}</p>}
          <CompareWorkspace
            baselineOrigin={comparison ? connectedComparison?.baseline.input_origin ?? "offline_import" : null}
            candidateOrigin={comparison ? connectedComparison?.candidate.input_origin ?? "offline_import" : null}
            comparison={comparison}
            connectedComparison={connectedComparison}
            onReplace={comparison ? () => { setComparison(null); setConnectedComparison(null); setSwapError(null); } : undefined}
            onSwap={connectedComparison && connectedComparison.baseline.query.connection_id === connectionSession?.connection_id ? swapConnectedComparison : undefined}
          />
        </>}
        {workspace === "Trends" && <HistoryWorkspace connectionId={readySessionId} focusTemplate={trendFocus} history={history} onOpenDoctor={acceptConnectedReport} onOpenItem={openGovernanceItem} onOpenQueries={() => setWorkspace("Queries")} onSessionExpired={expireConnection} onTab={setMonitoringTab} onTarget={setPickedTarget} onTargets={setKnownTargets} tab={monitoringTab} target={monitorTarget} />}
        {workspace === "Alerts" && <AlertsWorkspace generation={monitorGeneration(history.status)} onChanged={history.refresh} onOpenItem={openGovernanceItem} onOpenQueries={() => setWorkspace("Queries")} onOpenTrends={openTrends} onRetryStatus={history.refresh} store={history.status?.store} target={monitorTarget} targets={knownTargets} onTarget={setPickedTarget} />}
        {workspace === "Loads" && <LoadWorkspace connectionId={readySessionId} focus={loadFocus} key={readySessionId ?? "none"} onOpenQueries={() => setWorkspace("Queries")} onPaused={(count) => { if (readySessionId) setPausedLoads({ connectionId: readySessionId, count }); }} />}
        {workspace === "Governance" && <GovernanceWorkspace focusId={governanceFocus} onOpenQueries={() => setWorkspace("Queries")} onOpenTrends={openTrends} onRetryStatus={history.refresh} store={history.status?.store} target={monitorTarget} targets={knownTargets} onTarget={setPickedTarget} />}
        </Suspense>
      </section>
      <ConnectionDialog dispatch={dispatchConnection} onClose={closeConnectionDialog} onSessionEnded={invalidateConnectedArtifacts} onSessionExpired={expireConnection} open={dialogOpen} state={connection} />
    </main>
  );
}

export function requestFailureMessage(cause: unknown, kind: ExecutionKind): string {
  if (typeof cause === "object" && cause !== null && "message_id" in cause
    && (cause.message_id === "connection_expired" || cause.message_id === "connection_session_expired")) return cause.message_id;
  // A post-dispatch abort only stops this browser's wait. The remote execution
  // may still have reached StarRocks, so it remains terminally unknown.
  if (kind === "runtime" && cause instanceof DOMException && cause.name === "AbortError") return EXECUTION_RESPONSE_UNKNOWN;
  if (typeof cause === "object" && cause !== null && (
    kind === "runtime" && (("recoverability" in cause && cause.recoverability === "status_unknown")
      || ("recoverability" in cause && cause.recoverability === "restart")
      || ("code" in cause && cause.code === "execution_attempt_already_submitted"))
  )) return EXECUTION_RESPONSE_UNKNOWN;
  if (typeof cause === "object" && cause !== null && "message_id" in cause && typeof cause.message_id === "string") return cause.message_id;
  return cause instanceof Error ? cause.message : "request_failed";
}

export function shouldDiscardReportOnDiscoveryChange(origin: InputOrigin | null): boolean {
  return origin !== null && origin !== "offline_import" && origin !== "connected_sql_runtime";
}

export function canShowAuditWorkload(session: ConnectionSessionView | null): boolean {
  return session?.state === "ready" && session.capabilities.audit === "available";
}

export function AuditFallbackBanner({ session, workspace }: { session: ConnectionSessionView | null; workspace: Workspace }): JSX.Element | null {
  const { text } = useLocale();
  if (workspace !== "Queries" || !session || session.state !== "ready" || session.capabilities.audit === "available") return null;
  return <p className="capability-error" role="status">{text("Audit queries are not available on this connection. You can still select an existing Profile or import one to analyze.", "当前连接暂不支持审计查询。你仍可选择已有 Profile，或导入文件进行分析。")}</p>;
}

function workspaceLabel(workspace: Workspace, text: LocalizedText): string {
  const labels: Record<Workspace, [string, string]> = {
    Queries: ["Queries", "查询"],
    Doctor: ["Doctor", "诊断"],
    Recommendations: ["Recommendations", "优化建议"],
    Compare: ["Compare", "对比"],
    Trends: ["Trends", "趋势"],
    Alerts: ["Alerts", "告警"],
    Governance: ["Governance", "治理"],
    Loads: ["Loads", "导入"],
  };
  return text(...labels[workspace]);
}

function connectionTone(connection: ReturnType<typeof connectionReducer>): string {
  if (connection.phase === "ready" && connection.session) return "connected";
  if (connection.phase === "unsupported" || connection.phase === "expired" || connection.phase === "failed") return "warning";
  return "";
}

function localizedSessionStatus(connection: ReturnType<typeof connectionReducer>, text: LocalizedText): string {
  if ((connection.phase === "ready" || connection.phase === "unsupported") && connection.session) {
    const version = connection.session.detected_version?.value ?? text("version unknown", "版本未知");
    return `${connection.phase === "ready" ? text("Connected", "已连接") : text("Limited", "受限")} · ${version} · ${text("read-only", "只读")}`;
  }
  if (connection.phase === "connecting") return text("Connecting…", "连接中…");
  if (connection.phase === "expired") return connection.message === "remote_timeout"
    ? text("Query timed out · reconnect", "查询超时已断开 · 请重连")
    : text("Session expired · reconnect", "会话已过期 · 请重连");
  if (connection.phase === "failed") return text("Connection failed", "连接失败");
  return text("Not connected", "未连接");
}
