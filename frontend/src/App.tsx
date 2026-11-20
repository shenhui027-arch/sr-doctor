import { lazy, Suspense, useCallback, useEffect, useReducer, useRef, useState } from "react";
import type { JSX } from "react";
import type { AnalysisSnapshotReport, Comparison, ConnectedProfileComparisonResponse, ConnectedQueryRef, ConnectionSessionView, ExplainMode, ExplainRequest, ExplainSnapshot, InputOrigin, PlanAdvisorResponse, ProfileFetchResponse, RunningProfileSnapshotResponse } from "../../contracts/generated/doctor-contracts";
import { ConnectionDialog, connectionFailureMessage, isConnectionExpiredFailure } from "./features/connection/ConnectionDialog";
import { forgetResumableSession, readResumableSession, rememberResumableSession } from "./features/connection/connection-memory";
import { connectionReducer, initialConnectionState, type ConnectionAction } from "./features/connection/connection-state";
import { DoctorWorkspace } from "./features/doctor/DoctorWorkspace";
import { ReportExportButton } from "./features/export/ReportExportButton";
import { Button, Card, Icon, SegTabs } from "./design";
import { ContextBar } from "./features/shell/ContextBar";
import { TopBar, localizedSessionStatus } from "./features/shell/TopBar";
import { FirstRunPage } from "./features/start/FirstRunPage";
import { runningSnapshotMatchesBinding } from "./features/execution/running-official-binding";
import { OfficialAdvisorGuides } from "./features/recommendations/RecommendationsPanel";
import { QueriesWorkspace } from "./features/queries/QueriesWorkspace";
import { QueryCapabilityChips } from "./features/queries/QueryCapabilityChips";
import { SqlAnalysisWorkspace, type SqlAnalysisWorkspaceStage } from "./features/plan/SqlAnalysisWorkspace";
import { advanceSqlExecutionAttempt, attemptForConnection, canDispatchSqlExecution, createSqlExecutionAttempt, emptySqlWorkspaceDraft, type SqlExecutionAttempt, type SqlWorkspaceDraft } from "./app-sql-workspace-state";
import { formatAppRouteHash, parseAppRouteHash } from "./app-route-hash";
import { useLocale } from "./i18n/locale";
import { CONNECTION_EXPIRED_EVENT, doctorApi } from "./lib/api";
import { monitorGeneration, useHistoryStatus } from "./features/history/history-status";
import type { LoadFocus } from "./features/loads/LoadWorkspace";
import { AppMotion, useEntrance } from "./features/shell/motion";
import { SettingsWorkspace, type SettingsSection } from "./features/settings/SettingsWorkspace";
import { AlertsSummaryCard } from "./features/overview/AlertsSummaryCard";

// Connected audit UI is not needed for offline Profile diagnosis.
const AuditWorkloadWorkspace = lazy(() => import("./features/audit/AuditWorkloadWorkspace").then((module) => ({ default: module.AuditWorkloadWorkspace })));
// Compare and the monitoring pages are opened on demand; they stay out of the first load.
const CompareWorkspace = lazy(() => import("./features/compare/CompareWorkspace").then((module) => ({ default: module.CompareWorkspace })));
const PendingComparison = lazy(() => import("./features/compare/CompareWorkspace").then((module) => ({ default: module.PendingComparison })));
const OfflineCompareForm = lazy(() => import("./features/compare/OfflineCompareForm").then((module) => ({ default: module.OfflineCompareForm })));
const TrendsWorkspace = lazy(() => import("./features/history/HistoryWorkspace").then((module) => ({ default: module.TrendsWorkspace })));
const AlertsWorkspace = lazy(() => import("./features/history/MonitoringWorkspaces").then((module) => ({ default: module.AlertsWorkspace })));
const GovernanceWorkspace = lazy(() => import("./features/history/MonitoringWorkspaces").then((module) => ({ default: module.GovernanceWorkspace })));
const LoadWorkspace = lazy(() => import("./features/loads/LoadWorkspace").then((module) => ({ default: module.LoadWorkspace })));
const RunningProfileEntry = lazy(() => import("./features/queries/RunningProfileEntry").then((module) => ({ default: module.RunningProfileEntry })));

/** Task 63 (approved boards): five peer pages; Settings opens from the gear. */
const workspaces = ["Overview", "Queries", "Doctor", "Alerts", "Loads"] as const;
export type Workspace = (typeof workspaces)[number] | "Settings";
/** Doctor holds the analysis (each finding with its recommendation, task 65) and the before/after comparison. */
export type DoctorMode = "diagnosis" | "compare";
/** Alerts holds the alert list and the governance queue that follows from it. */
export type AlertsView = "alerts" | "governance";
/** One switch for every way to pick a query; nothing else lives in the header. */
export type QueryMode = "audit" | "profiles" | "running" | "sql";

export type ExplainOwner = { connectionId: string; generation: number };
type OwnedExplain<T> = { owner: ExplainOwner; value: T; mode: ExplainMode; kind?: ExecutionKind };
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
  const initialRoute = typeof window !== "undefined" ? parseAppRouteHash(window.location.hash) : null;
  const [workspace, setWorkspace] = useState<Workspace>(initialRoute?.workspace ?? "Queries");
  const [doctorMode, setDoctorMode] = useState<DoctorMode>(initialRoute?.doctorMode ?? "diagnosis");
  const [alertsView, setAlertsView] = useState<AlertsView>(initialRoute?.alertsView ?? "alerts");
  /** Overview: the audit performance dashboard, or the 30-day template trends from the history store. */
  const [overviewTab, setOverviewTab] = useState<"performance" | "trends">(initialRoute?.overviewTab ?? "performance");
  const openDoctor = (mode: DoctorMode) => { setDoctorMode(mode); setWorkspace("Doctor"); };
  const contentRef = useRef<HTMLElement>(null);
  const swapControllerRef = useRef<AbortController | null>(null);
  useEntrance(contentRef, workspace === "Doctor" ? `Doctor:${doctorMode}` : workspace === "Alerts" ? `Alerts:${alertsView}` : workspace);
  const [focusNodeId, setFocusNodeId] = useState<string | null>(null);
  const [queryMode, setQueryMode] = useState<QueryMode | null>(initialRoute?.queryMode ?? null);
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
  // Task 71: a reload keeps the connection. The tab remembers only the session's opaque ID as it is
  // hidden and re-attaches on load; the password stays in the server session, which still ends on
  // disconnect or after its idle time. The ID is cleared while the page is live, so a duplicated
  // tab never shares this tab's session.
  const liveSessionId = connectionSession?.connection_id ?? null;
  useEffect(() => {
    if (!liveSessionId) return;
    const remember = () => rememberResumableSession(liveSessionId);
    window.addEventListener("pagehide", remember);
    window.addEventListener("pageshow", forgetResumableSession);
    return () => {
      window.removeEventListener("pagehide", remember);
      window.removeEventListener("pageshow", forgetResumableSession);
    };
  }, [liveSessionId]);
  // Task 72 review: every automatic connect (re-attach, connect on open, expiry recovery) belongs to
  // the connection operation current when it started. A manual connect or disconnect starts a new
  // one, so a late automatic result is dropped, and a session it brought is closed, not left open.
  const connectionOpRef = useRef(0);
  const dispatchUserConnection = useCallback((action: ConnectionAction) => {
    connectionOpRef.current += 1;
    dispatchConnection(action);
  }, []);
  const autoDispatch = useCallback((op: number, action: ConnectionAction) => {
    if (connectionOpRef.current === op) dispatchConnection(action);
  }, []);
  const autoAccept = useCallback((op: number, session: ConnectionSessionView) => {
    if (connectionOpRef.current === op) { dispatchConnection({ type: "session", session }); return; }
    void doctorApi.closeConnection(session.connection_id).catch(() => undefined);
  }, []);
  const [resumeId] = useState(readResumableSession);
  useEffect(() => {
    forgetResumableSession();
    const controller = new AbortController();
    const op = connectionOpRef.current;
    const failed = (error: unknown) => {
      if (controller.signal.aborted) return;
      if (isConnectionExpiredFailure(error)) autoDispatch(op, { type: "expire", message: "connection_expired" });
      else autoDispatch(op, { type: "fail", message: connectionFailureMessage(error, "Connection failed.") });
    };
    // A late success always goes through autoAccept: after this effect is cleaned up (unmount) it closes the session.
    const accept = (session: ConnectionSessionView) => autoAccept(op, session);
    // Task 72 (design C1): with a saved connection the page connects on open. Reading whether one
    // is saved fails quietly; only a failed connect is shown. `otherwise` runs when none is saved.
    const connectSaved = (otherwise: () => void) => doctorApi.savedConnection(controller.signal).then((saved) => {
      if (controller.signal.aborted || connectionOpRef.current !== op) return;
      if (saved.state !== "saved") { otherwise(); return; }
      autoDispatch(op, { type: "connect" });
      // Not aborted on unmount: this request creates a session, so its late answer must arrive
      // to be closed by autoAccept (task 72 review).
      doctorApi.connectSaved().then(accept).catch(failed);
    }).catch(() => { if (!controller.signal.aborted) otherwise(); });
    if (resumeId) {
      autoDispatch(op, { type: "connect" });
      // A session the server no longer has (restart, idle expiry) falls back to the saved connection.
      doctorApi.resumeConnection(resumeId, controller.signal).then(accept).catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (isConnectionExpiredFailure(error)) void connectSaved(() => failed(error));
        else failed(error);
      });
    } else {
      void connectSaved(() => undefined);
    }
    return () => {
      controller.abort();
      // Unmounting supersedes every automatic connect still in flight (task 72 review).
      connectionOpRef.current += 1;
    };
  }, [resumeId, autoAccept, autoDispatch]);
  const currentExplainOwner = connection.phase === "ready" && connectionSession ? { connectionId: connectionSession.connection_id, generation: explainRequestGeneration.current } : null;
  const ownsSnapshot = ownsCurrentExplainResult(explainSnapshot?.owner ?? null, currentExplainOwner);
  const ownsError = ownsCurrentExplainResult(explainError?.owner ?? null, currentExplainOwner);
  const currentExplainSnapshot = ownsSnapshot ? explainSnapshot?.value ?? null : null;
  const currentExplainSnapshotMode = ownsSnapshot ? explainSnapshot?.mode ?? null : null;
  const currentExplainError = executionResponseUnknownLocksSession(executionUnknownSessionRef.current, connectionSession?.connection_id ?? null)
    ? EXECUTION_RESPONSE_UNKNOWN
    : ownsError ? explainError?.value ?? null : null;
  const currentExplainErrorMode = ownsError ? explainError?.mode ?? null : null;
  // Task 68: a failed "execute once" is reported with the execution, not as a plan failure.
  const currentExecutionError = ownsError && explainError?.kind === "runtime" ? explainError.value : null;
  const currentExecutionAttempt = attemptForConnection(executionAttempt, connectionSession?.connection_id ?? null);
  const sqlWorkspaceStage: SqlAnalysisWorkspaceStage = currentExecutionAttempt === null
    ? "draft"
    : currentExecutionAttempt.phase === "confirming" ? "confirm"
      : currentExecutionAttempt.phase === "unknown" ? "unknown" : "pending";
  const doctorReport = doctorEvidence?.value.outcome.report ?? report;
  const doctorReportOrigin = doctorEvidence?.value.input_origin ?? reportOrigin;
  const history = useHistoryStatus(readySessionId);
  const [trendFocus, setTrendFocus] = useState<string | null>(null);
  const [governanceFocus, setGovernanceFocus] = useState<string | null>(null);
  // The cluster the monitoring views show: the user's pick, else the one this
  // session is connected to (the status poll's target). A new session resets it.
  const [pickedTarget, setPickedTarget] = useState<string | null>(null);
  useEffect(() => { setPickedTarget(null); }, [readySessionId]);
  const monitorTarget = pickedTarget ?? history.status?.target ?? null;
  const [knownTargets, setKnownTargets] = useState<string[]>([]);
  // Review: "collection and history" entries open that section, not the default one.
  const [settingsSection, setSettingsSection] = useState<SettingsSection>(initialRoute?.settingsSection ?? "notify");
  const routeSyncMountedRef = useRef(false);
  const popRouteRef = useRef(false);
  useEffect(() => {
    const applyHashRoute = () => {
      const parsed = parseAppRouteHash(window.location.hash);
      if (!parsed) return;
      popRouteRef.current = true;
      setWorkspace(parsed.workspace);
      if (parsed.doctorMode !== undefined) {
        if (parsed.doctorMode === "diagnosis") setImporting(false);
        setDoctorMode(parsed.doctorMode);
      }
      if (parsed.alertsView !== undefined) setAlertsView(parsed.alertsView);
      if (parsed.overviewTab !== undefined) setOverviewTab(parsed.overviewTab);
      if (parsed.queryMode !== undefined) setQueryMode(parsed.queryMode);
      if (parsed.settingsSection !== undefined) setSettingsSection(parsed.settingsSection);
    };
    window.addEventListener("popstate", applyHashRoute);
    window.addEventListener("hashchange", applyHashRoute);
    return () => {
      window.removeEventListener("popstate", applyHashRoute);
      window.removeEventListener("hashchange", applyHashRoute);
    };
  }, []);
  useEffect(() => {
    if (!routeSyncMountedRef.current) {
      routeSyncMountedRef.current = true;
      return;
    }
    if (popRouteRef.current) {
      popRouteRef.current = false;
      return;
    }
    const nextHash = formatAppRouteHash({ workspace, overviewTab, queryMode, doctorMode, alertsView, settingsSection });
    if (window.location.hash !== nextHash) {
      window.history.pushState(null, "", `${window.location.pathname}${window.location.search}${nextHash}`);
    }
  }, [workspace, overviewTab, queryMode, doctorMode, alertsView, settingsSection]);
  const openSettings = (section: SettingsSection) => { setSettingsSection(section); setWorkspace("Settings"); };
  const openTrends = (templateKey: string, target: string) => { if (target) setPickedTarget(target); setTrendFocus(templateKey); setOverviewTab("trends"); setWorkspace("Overview"); };
  const openGovernanceItem = (itemId: string) => { setGovernanceFocus(itemId); setAlertsView("governance"); setWorkspace("Alerts"); };
  const openAlerts = Number(history.status?.open_alerts ?? "0");
  // Paused Routine Load jobs as the load workspace last read them; a new session starts unknown.
  const [pausedLoads, setPausedLoads] = useState<{ connectionId: string; count: number } | null>(null);
  const [loadFocus, setLoadFocus] = useState<LoadFocus | null>(null);
  const openLoads = (focus: LoadFocus) => { setLoadFocus(focus); setWorkspace("Loads"); };
  // Task 59: a load alert opens on the load category of the alerts workspace.
  const [alertCategory, setAlertCategory] = useState<"sql" | "load">("sql");
  // From a cluster's load history: the alerts open on that cluster.
  const openLoadAlerts = (target: string | null) => { if (target) setPickedTarget(target); setAlertCategory("load"); setAlertsView("alerts"); setWorkspace("Alerts"); };
  const pausedLoadCount = pausedLoads && pausedLoads.connectionId === readySessionId ? pausedLoads.count : 0;
  const connected = (connection.phase === "ready" || connection.phase === "unsupported") && connectionSession !== null;
  const auditReady = canShowAuditWorkload(connectionSession);
  const activeQueryMode: QueryMode = queryMode ?? (auditReady ? "audit" : "profiles");
  // Task 66: the Overview page shows the query page's own audit instance on its overview surface,
  // so the session keeps one list read (one discovery generation) for both pages.
  const overviewAudit = workspace === "Overview" && overviewTab === "performance" && auditReady && connectionSession !== null;

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

  // Task 72: a session the server no longer has. With a saved connection the page reconnects at
  // once (one attempt however many pages noticed); otherwise it shows the session as expired.
  const reconnectingRef = useRef(false);
  const expireConnection = useCallback((reason?: "remote_timeout") => {
    invalidateConnectedArtifacts(true);
    dispatchConnection({ type: "expire", message: reason ?? "connection_expired" });
    if (reconnectingRef.current) return;
    reconnectingRef.current = true;
    const op = connectionOpRef.current;
    doctorApi.savedConnection()
      .then((saved) => {
        if (saved.state !== "saved" || connectionOpRef.current !== op) return;
        autoDispatch(op, { type: "connect" });
        return doctorApi.connectSaved()
          .then((session) => autoAccept(op, session))
          .catch((error: unknown) => autoDispatch(op, { type: "fail", message: connectionFailureMessage(error, "Connection failed.") }));
      })
      .catch(() => undefined)
      .finally(() => { reconnectingRef.current = false; });
  }, [invalidateConnectedArtifacts, autoAccept, autoDispatch]);

  // Task 73: collector reads never extend a session's idle lease, so an open page keeps its own
  // session alive (every 5 minutes, and when the tab is shown again); a closed tab's session expires.
  // A request on any page that finds the session gone is checked here once, so the header and every
  // page agree.
  const checkingRef = useRef(false);
  useEffect(() => {
    if (!liveSessionId) return;
    let live = true;
    const check = () => {
      if (checkingRef.current) return;
      checkingRef.current = true;
      doctorApi.resumeConnection(liveSessionId)
        .catch((error: unknown) => { if (live && isConnectionExpiredFailure(error)) expireConnection(); })
        .finally(() => { checkingRef.current = false; });
    };
    const visible = () => { if (document.visibilityState === "visible") check(); };
    window.addEventListener(CONNECTION_EXPIRED_EVENT, check);
    document.addEventListener("visibilitychange", visible);
    const timer = window.setInterval(check, 5 * 60_000);
    return () => {
      live = false;
      window.removeEventListener(CONNECTION_EXPIRED_EVENT, check);
      document.removeEventListener("visibilitychange", visible);
      window.clearInterval(timer);
    };
  }, [liveSessionId, expireConnection]);

  // One read-only guide read per ready session, only once Doctor shows an analysis (the guide sits under its findings).
  // Without a report the page keeps its start hint (open queries / import) instead of an Advisor-only view.
  const advisorSession = connection.phase === "ready" && doctorReport ? connectionSession : null;
  const advisorTarget = advisorReadTarget(advisorSession);
  const recommendationsShown = workspace === "Doctor" && doctorMode === "diagnosis" && !importing;
  const advisorGuide = currentAdvisorResponse(advisor, advisorSession);
  useEffect(() => {
    if (!recommendationsShown || !advisorTarget || advisorRequestedRef.current === advisorTarget) return;
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
  }, [advisorTarget, expireConnection, recommendationsShown]);

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
    openDoctor("diagnosis");
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
          setExplainError({ owner, value: message, mode: attempt.planMode, kind: "runtime" });
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
    openDoctor("compare");
  };
  // Swapping a connected comparison asks the backend again with the two executions exchanged, in the same discovery generation.
  const swapConnectedComparison = () => {
    const current = connectedComparison;
    const generation = current?.baseline.query.discovery_generation ?? null;
    if (!current || !connectionSession || generation === null || current.baseline.query.connection_id !== connectionSession.connection_id) return;
    // A late swap response must never overwrite a newer comparison.
    swapControllerRef.current?.abort();
    const controller = new AbortController();
    swapControllerRef.current = controller;
    const isCurrent = () => swapControllerRef.current === controller && !controller.signal.aborted;
    setSwapError(null);
    doctorApi.compareConnectedProfiles({ connection_id: connectionSession.connection_id, baseline_query_id: current.candidate.query.query_id, candidate_query_id: current.baseline.query.query_id, discovery_generation: generation }, controller.signal)
      .then((next) => { if (isCurrent()) acceptConnectedComparison(next); })
      .catch(() => { if (isCurrent()) setSwapError(text("The swapped comparison could not be read; refresh the query list and compare again.", "交换后的对比读取失败；请刷新查询列表后重新对比。")); })
      .finally(() => { if (swapControllerRef.current === controller) swapControllerRef.current = null; });
  };
  const replaceComparison = () => {
    swapControllerRef.current?.abort();
    setComparison(null);
    setConnectedComparison(null);
    setSwapError(null);
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
  const openImport = () => { setImporting(true); openDoctor("diagnosis"); };
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
  const canSwapComparison = Boolean(connectedComparison && connectedComparison.baseline.query.connection_id === connectionSession?.connection_id);
  const alertsViewSwitch = (
    <SegTabs<AlertsView>
      aria-label={text("Alert views", "告警视图")}
      className="alerts-views"
      items={[
        { value: "alerts", label: text("Alerts", "告警") },
        { value: "governance", label: text("Governance queue", "治理队列") },
      ]}
      onValueChange={(next) => { setGovernanceFocus(null); setAlertsView(next); }}
      value={alertsView}
    />
  );
  const doctorContextActions = doctorMode === "diagnosis" ? (
    <>
      {doctorBaseline ? (
        <Button
          onClick={() => {
            setCompareBaseline(doctorBaseline);
            setComparison(null);
            setConnectedComparison(null);
            setDoctorMode("compare");
          }}
          size="sm"
          variant="secondary"
        >
          {text("Set as baseline and compare", "设为基线并对比")}
        </Button>
      ) : null}
      {doctorReport && !importing ? (
        <ReportExportButton label={text("Export report", "导出报告")} request={{ kind: "analysis", snapshot: doctorReport }} />
      ) : null}
    </>
  ) : (
    <>
      {canSwapComparison ? (
        <Button
          onClick={swapConnectedComparison}
          size="sm"
          title={text("Swap baseline and candidate Profiles", "交换基线与候选 Profile 并重新比较")}
          variant="secondary"
        >
          ⇄ <span className="hide-lt-1440">{text("Swap baseline / candidate", "交换基线/候选")}</span>
        </Button>
      ) : null}
      {comparison ? (
        <Button onClick={replaceComparison} size="sm" variant="secondary">{text("Replace Profiles", "替换 Profile")}</Button>
      ) : null}
      {comparison ? (
        <ReportExportButton label={text("Export comparison", "导出对比报告")} request={{ kind: "comparison", snapshot: comparison }} />
      ) : null}
    </>
  );
  return (
    <AppMotion>
    <main className="app-shell">
      <TopBar
        connected={connected}
        connection={connection}
        connectionTriggerRef={connectionTriggerRef}
        dialogOpen={dialogOpen}
        firstRun={firstRun}
        hasDoctorReport={Boolean(doctorReport)}
        historyStatus={history.status}
        onImport={openImport}
        onOpenCollection={() => openSettings("history")}
        onOpenConnection={() => setDialogOpen(true)}
        onOpenSettings={() => openSettings("notify")}
        onSelectWorkspace={(item) => {
          if (item === "Doctor") setImporting(false);
          if (item === "Loads") setLoadFocus(null);
          setWorkspace(item);
        }}
        openAlerts={openAlerts}
        pausedLoadCount={pausedLoadCount}
        workspace={workspace}
      />
      {workspace === "Doctor" ? (
        <ContextBar
          actions={doctorContextActions}
          compareBaselineQueryId={compareBaseline?.query.query_id ?? null}
          comparison={comparison}
          connectedComparison={connectedComparison}
          importing={importing}
          inputOrigin={doctorReportOrigin}
          mode={doctorMode}
          observedAtUtc={currentDoctorQuery?.observed_at_utc ?? null}
          onModeChange={(nextMode) => {
            if (nextMode === "diagnosis") setImporting(false);
            setDoctorMode(nextMode);
          }}
          onReturnToQueries={() => setWorkspace("Queries")}
          report={doctorReport}
        />
      ) : null}
      <section className={`workspace-content workspace-${workspace === "Doctor" && doctorMode !== "diagnosis" ? doctorMode : workspace.toLowerCase()}`} ref={contentRef}>
        {workspace === "Overview" && <div className="workspace-bar overview-bar">
          <SegTabs<"performance" | "trends">
            aria-label={text("Overview view", "总览视图")}
            items={[
              { value: "performance", label: text("Performance overview", "性能概览") },
              { value: "trends", label: text("30-day template trends", "30 天模板趋势") },
            ]}
            onValueChange={setOverviewTab}
            value={overviewTab}
          />
          {overviewTab === "performance" && !overviewAudit ? <span className="card-subtitle overview-bar-note">{connectionSession ? text("This connection has no audit table; the history store's trends are shown.", "当前连接没有审计表，显示历史库的趋势。") : text("Connect to see the audit performance overview; the history store's trends are shown.", "连接后显示审计性能概览；当前显示历史库的趋势。")}</span> : null}
          {overviewTab === "performance" && !connectionSession ? <div className="workspace-bar-right"><Button onClick={() => setDialogOpen(true)} size="sm" variant="secondary">{text("Connect", "连接")}</Button></div> : null}
        </div>}
        {/* Kept mounted: the query list, selection and filters survive a round trip to Doctor. */}
        <div className="queries-shell" hidden={!(workspace === "Queries" || overviewAudit)}>
          {workspace === "Queries" && capabilityError && <p className="capability-error">{capabilityError === "local_api_unavailable" ? text("Local API is unavailable.", "本地 API 不可用。") : capabilityError}</p>}
          {/* No session: offer offline import and the read-only connection side by side. */}
          {!connectionSession ? <FirstRunPage connection={connection} dispatch={dispatchUserConnection} maxInputBytes={maxInputBytes} onReport={acceptReport} onSessionEnded={invalidateConnectedArtifacts} onSessionExpired={expireConnection} /> : <>
          <AuditFallbackBanner session={connectionSession} workspace={workspace} />
          <div className="workspace-bar queries-toolbar" hidden={workspace !== "Queries"}>
            {/* Task 64: the three lists are one switch; the SQL workspace is the action at the end of the row. */}
            <SegTabs<QueryMode>
              aria-label={text("How to pick a query", "选择查询的方式")}
              className="query-modes"
              items={queryModes.filter((mode) => mode.available && mode.id !== "sql").map((mode) => ({ value: mode.id, label: mode.label }))}
              onValueChange={switchQueryMode}
              value={activeQueryMode}
            />
            <div className="workspace-bar-right">
              <QueryCapabilityChips session={connectionSession} />
              <Button aria-pressed={activeQueryMode === "sql"} className="query-run-sql" onClick={() => switchQueryMode("sql")} size="sm" variant={activeQueryMode === "sql" ? "primary" : "secondary"}><Icon name="play" size={14} />{text("Run SQL", "运行 SQL")}</Button>
            </div>
          </div>
          {/* Its own boundary: loading one view keeps the mode tabs above it in place. */}
          <div className={`queries-view queries-view-${overviewAudit ? "audit" : activeQueryMode}`} ref={queryViewRef}><Suspense fallback={<p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>}>
            {(overviewAudit || activeQueryMode === "audit") && <AuditWorkloadWorkspace extraCard={<AlertsSummaryCard loadPaused={pausedLoads && pausedLoads.connectionId === readySessionId ? pausedLoads.count : null} onOpen={() => { setAlertsView("alerts"); setWorkspace("Alerts"); }} open={history.status?.open_alerts ?? null} />} onHandoff={() => { switchQueryMode("audit"); setWorkspace("Queries"); }} surface={workspace === "Overview" ? "overview" : "queries"} baselineQueryId={compareBaseline?.connectionId === connectionSession?.connection_id ? compareBaseline?.query.query_id ?? null : null} onOpenCompare={acceptConnectedComparison} onOpenPlanCheck={(request) => { switchQueryMode("sql"); void requestExplain(request); }} onSetBaseline={setBaselineFromProfile} session={connectionSession} onConnect={() => setDialogOpen(true)} onDiscoveryChanged={invalidateConnectedArtifacts} onSessionExpired={expireConnection} onOpenDoctor={acceptConnectedReport} onOpenLoads={openLoads} />}
            {!overviewAudit && activeQueryMode === "profiles" && <QueriesWorkspace onConnect={() => setDialogOpen(true)} onImportProfile={openImport} onDiscoveryChanged={invalidateConnectedArtifacts} onSessionExpired={expireConnection} onOpenPlanCheck={(request) => { switchQueryMode("sql"); void requestExplain(request); }} onOpenCompare={acceptConnectedComparison} onOpenDoctor={acceptConnectedReport} session={connected ? connectionSession : null} />}
            {!overviewAudit && activeQueryMode === "running" && <RunningProfileEntry session={connectionSession} onDiscoveryChanged={invalidateConnectedArtifacts} onSessionExpired={expireConnection} onOpenDoctor={acceptRunningReport} />}
            {!overviewAudit && activeQueryMode === "sql" && (connectionSession?.state === "ready" ? <SqlAnalysisWorkspace
              connection={{ connectionId: connectionSession.connection_id, detectedVersion: connectionSession.detected_version?.value ?? null, tlsMode: connectionSession.tls_mode }}
              onBackToDraft={returnExecutionToDraft}
              onConfirmExecution={() => { if (currentExecutionAttempt) void dispatchExecutionAttempt(currentExecutionAttempt); }}
              onPlanModeChange={(planMode) => setSqlDraft((draft) => ({ ...draft, planMode }))}
              onRequestConfirmation={requestExecutionConfirmation}
              onSqlChange={(value) => setSqlDraft((draft) => ({ ...draft, connectionId: connectionSession.connection_id, value }))}
              onStopWaiting={stopWaitingForExecution}
              onViewPlan={(mode) => { if (sqlDraft.value.trim()) void requestExplain({ input_kind: "user_sql", connection_id: connectionSession.connection_id, mode, candidate_sql: sqlDraft.value }); }}
              planError={currentExecutionError === null ? currentExplainError : null}
              executionError={currentExecutionError}
              planMode={currentExecutionAttempt?.planMode ?? sqlDraft.planMode}
              planRequesting={ownsCurrentExplainResult(explaining, currentExplainOwner)}
              planResultMode={currentExplainErrorMode ?? currentExplainSnapshotMode}
              planSnapshot={currentExplainSnapshot}
              sql={currentExecutionAttempt?.sqlSnapshot ?? sqlDraft.value}
              stage={sqlWorkspaceStage}
            /> : <Card footer={<Button onClick={() => setDialogOpen(true)} size="sm" variant="primary">{text("Connect", "连接")}</Button>} title={text("SQL workspace", "SQL 工作区")} variant="detail"><p className="muted">{text("Connect to a database to view a plan or capture a Profile.", "连接数据库后可查看计划或采集 Profile。")}</p></Card>)}
          </Suspense></div>
          </>}
        </div>
        {/* Lazy workspaces suspend here, outside the kept-mounted query shell: hiding that shell would
            re-run its effects, reload the audit list and clear a comparison just opened from it. */}
        <Suspense fallback={<p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>}>
        {workspace === "Doctor" && doctorMode === "diagnosis" && <DoctorWorkspace
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
          onOpenCompare={() => { if (doctorBaseline) { setCompareBaseline(doctorBaseline); setComparison(null); setConnectedComparison(null); } setDoctorMode("compare"); }}
          onReport={acceptReport}
          report={doctorReport}
          findingsFooter={advisorGuide ? <OfficialAdvisorGuides advisorResponse={advisorGuide} /> : null}
        />}
        {workspace === "Doctor" && doctorMode === "compare" && <>
          {!comparison && compareBaseline && <PendingComparison baseline={compareBaseline} onClear={() => setCompareBaseline(null)} onPickCandidate={() => { switchQueryMode("audit"); setWorkspace("Queries"); }} />}
          {!comparison && !compareBaseline && <OfflineCompareForm connected={connected} onComparison={(next) => { setComparison(next); setConnectedComparison(null); }} />}
          {swapError && <p className="compare-swap-error" role="alert">{swapError}</p>}
          <CompareWorkspace
            baselineOrigin={comparison ? connectedComparison?.baseline.input_origin ?? "offline_import" : null}
            candidateOrigin={comparison ? connectedComparison?.candidate.input_origin ?? "offline_import" : null}
            comparison={comparison}
            connectedComparison={connectedComparison}
            onReplace={comparison ? replaceComparison : undefined}
            onSwap={connectedComparison && connectedComparison.baseline.query.connection_id === connectionSession?.connection_id ? swapConnectedComparison : undefined}
          />
        </>}
        {workspace === "Overview" && !overviewAudit && <TrendsWorkspace connectionId={readySessionId} focusTemplate={trendFocus} history={history} onOpenDoctor={acceptConnectedReport} onOpenItem={openGovernanceItem} onOpenQueries={() => setWorkspace("Queries")} onSessionExpired={expireConnection} onTarget={setPickedTarget} onTargets={setKnownTargets} target={monitorTarget} />}
        {workspace === "Settings" && <SettingsWorkspace
          connectionId={readySessionId}
          connectionStatus={localizedSessionStatus(connection, text)}
          history={history}
          locale={locale}
          onLocale={setLocale}
          onOpenConnection={() => setDialogOpen(true)}
          onSection={setSettingsSection}
          section={settingsSection}
          tlsMode={connectionSession?.tls_mode ?? null}
        />}
        {workspace === "Alerts" && alertsView === "alerts" && <AlertsWorkspace leading={alertsViewSwitch} generation={monitorGeneration(history.status)} initialCategory={alertCategory} key={alertCategory} onChanged={history.refresh} onOpenItem={openGovernanceItem} onOpenLoadHistory={(focus) => openLoads({ view: "history", ...focus })} onOpenQueries={() => setWorkspace("Queries")} onOpenTrends={openTrends} onRetryStatus={history.refresh} store={history.status?.store} target={monitorTarget} targets={knownTargets} onTarget={setPickedTarget} />}
        {workspace === "Loads" && <LoadWorkspace connectionId={readySessionId} focus={loadFocus} key={readySessionId ?? "none"} onConnect={() => setDialogOpen(true)} onSessionExpired={expireConnection} onOpenAlerts={openLoadAlerts} onOpenQueries={() => setWorkspace("Queries")} onPaused={(count) => { if (readySessionId) setPausedLoads({ connectionId: readySessionId, count }); }} />}
        {workspace === "Alerts" && alertsView === "governance" && <GovernanceWorkspace leading={alertsViewSwitch} focusId={governanceFocus} onOpenQueries={() => setWorkspace("Queries")} onOpenTrends={openTrends} onRetryStatus={history.refresh} store={history.status?.store} target={monitorTarget} targets={knownTargets} onTarget={setPickedTarget} />}
        </Suspense>
      </section>
      <ConnectionDialog dispatch={dispatchUserConnection} onClose={closeConnectionDialog} onSessionEnded={invalidateConnectedArtifacts} onSessionExpired={expireConnection} open={dialogOpen} state={connection} />
    </main>
    </AppMotion>
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
