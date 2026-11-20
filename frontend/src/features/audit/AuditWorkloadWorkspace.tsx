import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  AnalysisSnapshotReport,
  AuditActiveAtResponse,
  AuditFamilyDetailReadRequest,
  AuditFamilyDetailResponse,
  AuditFamilyPresentation,
  AuditFamilyResponse,
  AuditFingerprintFamilyResponse,
  AuditSimilarSqlResponse,
  SimilarSqlCluster,
  SimilarSqlDimensions,
  AuditFamilySort,
  AuditMetricFact,
  AuditMetricId,
  AuditOccurrencePresentation,
  AuditOccurrenceResponse,
  AuditOccurrenceSort,
  AuditOverviewFamilyTopSort,
  AuditOverviewResponse,
  AuditPerformanceDimensionId,
  AuditScope,
  AuditSqlReadRequest,
  AuditStatementFilterCoverage,
  AuditStatementSample,
  ConnectionCapabilityState,
  ConnectedQueryRef,
  ConnectedQuerySqlResponse,
  ConnectedProfileComparisonResponse,
  ConnectionSessionView,
  ExplainRequest,
  ProfileFetchResponse,
} from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Button, Dialog, InfoTip, SegTabs } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { AuditFamilyDetail } from "./AuditFamilyDetail";
import { AuditMetricValue } from "./AuditMetricValue";
import { AuditBoundedSourceNote, AuditDigestGuidance, auditSegmentAt } from "./AuditOverviewCharts";
import { AuditFamilyList, AuditFingerprintFamilyList, AuditKpiStrip, memberKey, type BaselineState, type FamilyMemberState } from "./AuditOverviewPanels";
import { AuditSimilarAside, AuditSimilarSqlList } from "./AuditSimilarSql";
import { AuditPerformanceOverview } from "./AuditPerformanceOverview";
import { AuditHistoryLoad } from "./AuditHistoryLoad";
import { LoadSummaryCard } from "../loads/LoadSummaryCard";
import type { LoadFocus } from "../loads/LoadWorkspace";
import { compactCountText, countText } from "../../lib/duration-display";
import { compactQueryId } from "../../lib/query-id";
import { SqlText, SqlToggleButton } from "../shell/SqlText";
import { auditClockText, auditDigestUnavailable, auditMillisecondsText, errorProportionText, metricRawText, metricText, positiveCount } from "./audit-display";
export { metricText } from "./audit-display";
import "./audit-workload.css";

type AuditApi = Pick<DoctorApi, "compareConnectedProfiles" | "fetchConnectedProfile" | "readAuditSql" | "readAuditFamilyDetail" | "readAuditOverview" | "readAuditActiveAt" | "readAuditOccurrences"> & Partial<Pick<DoctorApi, "readAuditFingerprintFamilies" | "readAuditSimilarSql" | "readAuditPerformance" | "readHistoryLoad" | "readLoadOverview">>;
/** Unofficial fingerprint groups of the digest-missing executions in the admitted families window. */
type FingerprintState = { response: AuditFingerprintFamilyResponse | null; loading: boolean; error: string | null };
/** The family selected in the repeated-SQL table and its member ranking (right-side drawer). */
type FamilySelection = { family: AuditFamilyPresentation; members: AuditOccurrenceResponse | null; loading: boolean; error: string | null; memberKey: string | null; showAll: boolean; drawerOpen: boolean; restoreFocus: boolean };
type OccurrenceSelection = { key: string | null; drawerOpen: boolean; restoreFocus: boolean };
type AuditMode = "occurrences" | "families" | "family" | "similar";
/** A2: the similar-SQL read, its per-request splits and the selected cluster. */
type SimilarState = { response: AuditSimilarSqlResponse | null; loading: boolean; error: string | null; detached: string[]; refined: string[]; selectedKey: string | null };
type AuditWindowPreset = "1h" | "12h" | "1d" | "3d" | "7d" | "30d";
type SqlState = { binding: AuditSqlReadRequest; result: ConnectedQuerySqlResponse };
type FamilyDetailState = { familyKey: string; parentScope: AuditScope; response: AuditFamilyDetailResponse };
type ConnectionScopedResponse = { connection_id: string };

export type AuditWorkloadWorkspaceProps = {
  api?: AuditApi;
  /** The query set as the comparison baseline in this session, if any. */
  baselineQueryId?: string | null;
  onOpenCompare?(comparison: ConnectedProfileComparisonResponse): void;
  onConnect(): void;
  onDiscoveryChanged?(): void;
  onOpenDoctor?(profile: ProfileFetchResponse): void;
  onOpenPlanCheck?(request: ExplainRequest): void;
  onSetBaseline?(profile: ProfileFetchResponse): void;
  /** Opens the load workspace; with it the overview shows the load card (task 52). */
  onOpenLoads?(focus: LoadFocus): void;
  /** remote_timeout: the backend discarded the session after a timed-out read, not an idle expiry. */
  onSessionExpired?(reason?: "remote_timeout"): void;
  session: ConnectionSessionView | null;
  /** Task 66: the Overview page shows only the performance overview; the query page only the lists. */
  surface?: "queries" | "overview";
  /**
   * Overview surface: a drill-down (a time window, the family ranking) has opened its list in this
   * same instance; the app shows the query page. One instance serves both pages, so a list read
   * never makes the other page's rows stale (the backend keeps one discovery generation per session).
   */
  onHandoff?(): void;
  /** Overview surface: a summary card next to the load card (alerts). */
  extraCard?: ReactNode;
};

const occurrenceSorts: AuditOccurrenceSort[] = ["query_time_desc", "scan_bytes_desc", "scan_rows_desc", "cpu_cost_desc", "memory_cost_desc", "newest_first"];
const familySorts: AuditFamilySort[] = ["cumulative_query_time_desc", "cumulative_scan_bytes_desc", "cumulative_cpu_cost_desc", "execution_count_desc", "error_count_desc", "error_proportion_desc", "max_query_time_desc", "max_memory_cost_desc"];
const limits = [50, 100, 500] as const;
const windowPresets: AuditWindowPreset[] = ["1h", "12h", "1d", "3d", "7d", "30d"];
const windowPresetSeconds: Record<AuditWindowPreset, number> = { "1h": 3600, "12h": 43200, "1d": 86400, "3d": 259200, "7d": 604800, "30d": 2592000 };

export function AuditWorkloadWorkspace({
  api = doctorApi,
  baselineQueryId = null,
  onOpenCompare,
  onConnect,
  onDiscoveryChanged,
  onOpenDoctor,
  onOpenPlanCheck,
  onSessionExpired,
  onSetBaseline,
  onOpenLoads,
  session,
  surface = "queries",
  onHandoff,
  extraCard,
}: AuditWorkloadWorkspaceProps): JSX.Element {
  const { text } = useLocale();
  const [scope, setScope] = useState<AuditScope>(() => defaultAuditScope(new Date(), session?.capabilities.audit_statement_filter === "available"));
  const [windowPreset, setWindowPreset] = useState<AuditWindowPreset>("1h");
  const [occurrenceSort, setOccurrenceSort] = useState<AuditOccurrenceSort>("query_time_desc");
  const [familySort, setFamilySort] = useState<AuditFamilySort>("cumulative_query_time_desc");
  const [familyTopSort, setFamilyTopSort] = useState<AuditOverviewFamilyTopSort>("cumulative_query_time_desc");
  const [limit, setLimit] = useState<(typeof limits)[number]>(100);
  const [mode, setMode] = useState<AuditMode>("families");
  // O1: the performance overview is the default face of the families/occurrences reads.
  const [overviewShown, setOverviewShown] = useState(surface === "overview");
  // O2: the overview reads either the chosen window or the local 30-day history.
  const [overviewRange, setOverviewRange] = useState<"window" | "history">("window");
  const [familySelection, setFamilySelection] = useState<FamilySelection | null>(null);
  const [fetchedQueryIds, setFetchedQueryIds] = useState<ReadonlySet<string>>(() => new Set());
  const [moreFilters, setMoreFilters] = useState(false);
  const [occurrences, setOccurrences] = useState<AuditOccurrenceResponse | null>(null);
  const [families, setFamilies] = useState<AuditFamilyResponse | null>(null);
  const [fingerprints, setFingerprints] = useState<FingerprintState | null>(null);
  const [similar, setSimilar] = useState<SimilarState | null>(null);
  const [similarDimensions, setSimilarDimensions] = useState<SimilarSqlDimensions>({ tables: true, join: true, predicates: true });
  const [overview, setOverview] = useState<AuditOverviewResponse | null>(null);
  const [activeAt, setActiveAt] = useState<AuditActiveAtResponse | null>(null);
  const [activeFamily, setActiveFamily] = useState<AuditFamilyPresentation | null>(null);
  const [familyDetail, setFamilyDetail] = useState<FamilyDetailState | null>(null);
  const [familyParentScope, setFamilyParentScope] = useState<AuditScope | null>(null);
  const [familyDrillScope, setFamilyDrillScope] = useState<AuditScope | null>(null);
  const [occurrenceSelection, setOccurrenceSelection] = useState<OccurrenceSelection>({ key: null, drawerOpen: false, restoreFocus: true });
  const selectedKey = occurrenceSelection.key;
  const [sql, setSql] = useState<SqlState | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  const [profile, setProfile] = useState<ProfileFetchResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [responseAdmitted, setResponseAdmitted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [sqlLoading, setSqlLoading] = useState(false);
  const [profileLoading, setProfileLoading] = useState(false);
  const [profileError, setProfileError] = useState<string | null>(null);
  const generation = useRef(0);
  const controllers = useRef(new Set<AbortController>());
  const controllerConnectionIds = useRef(new WeakMap<AbortController, string | null>());
  const autoLoadedConnection = useRef<string | null>(null);
  const overviewGenerationStaleRef = useRef(false);
  const sessionConnectionId = session?.connection_id ?? null;
  const currentSessionConnectionId = useRef<string | null>(sessionConnectionId);
  // This ref is deliberately updated during render. A prop transition can paint before the
  // reset effect runs, so completion admission must not rely on that effect for identity.
  currentSessionConnectionId.current = sessionConnectionId;

  const ready = session?.state === "ready";
  const auditAvailable = ready && session?.capabilities.audit === "available";
  const userFilterCapability = session?.capabilities.audit_user_filter ?? "unknown";
  const userFilterAvailable = userFilterCapability === "available";
  const statementFilterCapability = session?.capabilities.audit_statement_filter ?? "unknown";
  const statementFilterAvailable = statementFilterCapability === "available";
  const currentOccurrences = responseAdmitted ? auditResponseForConnection(occurrences, sessionConnectionId) : null;
  const currentFamilies = responseAdmitted ? auditResponseForConnection(families, sessionConnectionId) : null;
  const currentFingerprints = currentFamilies && fingerprints && (!fingerprints.response || fingerprints.response.connection_id === sessionConnectionId) ? fingerprints : null;
  const currentOverview = responseAdmitted ? auditResponseForConnection(overview, sessionConnectionId) : null;
  const currentActiveAt = responseAdmitted ? auditResponseForConnection(activeAt, sessionConnectionId) : null;
  const currentFamilyDetail = responseAdmitted && familyDetail && auditResponseForConnection(familyDetail.response, sessionConnectionId)
    ? familyDetail
    : null;
  const discovery = currentActiveAt ?? currentOccurrences;
  const occurrenceRows = useMemo(() => currentFamilyDetail
    ? familyDetailRepresentativeRows(currentFamilyDetail.response)
    : currentActiveAt?.participants.rows.map((row) => row.occurrence) ?? currentOccurrences?.rows, [currentActiveAt, currentFamilyDetail, currentOccurrences]);
  const selected = useMemo(() => {
    return occurrenceRows?.find((row, index) => occurrenceRowKey(row, index) === selectedKey) ?? null;
  }, [occurrenceRows, selectedKey]);

  const invalidateRequests = useCallback(() => {
    generation.current += 1;
    // Navigation/selection invalidates response admission, not the shared session.
    // Aborting an owned Audit read shuts down its MySQL socket and evicts that
    // session. Let the already-issued bounded read drain under its server deadline.
    controllers.current.clear();
  }, []);

  const beginRequest = useCallback(() => {
    invalidateRequests();
    setLoading(false);
    setSqlLoading(false);
    setProfileLoading(false);
    const controller = new AbortController();
    controllers.current.add(controller);
    controllerConnectionIds.current.set(controller, currentSessionConnectionId.current);
    return { controller, requestGeneration: ++generation.current };
  }, [invalidateRequests]);

  const requestIsCurrent = useCallback((requestGeneration: number, controller: AbortController) => (
    isAuditRequestCurrentForConnection(
      generation.current,
      requestGeneration,
      controller.signal,
      controllerConnectionIds.current.get(controller) ?? null,
      currentSessionConnectionId.current,
    )
  ), []);

  const clearTransientEvidence = useCallback(() => {
    setSql(null);
    setSqlError(null);
    setSqlLoading(false);
    setProfile(null);
    setProfileError(null);
    setProfileLoading(false);
  }, []);

  const loadOverview = useCallback(async (nextScope: AuditScope, primary: "occurrences" | "families", topSort = familyTopSort): Promise<AuditOverviewResponse | null> => {
    if (!session || !auditAvailable) return null;
    const { controller, requestGeneration } = beginRequest();
    setLoading(true);
    setError(null);
    setOverview(null);
    setActiveAt(null);
    setFamilyDetail(null);
    setOccurrences(null);
    setFamilies(null);
    setFingerprints(null);
    setResponseAdmitted(false);
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current));
    setFamilySelection(null);
    clearTransientEvidence();
    try {
      const result = await api.readAuditOverview({
        connection_id: session.connection_id,
        request: {
          scope: nextScope,
          primary: primary === "occurrences"
            ? { state: primary, sort: occurrenceSort, limit }
            : { state: primary, sort: familySort, limit: Math.min(limit, 200) },
          family_top_sort: topSort,
        },
      }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return null;
      overviewGenerationStaleRef.current = false;
      onDiscoveryChanged?.();
      setOverview(result);
      setResponseAdmitted(true);
      // Projection only: keep the server's started-in-window population and Top facts intact.
      if (result.primary.state === "occurrences") setOccurrences({ ...result, rows: result.primary.rows, top: result.primary.top });
      else setFamilies({ ...result, rows: result.primary.rows, top: result.primary.top });
      // A second, serial read: only when the admitted window has digest-missing executions.
      if (result.primary.state === "families" && positiveCount(result.coverage.missing_digest_count) && api.readAuditFingerprintFamilies) {
        setFingerprints({ response: null, loading: true, error: null });
        try {
          const response = await api.readAuditFingerprintFamilies({
            connection_id: session.connection_id,
            request: { scope: result.scope, sort: familySort, limit: Math.min(limit, 200) },
          }, controller.signal);
          if (!requestIsCurrent(requestGeneration, controller)) return null;
          setFingerprints({ response, loading: false, error: null });
        } catch (cause) {
          if (!requestIsCurrent(requestGeneration, controller)) return null;
          setFingerprints({ response: null, loading: false, error: auditErrorLabel(cause) });
          if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
        }
      }
      return result;
    } catch (cause) {
      if (!requestIsCurrent(requestGeneration, controller)) return null;
      setError(auditErrorLabel(cause));
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
      return null;
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setLoading(false);
    }
  }, [api, auditAvailable, beginRequest, clearTransientEvidence, familySort, familyTopSort, limit, occurrenceSort, onDiscoveryChanged, onSessionExpired, requestIsCurrent, session]);

  const loadOccurrences = useCallback((nextScope = scope) => loadOverview(nextScope, "occurrences"), [loadOverview, scope]);
  const loadFamilies = useCallback((nextScope = scope) => loadOverview(nextScope, "families"), [loadOverview, scope]);

  useEffect(() => {
    invalidateRequests();
    autoLoadedConnection.current = null;
    overviewGenerationStaleRef.current = false;
    setScope(defaultAuditScope(new Date(), statementFilterAvailable));
    setWindowPreset("1h");
    setMode("families");
    setFamilySelection(null);
    setFetchedQueryIds(new Set());
    setOccurrences(null);
    setFamilies(null);
    setFingerprints(null);
    setSimilar(null);
    setSimilarDimensions({ tables: true, join: true, predicates: true });
    setOverview(null);
    setActiveAt(null);
    setActiveFamily(null);
    setFamilyDetail(null);
    setResponseAdmitted(false);
    setFamilyParentScope(null);
    setFamilyDrillScope(null);
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current));
    setError(null);
    setLoading(false);
    clearTransientEvidence();
  }, [clearTransientEvidence, invalidateRequests, session?.connection_id, session?.state, statementFilterAvailable]);

  useEffect(() => {
    if (userFilterAvailable) return;
    setScope((current) => current.user === null ? current : { ...current, user: null });
  }, [userFilterAvailable]);

  useEffect(() => {
    if (!session || !auditAvailable || autoLoadedConnection.current === session.connection_id) return;
    autoLoadedConnection.current = session.connection_id;
    void loadFamilies(defaultAuditScope(new Date(), statementFilterAvailable));
  }, [auditAvailable, loadFamilies, session, statementFilterAvailable]);

  // Task 66: the Overview page shows this instance's overview; the query page its lists.
  // The view as last rendered; read by the surface effect below, which may run on a render that
  // returned early (no session, no audit capability) before the view is derived.
  const viewRef = useRef<string>("families");
  // Review: the dashboard reads the families/occurrences window, so a query-page sub-view (similar
  // SQL, a family's detail or members, in flight at a time) returns to that window first.
  useEffect(() => {
    setOverviewShown(surface === "overview");
    const shown = viewRef.current;
    if (surface === "overview" && shown !== "families" && shown !== "occurrences") chooseMode(mode === "occurrences" ? "occurrences" : "families");
    // Only a change of surface does this; the view and mode are read as they are then.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [surface]);

  useEffect(() => () => invalidateRequests(), [invalidateRequests]);

  function refresh(): void {
    const nextScope = auditScopeForWindowPreset(scope, windowPreset);
    setScope(nextScope);
    loadForMode(nextScope);
  }

  async function loadSimilar(nextScope: AuditScope, detached: string[] = [], refined: string[] = [], selectedKey: string | null = null): Promise<void> {
    if (!session || !auditAvailable || !api.readAuditSimilarSql) return;
    const { controller, requestGeneration } = beginRequest();
    setLoading(true);
    setError(null);
    setSimilar((current) => ({ response: current?.response ?? null, loading: true, error: null, detached, refined, selectedKey }));
    try {
      const response = await api.readAuditSimilarSql({
        connection_id: session.connection_id,
        request: { scope: nextScope, sort: familySort, limit: Math.min(limit, 200), dimensions: similarDimensions, detached_member_keys: detached, refined_cluster_keys: refined },
      }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
      // Keep the selection only while that cluster still exists in the new answer.
      const kept = selectedKey && response.clusters.some((cluster) => cluster.cluster_key === selectedKey) ? selectedKey : null;
      setSimilar({ response, loading: false, error: null, detached, refined, selectedKey: kept });
    } catch (cause) {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      setSimilar({ response: null, loading: false, error: auditErrorLabel(cause), detached, refined, selectedKey: null });
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setLoading(false);
    }
  }

  function chooseSimilar(): void {
    const requestScope = mode === "family"
      ? familyParentScope ?? clearFamilyDrillScope(scope)
      : currentFamilies?.scope ?? currentOccurrences?.scope ?? scope;
    setFamilyParentScope(null);
    setFamilyDrillScope(null);
    setActiveFamily(null);
    setFamilySelection(null);
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current));
    setMode("similar");
    void loadSimilar(requestScope);
  }

  function loadForMode(nextScope: AuditScope): void {
    if (mode === "similar") {
      void loadSimilar(nextScope);
      return;
    }
    if (mode === "families") {
      void loadFamilies(nextScope);
      return;
    }
    if (mode === "family") {
      if (currentFamilyDetail) returnToParent(nextScope);
      else void loadOccurrences(nextScope);
      return;
    }
    void loadOccurrences(nextScope);
  }

  function chooseMode(next: "occurrences" | "families"): void {
    if (mode === "similar") {
      const requestScope = similar?.response?.scope ?? scope;
      setMode(next);
      if (next === "families") void loadFamilies(requestScope);
      else void loadOccurrences(requestScope);
      return;
    }
    const switchScopes = auditModeSwitchScopes({
      mode,
      draftScope: scope,
      occurrenceScope: currentOccurrences?.scope ?? null,
      familyScope: currentFamilies?.scope ?? null,
      familyParentScope,
      familyDrillScope,
    });
    if (mode === "family") {
      setScope(switchScopes.draftScope);
      setFamilyParentScope(null);
      setFamilyDrillScope(null);
      setActiveFamily(null);
      setMode(next);
      if (next === "families") void loadFamilies(switchScopes.requestScope);
      else void loadOccurrences(switchScopes.requestScope);
      return;
    }
    setActiveFamily(null);
    setMode(next);
    if (next === "families") void loadFamilies(switchScopes.requestScope);
    else void loadOccurrences(switchScopes.requestScope);
  }

  function selectOccurrence(rowKey: string): void {
    const { resetEvidence, selection } = selectOccurrenceRow(occurrenceSelection, rowKey);
    if (resetEvidence) {
      invalidateRequests();
      clearTransientEvidence();
    }
    setOccurrenceSelection(selection);
    setError(null);
  }

  function closeOccurrence(): void {
    invalidateRequests();
    clearTransientEvidence();
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current, "dismiss"));
  }

  /** `members` is the family's own member read when it holds the current discovery. */
  function openFamily(family: AuditFamilyPresentation, members: AuditOccurrenceResponse | null = null): void {
    if (!session || family.membership.state !== "exact_native_digest") return;
    const parent = members ?? currentOverview ?? currentFamilies;
    if (!parent) return;
    const listScope = members ? currentFamilies?.scope ?? clearFamilyDrillScope(members.scope) : parent.scope;
    const request = auditFamilyDetailRequest(session.connection_id, parent.discovery_generation, parent.scope, family);
    if (!request) return;
    const { controller, requestGeneration } = beginRequest();
    setLoading(true);
    setError(null);
    setActiveAt(null);
    setActiveFamily(null);
    setFamilyDetail(null);
    setResponseAdmitted(false);
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current));
    clearTransientEvidence();
    void api.readAuditFamilyDetail(request, controller.signal).then((result) => {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      if (!isAuditFamilyDetailBound(result, request, family)) {
        setError("invalid_audit_family_detail_binding");
        return;
      }
      setScope(result.scope);
      setWindowPreset(auditWindowPresetForScope(result.scope) ?? windowPreset);
      setFamilyParentScope(listScope);
      setFamilyDrillScope(result.scope);
      setActiveFamily(family);
      setFamilyDetail({ familyKey: request.family_key, parentScope: listScope, response: result });
      setFamilySelection(null);
      setResponseAdmitted(true);
      setOccurrences(null);
      setFamilies(null);
      setFingerprints(null);
      setOverview(null);
      setMode("family");
      onDiscoveryChanged?.();
    }).catch((cause: unknown) => {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      setError(auditErrorLabel(cause));
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    }).finally(() => {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setLoading(false);
    });
  }

  function returnToParent(nextScope = scope): void {
    if (familyDetail && !currentFamilyDetail) return;
    const parentScope = familyParentScope ?? currentFamilyDetail?.parentScope;
    const parent = parentScope && familyDrillScope
      ? auditParentScopeAfterFamilyEdits(parentScope, familyDrillScope, nextScope)
      : parentScope ?? clearFamilyDrillScope(nextScope);
    setScope(parent);
    setWindowPreset(auditWindowPresetForScope(parent) ?? windowPreset);
    setActiveFamily(null);
    setFamilyDetail(null);
    setFamilyParentScope(null);
    setFamilyDrillScope(null);
    setMode("families");
    void loadFamilies(parent);
  }

  function browseFamilyMembers(): void {
    if (!currentFamilyDetail || !activeFamily) return;
    const memberBrowse = auditFamilyMemberBrowseState(activeFamily, familyParentScope ?? currentFamilyDetail.parentScope, currentFamilyDetail.response);
    setFamilyDetail(null);
    setActiveFamily(memberBrowse.activeFamily);
    setFamilyParentScope(memberBrowse.parentScope);
    setFamilyDrillScope(memberBrowse.drillScope);
    setMode(memberBrowse.mode);
    void loadOccurrences(memberBrowse.requestScope);
  }

  function selectFamilyRepresentative(occurrence: AuditOccurrencePresentation): void {
    const index = currentFamilyDetail ? familyDetailRepresentativeRows(currentFamilyDetail.response).findIndex((row) => row === occurrence) : -1;
    selectOccurrence(occurrenceRowKey(occurrence, index < 0 ? 0 : index));
  }

  async function selectTime(atMilliseconds: string): Promise<void> {
    if (!session || !currentOverview || loading) return;
    // A family member read replaced the overview discovery; read the overview again first.
    const stale = overviewGenerationStaleRef.current || (familySelection?.members !== null && familySelection?.members !== undefined);
    const base = stale ? await loadOverview(currentOverview.scope, mode === "occurrences" ? "occurrences" : "families") : currentOverview;
    if (!base) return;
    const { controller, requestGeneration } = beginRequest();
    setLoading(true);
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current));
    setError(null);
    clearTransientEvidence();
    try {
      const result = await api.readAuditActiveAt({
        connection_id: session.connection_id,
        discovery_generation: (stale ? null : currentActiveAt?.discovery_generation) ?? base.discovery_generation,
        scope: base.scope,
        at_milliseconds: atMilliseconds,
      }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
      onDiscoveryChanged?.();
      setActiveAt(result);
    } catch (cause) {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      setError(auditErrorLabel(cause));
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setLoading(false);
    }
  }

  async function viewSql(occurrence = selected): Promise<void> {
    if (!session || !occurrence || sqlLoading) return;
    const request = auditSqlRequestForOccurrence(session.connection_id, discovery, currentFamilyDetail, occurrence);
    if (!request) return;
    selectFamilyDetailAction(occurrence);
    await toggleSql(request);
  }

  // A family member's full SQL, bound to the member list's own discovery generation and scope.
  async function viewMemberSql(): Promise<void> {
    const request = session ? familyMemberSqlRequest(session.connection_id, familySelection?.members ?? null, selectedMember) : null;
    if (!request || sqlLoading) return;
    await toggleSql(request);
  }

  async function toggleSql(request: AuditSqlReadRequest): Promise<void> {
    if (visibleSql(sql, request) !== null) {
      invalidateRequests();
      clearTransientEvidence();
      return;
    }
    const { controller, requestGeneration } = beginRequest();
    setSql(null);
    setSqlError(null);
    setSqlLoading(true);
    try {
      const result = await api.readAuditSql(request, controller.signal);
      if (requestIsCurrent(requestGeneration, controller)) setSql({ binding: request, result });
    } catch (cause) {
      if (requestIsCurrent(requestGeneration, controller)) {
        setSqlError(auditErrorLabel(cause));
        if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
      }
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setSqlLoading(false);
    }
  }

  /** `openAfter` turns fetch + open diagnosis into one user action; the read is the same single fetch. */
  async function fetchProfile(occurrence = selected, openAfter = false): Promise<void> {
    if (!session || !occurrence || occurrence.query.profile_fetch.state !== "available" || profileLoading) return;
    const discoveryGeneration = currentFamilyDetail
      ? auditFamilyDetailDiscoveryGeneration(currentFamilyDetail, occurrence, session.connection_id)
      : discovery?.discovery_generation;
    if (discoveryGeneration === null || discoveryGeneration === undefined) { setProfileError(text("Refresh the query list and select this query again.", "查询列表已失效，请刷新列表后重新选择。")); return; }
    selectFamilyDetailAction(occurrence);
    const { controller, requestGeneration } = beginRequest();
    setProfile(null);
    setProfileError(null);
    setProfileLoading(true);
    setError(null);
    try {
      const result = await api.fetchConnectedProfile({
        connection_id: session.connection_id,
        query_id: occurrence.query.query_id,
        discovery_generation: discoveryGeneration,
        kind: "baseline",
      }, controller.signal);
      if (requestIsCurrent(requestGeneration, controller)) {
        setProfile(result);
        if (result.state === "fetched") setFetchedQueryIds((current) => new Set(current).add(result.query.query_id));
        if (openAfter && onOpenDoctor) setOccurrenceSelection((current) => closeOccurrenceDrawer(current, "diagnose", result.state === "fetched"));
        if (openAfter) onOpenDoctor?.(result);
      }
    } catch (cause) {
      if (requestIsCurrent(requestGeneration, controller)) {
        setError(auditErrorLabel(cause));
        setProfileError(profileFetchFailureText(cause, text));
        if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
      }
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setProfileLoading(false);
    }
  }

  function selectFamilyDetailAction(occurrence: AuditOccurrencePresentation): void {
    if (!currentFamilyDetail) return;
    const index = familyDetailRepresentativeRows(currentFamilyDetail.response).findIndex((row) => row === occurrence);
    const nextKey = index >= 0 ? occurrenceRowKey(occurrence, index) : null;
    if (nextKey === null) return;
    const { resetEvidence, selection } = selectOccurrenceRow(occurrenceSelection, nextKey);
    if (resetEvidence) {
      invalidateRequests();
      clearTransientEvidence();
    }
    setOccurrenceSelection(selection);
  }

  /** Selecting a family reads its member ranking (one occurrence read scoped to the exact family). */
  async function selectFamily(family: AuditFamilyPresentation): Promise<void> {
    if (!session || loading || family.membership.state !== "exact_native_digest") return;
    const memberScope = auditScopeForFamily(currentFamilies?.scope ?? scope, family);
    if (!memberScope) return;
    const { controller, requestGeneration } = beginRequest();
    overviewGenerationStaleRef.current = true;
    setOccurrenceSelection((current) => closeOccurrenceDrawer(current));
    setError(null);
    clearTransientEvidence();
    setActiveAt(null);
    setFamilySelection(openFamilyDrawer(family));
    try {
      const result = await api.readAuditOccurrences({ connection_id: session.connection_id, request: { scope: memberScope, sort: "query_time_desc", limit: 50 } }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
      onDiscoveryChanged?.();
      setFamilySelection((current) => current?.family === family ? { ...current, members: result, loading: false, memberKey: result.rows[0] ? memberKey(result.rows[0], 0) : null } : current);
    } catch (cause) {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      setFamilySelection((current) => current?.family === family ? { ...current, loading: false, error: auditErrorLabel(cause) } : current);
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    } finally {
      controllers.current.delete(controller);
    }
  }

  function closeFamily(): void {
    invalidateRequests();
    clearTransientEvidence();
    setFamilySelection((current) => closeFamilyDrawer(current, "dismiss"));
  }

  function selectMember(key: string): void {
    if (key !== familySelection?.memberKey) {
      invalidateRequests();
      clearTransientEvidence();
    }
    setFamilySelection((current) => selectFamilyMember(current, key));
  }

  const selectedMember = familySelection?.members?.rows.find((row, index) => memberKey(row, index) === familySelection.memberKey) ?? null;

  /** Profile fetch for the selected member, bound to the member read that returned it. */
  async function fetchMemberProfile(after: "diagnose" | "baseline"): Promise<void> {
    const members = familySelection?.members;
    if (!session || !members || !selectedMember || selectedMember.query.profile_fetch.state !== "available" || profileLoading) return;
    const { controller, requestGeneration } = beginRequest();
    setProfileError(null);
    setFamilySelection((current) => current ? { ...current, error: null } : current);
    setProfileLoading(true);
    try {
      const result = await api.fetchConnectedProfile({ connection_id: session.connection_id, query_id: selectedMember.query.query_id, discovery_generation: members.discovery_generation, kind: "baseline" }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
      if (result.state === "fetched") setFetchedQueryIds((current) => new Set(current).add(result.query.query_id));
      if (after === "diagnose") {
        if (onOpenDoctor) setFamilySelection((current) => closeFamilyDrawer(current, "diagnose", result.state === "fetched"));
        onOpenDoctor?.(result);
      } else {
        setFamilySelection((current) => closeFamilyDrawer(current, "baseline", result.state === "fetched"));
        onSetBaseline?.(result);
      }
    } catch (cause) {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      setFamilySelection((current) => current ? { ...current, error: profileFetchFailureText(cause, text) } : current);
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setProfileLoading(false);
    }
  }

  /** Compare the baseline with the selected member: both must be in this member read (one discovery generation). */
  async function compareWithBaseline(): Promise<void> {
    const members = familySelection?.members;
    if (!session || !members || !selectedMember || !baselineQueryId || profileLoading) return;
    if (!members.rows.some((row) => row.query.query_id === baselineQueryId) || selectedMember.query.query_id === baselineQueryId) return;
    const { controller, requestGeneration } = beginRequest();
    setFamilySelection((current) => current ? { ...current, error: null } : current);
    setProfileLoading(true);
    try {
      const result = await api.compareConnectedProfiles({ connection_id: session.connection_id, baseline_query_id: baselineQueryId, candidate_query_id: selectedMember.query.query_id, discovery_generation: members.discovery_generation }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
      if (onOpenCompare) setFamilySelection((current) => closeFamilyDrawer(current, "compare"));
      onOpenCompare?.(result);
    } catch (cause) {
      if (!requestIsCurrent(requestGeneration, controller)) return;
      setFamilySelection((current) => current ? { ...current, error: profileFetchFailureText(cause, text) } : current);
      if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    } finally {
      controllers.current.delete(controller);
      if (requestIsCurrent(requestGeneration, controller)) setProfileLoading(false);
    }
  }

  function planCheckMember(): void {
    const members = familySelection?.members;
    if (!session || !members || !selectedMember || selectedMember.query.plan_check.state !== "available") return;
    if (onOpenPlanCheck) setFamilySelection((current) => closeFamilyDrawer(current, "plan_check"));
    onOpenPlanCheck?.({ input_kind: "plan_sql_reference", connection_id: session.connection_id, mode: "costs", query_id: selectedMember.query.query_id, discovery_generation: members.discovery_generation, plan_sql_ref: selectedMember.query.plan_check.plan_sql_ref });
  }

  if (!session) return <AuditUnavailable onConnect={onConnect} text={text} />;
  if (!auditAvailable) return <AuditUnavailable onConnect={onConnect} session={session} text={text} />;

  const response = currentOccurrences ?? currentFamilies;
  const digestUnavailable = auditDigestUnavailable((currentOverview ?? response)?.coverage);
  const sqlRequest = selected ? auditSqlRequestForOccurrence(session.connection_id, discovery, currentFamilyDetail, selected) : null;
  const shownSql = sqlRequest ? visibleSql(sql, sqlRequest) : null;
  const memberSqlRequest = familyMemberSqlRequest(session.connection_id, familySelection?.members ?? null, selectedMember);
  const memberSql = memberSqlRequest ? visibleSql(sql, memberSqlRequest) : null;
  const selectedDiscoveryGeneration = selected && currentFamilyDetail
    ? auditFamilyDetailDiscoveryGeneration(currentFamilyDetail, selected, session.connection_id)
    : discovery?.discovery_generation ?? null;
  const visibleProfile = auditProfileForSelectedOccurrence(selected?.query ?? null, selectedDiscoveryGeneration, profile);
  const doctorProfile = visibleProfile;
  const familyDetailShown = mode === "family" && currentFamilyDetail !== null;
  const membersShown = familySelection?.showAll === true && familySelection.members !== null && !currentActiveAt && !familyDetailShown;
  const coverage = (currentOverview ?? response)?.coverage ?? null;
  const statementFilter = coverage?.statement_filter;
  const view = currentActiveAt ? "active_at" : familyDetailShown ? "family_detail" : membersShown ? "members" : mode === "similar" ? "similar" : mode === "families" ? "families" : "occurrences";
  viewRef.current = view;
  const currentSimilar = similar && (!similar.response || similar.response.connection_id === sessionConnectionId) ? similar : null;
  const selectedCluster = currentSimilar?.response?.clusters.find((cluster) => cluster.cluster_key === currentSimilar.selectedKey) ?? null;
  const title = {
    active_at: text("In flight at the selected time", "所选时刻的在途查询"),
    family_detail: text("Query family detail", "查询族详情"),
    members: text("Family members · by duration", "查询族成员 · 按耗时"),
    families: text("Repeated SQL", "同类 SQL"),
    similar: text("Similar SQL", "相似 SQL"),
    occurrences: text("Individual executions", "单次执行"),
  }[view];
  const filterNote = statementFilter?.state === "applied" ? text("this tool's and non-query statements hidden", "已隐藏本工具与非查询语句") : text("all statements shown", "显示全部语句");
  const subtitle = view === "similar" ? text("Merged above exact templates · every cluster shows its basis and can be split · computed for this window in memory", "在精确模板之上归并 · 每个聚类显示归并依据，可拆分 · 本窗口内存计算")
    : view === "families" ? `${text("Grouped by exact native digest", "按精确原生 digest 归组")} · ${filterNote}`
    : view === "occurrences" ? `${sortLabel(occurrenceSort, text)} · ${filterNote}`
    : view === "members" ? `${text("Exact family members in this window", "本窗口内精确查询族成员")} · ${filterNote}`
    : view === "family_detail" ? `${text("Parent window", "父窗口")} ${currentFamilyDetail ? formatUtcWindow(currentFamilyDetail.parentScope) : "—"}`
    : text("A fresh read of the retained intervals covering that instant", "按审计区间重新读取覆盖该时刻的查询");
  const hiddenTitle = statementFilter?.state === "applied" ? text(`Hidden: ${statementFilter.hidden_non_query_row_count} non-query and ${statementFilter.hidden_doctor_row_count} tool statements`, `已隐藏 ${statementFilter.hidden_non_query_row_count} 条非查询语句、${statementFilter.hidden_doctor_row_count} 条本工具语句`) : undefined;
  const events = coverage ? countText(coverage.retained_event_count) ?? coverage.retained_event_count : "—";
  const footLeft = view === "families" && currentFamilies
    ? `${text(`${currentFamilies.coverage.retained_family_count} query families`, `${currentFamilies.coverage.retained_family_count} 个查询族`)} · ${text(`${events} executions in the window`, `窗口内 ${events} 次执行`)}${currentFamilies.top.truncated ? ` · ${text(`showing the first ${currentFamilies.top.returned_count}`, `仅显示前 ${currentFamilies.top.returned_count} 个`)}` : ""}`
    : view === "occurrences" && currentOccurrences
    ? `${text(`${events} executions in the window`, `窗口内 ${events} 次执行`)} · ${text(`showing ${currentOccurrences.top.returned_count}`, `显示 ${currentOccurrences.top.returned_count} 条`)}${currentOccurrences.top.truncated ? ` · ${text("partial", "部分结果")}` : ""}`
    : view === "members" && familySelection?.members
    ? text(`${familySelection.members.top.returned_count} of ${familySelection.members.top.full_window_count} members`, `${familySelection.members.top.full_window_count} 次中前 ${familySelection.members.top.returned_count}`)
    : "";
  const moreFiltersId = "audit-more-filters";
  const readPerformance = api.readAuditPerformance;
  const readHistoryLoad = api.readHistoryLoad;
  const readLoadOverview = api.readLoadOverview;
  const overviewView = overviewShown && readPerformance !== undefined && (view === "families" || view === "occurrences");
  const viewItems: { value: "families" | "similar" | "occurrences"; label: string; disabled?: boolean }[] = [
    ...((!digestUnavailable || mode === "families") ? [{ value: "families" as const, label: text("Repeated SQL", "同类 SQL"), disabled: loading }] : []),
    ...(api.readAuditSimilarSql ? [{ value: "similar" as const, label: text("Similar SQL", "相似 SQL"), disabled: loading }] : []),
    { value: "occurrences" as const, label: text("Individual executions", "单次执行"), disabled: loading },
  ];
  const activeSubView: "families" | "similar" | "occurrences" = view === "similar" ? "similar" : view === "occurrences" ? "occurrences" : "families";
  const viewSwitch = <SegTabs<"families" | "similar" | "occurrences"> aria-label={text("View", "视图")} items={viewItems} onValueChange={(next) => { setOverviewShown(false); if (next === "similar") chooseSimilar(); else if (!(overviewView && mode === next)) chooseMode(next); }} size="sm" value={activeSubView} />;
  const describePerformanceFailure = (cause: unknown): string => {
    if (isConnectionExpiredFailure(cause)) onSessionExpired?.(sessionEndReason(cause));
    return auditErrorLabel(cause);
  };
  const drillToWindow = (startMilliseconds: string, endMilliseconds: string, target: "occurrences" | "families"): void => {
    const base = currentOverview?.scope ?? scope;
    const nextScope: AuditScope = { ...base, window: { start_seconds: Number(startMilliseconds) / 1000, end_seconds: Number(endMilliseconds) / 1000 } };
    setOverviewShown(false);
    setMode(target);
    setScope(nextScope);
    if (target === "families") void loadFamilies(nextScope);
    else void loadOccurrences(nextScope);
    if (surface === "overview") onHandoff?.();
  };
  const filterDimension = (dimension: AuditPerformanceDimensionId, value: string): void => {
    const base = currentOverview?.scope ?? scope;
    const field = dimension === "resource_group" ? "resource_group" : dimension;
    const nextScope: AuditScope = { ...base, [field]: value };
    setScope(nextScope);
    loadForMode(nextScope);
  };

  const rangeToggle = readHistoryLoad ? <SegTabs<"window" | "history"> aria-label={text("Overview range", "概览范围")} items={[{ value: "window", label: text("Selected window · audit table", "所选时间窗 · 审计表") }, { value: "history", label: text("30 days · history", "30 天 · 历史库") }]} onValueChange={setOverviewRange} size="sm" value={overviewRange} /> : null;
  const similarDrawerOpen = !overviewView && view === "similar" && Boolean(selectedCluster && currentSimilar?.response);
  const familyDrawerOpen = !overviewView && view !== "similar" && Boolean(familySelection && familySelection.drawerOpen && !currentActiveAt && !familyDetailShown);
  const occurrenceDrawerOpen = !overviewView && view !== "similar" && !familyDrawerOpen && occurrenceSelection.drawerOpen && selected !== null;

  return <section className="audit-workload audit-workload-overview" aria-busy={loading} aria-labelledby="audit-board-title" data-view={overviewView ? "overview" : view}>
    <form className="workspace-bar audit-filter-bar" aria-label={text("Filters", "筛选")} onSubmit={(event) => { event.preventDefault(); refresh(); }}>
      <div className="filter-chip-input"><label htmlFor="audit-filter-window">{text("Window", "窗口")}</label><select aria-label={text("Audit window", "审计窗口")} className="audit-filter-window" id="audit-filter-window" onChange={(event) => setWindowPreset(event.target.value as AuditWindowPreset)} title={response ? formatUtcWindow(response.scope) : undefined} value={windowPreset}>{windowPresets.map((preset) => <option key={preset} value={preset}>{windowPresetLabel(preset, text)}</option>)}</select></div>
      <div className="filter-chip-input"><label htmlFor="audit-filter-database">{text("DB", "库")}</label><input aria-label={text("Database filter", "数据库筛选")} className="audit-filter-database" id="audit-filter-database" onChange={(event) => setScope((current) => ({ ...current, database: emptyToNull(event.target.value) }))} placeholder={text("All databases", "全部数据库")} value={scope.database ?? ""} /></div>
      <div className="filter-chip-input"><label htmlFor="audit-filter-user">{text("User", "用户")}</label><input aria-describedby={userFilterAvailable ? undefined : "audit-user-filter-capability"} aria-label={text("Exact user filter", "精确用户筛选")} className="audit-filter-user" disabled={!userFilterAvailable} id="audit-filter-user" onChange={(event) => setScope((current) => ({ ...current, user: emptyToNull(event.target.value) }))} placeholder={text("All users", "全部用户")} title={userFilterAvailable ? undefined : auditUserFilterNote(userFilterCapability, text)} value={scope.user ?? ""} /></div>
      <div className="filter-chip-input"><label htmlFor="audit-filter-state">{text("State", "状态")}</label><select aria-label={text("Audit source state", "审计源状态")} className="audit-filter-state" id="audit-filter-state" onChange={(event) => setScope((current) => ({ ...current, state: auditTerminalStateFromControl(event.target.value) }))} value={scope.state ?? ""}><option value="">{text("All terminal states", "全部终态")}</option><option value="finished">{text("Finished", "已完成")}</option><option value="error">{text("Failed", "执行失败")}</option></select></div>
      <div className="filter-chip-input"><label htmlFor="audit-filter-limit">{text("Limit", "上限")}</label><select aria-label={text("Top limit", "Top 上限")} className="audit-filter-limit" id="audit-filter-limit" onChange={(event) => setLimit(Number(event.target.value) as (typeof limits)[number])} value={limit}>{limits.map((value) => <option key={value} value={value}>{text(`${value} rows`, `${value} 条`)}</option>)}</select></div>
      {mode === "similar" && <fieldset className="audit-similar-dimensions"><legend className="sr-only">{text("Merge basis", "归并依据")}</legend>{([["tables", text("Tables", "表集合")], ["join", text("Join shape", "Join 形态")], ["predicates", text("Predicate columns", "谓词列集合")]] as const).map(([key, label]) => <label className="check-inline" key={key}><input checked={similarDimensions[key]} disabled={similarDimensions[key] && Object.values(similarDimensions).filter(Boolean).length === 1} onChange={(event) => setSimilarDimensions((current) => ({ ...current, [key]: event.target.checked }))} type="checkbox" /><span>{label}</span></label>)}</fieldset>}
      <button aria-controls={moreFiltersId} aria-expanded={moreFilters} className="btn btn-ghost btn--sm audit-more-filters" onClick={() => setMoreFilters((open) => !open)} type="button">{text("More filters", "更多筛选")}</button>
      <div className="workspace-bar-right audit-filter-actions">
        {/* Task 68: on the Overview page the range sits in the filter row instead of a row of its own. */}
        {surface === "overview" && overviewView ? rangeToggle : null}
        <InfoTip label={text("Manual refresh · up to the last 30 days", "手动刷新 · 最多最近 30 天")}>{text("Audit records are read only when you refresh, for up to the last 30 days.", "只在点击刷新时读取审计记录，最多最近 30 天。")}</InfoTip>
        <Button disabled={loading} size="sm" type="submit" variant="secondary">{loading ? text("Refreshing…", "刷新中…") : text("Refresh", "刷新")}</Button>
      </div>
      <div className="audit-filter-more" hidden={!moreFilters} id={moreFiltersId}>
        <div className="audit-statement-filter">
          <label className="check-inline"><input aria-describedby="audit-statement-filter-note" aria-label={text("Hide this tool's queries and non-query statements", "隐藏本工具查询与非查询语句")} checked={scope.statement_filter === "exclude_doctor_and_non_query"} disabled={!statementFilterAvailable} onChange={(event) => setScope((current) => ({ ...current, statement_filter: event.target.checked ? "exclude_doctor_and_non_query" : "all" }))} type="checkbox" /><span>{text("Hide this tool's queries and non-query statements", "隐藏本工具查询与非查询语句")}</span></label>
          <span className="audit-filter-note muted" id="audit-statement-filter-note">{statementFilterAvailable
            ? text("Keeps rows with isQuery = 1 only. SET/SHOW/DDL/INSERT and other non-query statements, and statements sent by this tool, are hidden; the hidden counts are shown in the table subtitle.", "仅保留 isQuery = 1 的记录；SET/SHOW/DDL/INSERT 等非查询语句及本工具自身发出的语句会被隐藏，隐藏条数见表格副标题。")
            : auditStatementFilterNote(statementFilterCapability, text)}</span>
        </div>
        <label className="field"><span className="field-label">{text("Catalog", "Catalog")}</span><input aria-label={text("Catalog filter", "Catalog 筛选")} className="input" onChange={(event) => setScope((current) => ({ ...current, catalog: emptyToNull(event.target.value) }))} value={scope.catalog ?? ""} /></label>
        <label className="field"><span className="field-label">{text("Resource group", "资源组")}</span><input aria-label={text("Resource group filter", "资源组筛选")} className="input" onChange={(event) => setScope((current) => ({ ...current, resource_group: emptyToNull(event.target.value) }))} value={scope.resource_group ?? ""} /></label>
        <label className="field"><span className="field-label">{text("Rank individual executions", "单次执行：优先看什么")}</span><select aria-label={text("Rank individual executions", "单次执行：优先看什么")} className="select" onChange={(event) => setOccurrenceSort(event.target.value as AuditOccurrenceSort)} value={occurrenceSort}>{occurrenceSorts.map((sort) => <option key={sort} value={sort}>{sortLabel(sort, text)}</option>)}</select></label>
        <label className="field"><span className="field-label">{text("Rank repeated SQL", "同类 SQL：优先看什么")}</span><select aria-label={text("Rank repeated SQL", "同类 SQL：优先看什么")} className="select" onChange={(event) => setFamilySort(event.target.value as AuditFamilySort)} value={familySort}>{familySorts.map((sort) => <option key={sort} value={sort}>{sortLabel(sort, text)}</option>)}</select></label>
        {!userFilterAvailable && <span className="audit-filter-note muted" id="audit-user-filter-capability">{auditUserFilterNote(userFilterCapability, text)}</span>}
        {windowPreset === "30d" && <p className="audit-filter-note muted" role="note">{text("A 30-day read may cover every retained partition. Narrow the window or filters if the query exceeds its budget.", "30 天查询可能覆盖全部保留分区；若查询超出预算，请缩小时间窗或筛选范围。")}</p>}
      </div>
    </form>

    {!familyDetailShown && view !== "similar" && <AuditWindowEvidence compact response={response} scope={scope} windowPreset={windowPreset} text={text} />}
    {error && <div className="notice-banner notice-banner--danger" role="alert">{error === "not_found_or_expired_or_not_profiled" ? text("No Profile is available for this query. It may not have been collected or may have expired.", "这条查询没有可用的 Profile：可能未采集，或已过期。请选择另一条查询。") : error}</div>}
    {/* Task 66: the in-flight strip (pick a time to list what was running) sits above the query lists; the Overview has its own cards. */}
    {currentOverview && !familyDetailShown && view !== "similar" && surface === "queries" && !overviewView && <AuditKpiStrip concurrency={currentOverview.concurrency} coverage={currentOverview.coverage} disabled={loading} onSelectTime={(at) => void selectTime(at)} text={text} />}

    {overviewView && <div className="audit-overview-head">
      <div>{/* The Overview page names itself in its view tabs (design board 02); the heading stays for screen readers. */}<h1 className={surface === "overview" ? "sr-only" : undefined} id="audit-board-title">{text("Overview · audit performance", "概览 · 审计性能")}</h1><p data-audit-statement-filter={statementFilter?.state} title={hiddenTitle}>{filterNote}</p></div>
      <div className="audit-overview-head-controls">
        {surface === "overview" ? null : rangeToggle}
        {surface === "overview" ? null : viewSwitch}
      </div>
    </div>}
    {overviewView && overviewRange === "history" && readHistoryLoad && <AuditHistoryLoad
      api={{ readHistoryLoad: readHistoryLoad.bind(api) }}
      connectionId={session.connection_id}
      describeFailure={describePerformanceFailure}
      text={text}
    />}
    {overviewView && overviewRange === "window" && readPerformance && (currentOverview
      ? <AuditPerformanceOverview
        api={{ readAuditPerformance: readPerformance.bind(api) }}
        concurrency={currentOverview.concurrency}
        connectionId={session.connection_id}
        coverage={currentOverview.coverage}
        describeFailure={describePerformanceFailure}
        families={currentFamilies?.rows ?? []}
        familySortLabel={sortLabel(familySort, text)}
        filterableUser={userFilterAvailable}
        onDrill={drillToWindow}
        onFilter={filterDimension}
        onOpenFamilies={() => { setOverviewShown(false); if (mode !== "families") chooseMode("families"); if (surface === "overview") onHandoff?.(); }}
        dashboard={surface === "overview"}
        extraCard={extraCard}
        loadCard={readLoadOverview && onOpenLoads ? <LoadSummaryCard api={{ readLoadOverview: readLoadOverview.bind(api) }} connectionId={session.connection_id} describeFailure={describePerformanceFailure} onOpen={onOpenLoads} text={text} /> : undefined}
        scope={currentOverview.scope}
        text={text}
      />
      : !error && <p className="audit-empty" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>)}
    {!overviewView && <>
      <div className="audit-board-layout">
        <section aria-labelledby="audit-board-title" className="card card--table audit-board-main">
          <header className="card-hd audit-board-main-head">
            <div className="card-title-group"><h1 className="card-title" id="audit-board-title">{title}</h1><p className="card-subtitle" data-audit-statement-filter={statementFilter?.state} title={hiddenTitle}>{subtitle}</p></div>
            <div className="card-hd-actions">{view === "families" || view === "occurrences" || view === "similar"
              ? viewSwitch
              : <Button disabled={loading} onClick={() => {
                if (view === "members") setFamilySelection((current) => current ? { ...current, showAll: false } : current);
                else if (view === "family_detail") returnToParent();
                else loadForMode(currentOverview?.scope ?? scope);
              }} size="sm" variant="secondary">{text("Back to the list", "返回列表")}</Button>}</div>
          </header>
          {/* Task 69: the card's one scrolling body; the head and foot stay put. */}
          <div className="card-bd card-bd--flush audit-board-main-body">
          {currentActiveAt && <AuditActiveAtEvidence chartCount={currentOverview?.concurrency.series.state === "exact" ? auditSegmentAt(currentOverview.concurrency.series.points, currentActiveAt.at_milliseconds)?.active_count : undefined} response={currentActiveAt} text={text} />}
          {view === "family_detail" && currentFamilyDetail
            ? <AuditFamilyDetail detail={currentFamilyDetail.response} onBrowseMembers={browseFamilyMembers} onFetchProfile={(occurrence) => void fetchProfile(occurrence)} onReturn={returnToParent} onSelectRepresentative={selectFamilyRepresentative} onViewSql={(occurrence) => void viewSql(occurrence)} shownSqlQueryId={shownSql !== null ? selected?.query.query_id ?? null : null} parentWindow={formatUtcWindow(currentFamilyDetail.parentScope)} profileLoading={profileLoading} sqlLoading={sqlLoading} text={text} />
            : view === "members" && familySelection?.members
            ? <><AuditOccurrenceTable disabled={loading} occurrences={familySelection.members} onSelect={selectMember} selectedKey={familySelection.memberKey} text={text} /><div className="audit-members-detail"><Button disabled={loading} onClick={() => openFamily(familySelection.family, familySelection.members)} size="sm" variant="secondary">{text("Distribution and representatives", "耗时分布与代表执行")}</Button></div></>
            : view === "similar"
            ? currentSimilar?.error
              ? <div className="notice-banner notice-banner--danger" role="alert">{currentSimilar.error}</div>
              : currentSimilar?.response
              ? <AuditSimilarSqlList onSelect={(cluster: SimilarSqlCluster) => setSimilar((current) => current ? { ...current, selectedKey: cluster.cluster_key } : current)} response={currentSimilar.response} selectedKey={currentSimilar.selectedKey} text={text} />
              : <p className="audit-empty state-box" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>
            : view === "families"
            ? <>{digestUnavailable
              ? <AuditDigestGuidance text={text} />
              : loading && !currentFamilies
              ? <p className="audit-empty state-box" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>
              : currentFamilies && currentFamilies.rows.length === 0
              ? <p className="audit-empty state-box">{text("No exact query families were returned for this fixed window.", "此固定窗口未返回精确查询族。")}</p>
              : <AuditFamilyList disabled={loading} families={currentFamilies?.rows ?? []} familyTop={currentOverview?.family_top ?? null} onSelect={(family) => void selectFamily(family)} selectedKey={familySelection && familySelection.family.membership.state === "exact_native_digest" ? familySelection.family.membership.family_key : null} sort={familySort} text={text} />}
              {currentFingerprints && <AuditFingerprintFamilyList error={currentFingerprints.error} loading={currentFingerprints.loading} onShowExecutions={() => chooseMode("occurrences")} response={currentFingerprints.response} text={text} />}</>
            : <AuditOccurrenceTable activeAt={currentActiveAt} disabled={loading} occurrences={currentOccurrences} onSelect={selectOccurrence} selectedKey={selected ? occurrenceRowKey(selected, occurrenceRows?.indexOf(selected) ?? 0) : null} text={text} />}
          {activeFamily && mode === "family" && !familyDetail && <FamilySummary family={activeFamily} onReturn={returnToParent} text={text} />}
          </div>
          <footer className="card-ft audit-board-main-foot"><span>{footLeft}</span><span>{text("Audit records may be ingested late", "审计记录可能延迟入库")}</span></footer>
        </section>
      </div>
      <AuditSimilarAside
        cluster={view === "similar" ? selectedCluster : null}
        dimensions={currentSimilar?.response?.dimensions ?? null}
        loading={loading}
        onClose={() => setSimilar((current) => current ? { ...current, selectedKey: null } : current)}
        onDetach={(keys) => {
          if (currentSimilar?.response && selectedCluster) {
            void loadSimilar(currentSimilar.response.scope, [...currentSimilar.detached, ...keys], currentSimilar.refined, selectedCluster.cluster_key);
          }
        }}
        onRefine={() => {
          if (currentSimilar?.response && selectedCluster) {
            void loadSimilar(currentSimilar.response.scope, currentSimilar.detached, [...currentSimilar.refined, selectedCluster.cluster_key]);
          }
        }}
        onUndo={() => {
          if (currentSimilar?.response && selectedCluster) {
            void loadSimilar(currentSimilar.response.scope, [], currentSimilar.refined, selectedCluster.cluster_key);
          }
        }}
        open={similarDrawerOpen}
        splitCount={currentSimilar?.detached.length ?? 0}
        text={text}
      />
      <AuditFamilyDrawer
        baseline={baselineState(baselineQueryId, familySelection?.members ?? null, selectedMember)}
        canDiagnose={selectedMember?.query.profile_fetch.state === "available" && Boolean(onOpenDoctor)}
        canPlanCheck={selectedMember?.query.plan_check.state === "available"}
        error={familySelection?.error ?? null}
        family={familySelection?.family ?? null}
        fetchedQueryIds={fetchedQueryIds}
        loading={familySelection?.loading ?? false}
        memberSql={memberSql}
        memberSqlError={memberSqlRequest ? sqlError : null}
        memberSqlLoading={sqlLoading}
        members={familySelection?.members ?? null}
        onClose={closeFamily}
        onCompareWithBaseline={onOpenCompare ? () => void compareWithBaseline() : undefined}
        onDiagnose={() => void fetchMemberProfile("diagnose")}
        onPlanCheck={onOpenPlanCheck ? planCheckMember : undefined}
        onSelectMember={selectMember}
        onSetBaseline={onSetBaseline ? () => void fetchMemberProfile("baseline") : undefined}
        onShowMembers={familySelection?.showAll ? undefined : () => setFamilySelection((current) => closeFamilyDrawer(current, "show_members"))}
        onViewMemberSql={() => void viewMemberSql()}
        open={familyDrawerOpen}
        profileLoading={profileLoading}
        restoreFocus={familySelection?.restoreFocus ?? true}
        selectedMemberKey={familySelection?.memberKey ?? null}
        text={text}
      />
      <SelectedAuditOccurrence
        doctorProfile={doctorProfile}
        occurrence={selected}
        onClose={closeOccurrence}
        onDiagnose={() => void fetchProfile(selected, true)}
        onOpenDoctor={onOpenDoctor ? (nextProfile) => {
          setOccurrenceSelection((current) => closeOccurrenceDrawer(current, "open_doctor"));
          onOpenDoctor(nextProfile);
        } : undefined}
        onViewSql={() => void viewSql()}
        open={occurrenceDrawerOpen}
        profile={visibleProfile}
        profileError={profileError}
        profileLoading={profileLoading}
        restoreFocus={occurrenceSelection.restoreFocus}
        sql={shownSql}
        sqlError={sqlError}
        sqlLoading={sqlLoading}
        text={text}
      />
    </>}
  </section>;
}

function AuditUnavailable({ onConnect, session, text }: { onConnect(): void; session?: ConnectionSessionView; text: LocalizedText }): JSX.Element {
  const detail = !session
    ? text("Connect to a database with AuditLoader to read its query history.", "连接已安装 AuditLoader 的数据库后可读取查询历史。")
    : text(`Audit workload is unavailable for this connection: ${lookupEnumLabel({ unavailable: ["unavailable", "不可用"], forbidden: ["forbidden", "禁止"], unknown: ["unknown", "未知"], available: ["available", "可用"] }, session.capabilities.audit, text)}. The existing Queries fallback remains separate.`, `该连接的 Audit 工作负载不可用：${lookupEnumLabel({ unavailable: ["unavailable", "不可用"], forbidden: ["forbidden", "禁止"], unknown: ["unknown", "未知"], available: ["available", "可用"] }, session.capabilities.audit, text)}。既有 Queries 降级入口保持独立。`);
  return <section className="card card--detail audit-unavailable"><div className="card-bd state-box"><strong>{text("Audit workload unavailable", "审计工作负载不可用")}</strong><p>{detail}</p><Button onClick={onConnect} size="sm" variant="primary">{text("Connect StarRocks", "连接 StarRocks")}</Button></div></section>;
}

export function AuditWindowEvidence({ response, scope, text, windowPreset }: { compact?: boolean; response: AuditOccurrenceResponse | AuditFamilyResponse | null; scope: AuditScope; text: LocalizedText; windowPreset?: AuditWindowPreset }): JSX.Element {
  const coverage = response?.coverage;
  const resultScope = response?.scope ?? scope;
  const pendingDraft = response !== null && (scopeKey(response.scope) !== scopeKey(scope) || (windowPreset !== undefined && auditWindowPresetForScope(response.scope) !== windowPreset));

  // The applied window, hidden counts and returned counts live in the filter and table
  // (design board 01); this strip appears only when something needs the reader's attention.
  const conflicting = positiveCount(coverage?.conflicting_duplicate_row_count);
  const ungrouped = positiveCount(coverage?.missing_digest_count) && !auditDigestUnavailable(coverage);
  const truncated = response?.top.truncated === true;
  if (!pendingDraft && !conflicting && !ungrouped && !truncated) return <></>;
  return <section className="notice-banner notice-banner--warn audit-window-evidence" aria-label={text("Audit window coverage", "审计窗口覆盖")}>
    <div className="audit-window-evidence-stack">
      {truncated && <p><strong>{text("Partial results; narrow the filters or increase the limit.", "仅展示部分结果，可缩小筛选范围或增加条数上限。")}</strong> <span title={formatUtcWindow(resultScope)}>{text("Returned", "已返回")} {response?.top.returned_count ?? "—"} / {response?.top.full_window_count ?? "—"}</span></p>}
      {pendingDraft && <p role="status">{text("Filters changed. Refresh to update the results.", "筛选条件已修改，点击刷新后更新结果。")}</p>}
      {conflicting && <p role="status">{text("Some audit records conflict; affected metrics may be unavailable.", "部分审计记录存在冲突，相关指标可能缺失。")}</p>}
      {ungrouped && <p>{text("Some queries lack a native SQL digest; they are grouped separately below by unofficial fingerprint.", "部分查询缺少原生 SQL digest，已在下方按非官方指纹单独归组。")}</p>}
    </div>
  </section>;
}

export function AuditActiveAtEvidence({ chartCount, response, text }: { chartCount?: string; response: AuditActiveAtResponse; text: LocalizedText }): JSX.Element {
  return <section className="audit-window-evidence" aria-label={text("Selected-time evidence", "所选时刻证据")}>
    <p><strong>{text("Selected time", "所选时刻")}</strong> <span title={`${response.at_milliseconds} ms`}>{auditClockText(response.at_milliseconds)}</span> · {text("reader session zone", "读取会话时区")} {response.audit_timestamp_time_zone?.effective_time_zone ?? "—"} · {text("historical in-flight", "历史在途")} {response.active_count}</p>
    <AuditBoundedSourceNote source={response.source_coverage} text={text} />
    <p>{text("Fresh separate read; not the chart snapshot. Includes eligible queries within the fixed lookback. This is not a blocked-query list.", "独立重新读取，不代表原曲线同一快照；包含固定回溯范围内的有效查询，不是阻塞 SQL 列表。")}</p>
    {chartCount !== undefined && <p>{text("Original chart sample count", "原曲线样本数")} {chartCount} · {text("Fresh read count", "重新读取数量")} {response.active_count}{chartCount !== response.active_count && <strong> · {text("Audit source changed; refresh the chart to realign.", "审计来源已变化，请刷新图表重新对齐。")}</strong>}</p>}
    <p>{text("Returned", "已返回")} {response.participants.rows.length} · {response.participants.state === "truncated"
      ? text(`Participant list truncated; ${response.participants.omitted_count} omitted. The curve is not a Top-500 sample.`, `参与者列表已截断，省略 ${response.participants.omitted_count} 条；曲线不是 Top 500 样本。`)
      : text("Complete participant list for this read", "本次读取的完整参与者列表")}</p>
  </section>;
}

export function AuditOccurrenceTable({ activeAt = null, disabled = false, occurrences, onSelect, selectedKey, text }: { activeAt?: AuditActiveAtResponse | null; disabled?: boolean; occurrences: AuditOccurrenceResponse | null; onSelect(key: string): void; selectedKey: string | null; text: LocalizedText }): JSX.Element {
  const rows = activeAt ? activeAt.participants.rows.map((row) => row.occurrence) : occurrences?.rows ?? [];
  const title = activeAt ? text("Historical in-flight participants", "历史在途参与者") : text("Individual executions", "单次执行");
  return <section className="audit-results" aria-labelledby="audit-occurrences-title">
    <h3 className="sr-only" id="audit-occurrences-title">{title}</h3>
    <div className="audit-table-wrap"><table aria-label={activeAt ? title : text("Audited query rankings", "审计查询排行")} className={activeAt ? "data-table" : "data-table audit-occurrence-table"}><thead><tr>
      <th>{text("Query ID", "查询 ID")}</th><th>SQL</th><th>{text("Start / reader zone", "开始 / 读取时区")}</th><th>{text("Source state", "源状态")}</th>
      {activeAt && <><th>{text("Approx. end / reader zone", "近似结束 / 读取时区")}</th><th>{text("Elapsed at selected time", "截至所选时刻已持续")}</th></>}
      <th className="num" title="queryTime">{text("Query duration", "查询耗时")}</th><th className="num" title="scanRows">{text("Scanned rows", "扫描行数")}</th><th className="num" title="returnRows">{text("Returned rows", "返回行数")}</th><th className="num" title="scanBytes">{text("Scanned bytes", "扫描数据量")}</th><th className="num" title="cpuCost">{text("CPU time", "CPU 耗时")}</th><th className="num" title="memoryCost">{text("Audit memory", "审计内存")}</th><th>Profile</th>
    </tr></thead><tbody>{rows.map((row, index) => {
      const key = occurrenceRowKey(row, index);
      const participant = activeAt?.participants.rows[index];
      const isSelected = key === selectedKey;
      return <tr className={`audit-row${isSelected ? " is-selected" : ""}`} key={key} onClick={(event) => { if (!disabled) { event.currentTarget.querySelector<HTMLButtonElement>(".row-pick-btn")?.focus(); onSelect(key); } }}>
        <td className="audit-cell-id"><button aria-haspopup="dialog" aria-label={`${text("Select audited query", "选择审计查询")} ${row.query.query_id}`} className="row-pick-btn mono" disabled={disabled} onClick={(event) => { event.stopPropagation(); onSelect(key); }} title={row.query.query_id} type="button">{compactQueryId(row.query.query_id)}</button></td>
        <OccurrenceStatementCell sample={row.statement_sample} text={text} />
        <td className="audit-cell-time mono">{observedAt(row.query)}</td>
        <td className="audit-cell-status" title={row.query.status}><span className="status-inline"><span className={statusDotClass(row.query.status)} />{auditStatusLabel(row.query.status, text)}</span></td>
        {participant && <><td className="num audit-cell-num" title={`${participant.end_milliseconds} ms`}>{auditClockText(participant.end_milliseconds)}</td><td className="num audit-cell-num" title={`${participant.elapsed_milliseconds} ms`}>{auditMillisecondsText(participant.elapsed_milliseconds)}</td></>}
        {(["query_time", "scan_rows", "return_rows", "scan_bytes", "cpu_cost", "memory_cost"] as const).map((id) => <td className="num audit-cell-num" key={id} title={metricRawText(metric(row, id))}>{metricValue(metric(row, id), text)}</td>)}
        <td className="audit-cell-profile" title={row.query.profile_fetch.state === "available" ? undefined : row.query.profile_fetch.reason}>{row.query.profile_fetch.state === "available" ? <span className="status-inline"><span className="status-dot status-dot--warn" />{profileLabel(row.query, text)}</span> : <span className="state-stamp">{profileLabel(row.query, text)}</span>}</td>
      </tr>;
    })}</tbody></table></div>
    {disabled && rows.length === 0 && <p className="audit-empty state-box" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>}
    {!disabled && (activeAt || occurrences) && rows.length === 0 && <p className="audit-empty state-box">{activeAt ? text("No retained query interval covers this time.", "没有保留查询区间覆盖该时刻。") : text("No occurrences were returned for this fixed window.", "此固定窗口未返回执行记录。")}</p>}
    {!activeAt && <details className="disclosure audit-coverage"><summary>{text("Metric coverage details", "指标覆盖详情")}</summary><div className="disclosure-body"><p>{coverageMetricLine(occurrences?.coverage, text)}</p></div></details>}
  </section>;
}

export function FamilySummary({ family, onReturn, text }: { family: AuditFamilyPresentation; onReturn?(): void; text: LocalizedText }): JSX.Element {
  return <section className="audit-family-summary"><div><h3>{text("Exact native digest family", "精确原生 digest 查询族")}</h3>{onReturn && <button className="btn btn-secondary btn--sm" onClick={() => onReturn()} type="button">{text("Return to parent window", "返回父窗口")}</button>}</div><p><FamilyLabel family={family} /></p><dl className="kv-grid">
    <dt>{text("Execution count", "执行次数")}</dt><dd className="mono">{family.execution_count ?? "—"}</dd>
    <dt>{text("Error proportion · ERR / total", "错误占比 · ERR / 总数")}</dt><dd className="mono" title={errorProportionText(family.error_proportion)}>{errorProportionText(family.error_proportion)}</dd>
    <dt>{text("Cumulative query time", "累计查询耗时")}</dt><dd className="mono" title={aggregateRawText(family, "query_time", "sum")}>{metricValue(aggregateMetric(family, "query_time", "sum"), text)}</dd>
    <dt>{text("Cumulative CPU", "累计 CPU")}</dt><dd className="mono" title={aggregateRawText(family, "cpu_cost", "sum")}>{metricValue(aggregateMetric(family, "cpu_cost", "sum"), text)}</dd>
    <dt>{text("Maximum query time", "最大查询耗时")}</dt><dd className="mono" title={aggregateRawText(family, "query_time", "max")}>{metricValue(aggregateMetric(family, "query_time", "max"), text)}</dd>
    <dt>{text("Maximum audit memory", "最大审计内存")}</dt><dd className="mono" title={aggregateRawText(family, "memory_cost", "max")}>{metricValue(aggregateMetric(family, "memory_cost", "max"), text)}</dd>
    <dt>{text("Cumulative scanned bytes", "累计扫描数据量")}</dt><dd className="mono" title={aggregateRawText(family, "scan_bytes", "sum")}>{metricValue(aggregateMetric(family, "scan_bytes", "sum"), text)}</dd>
  </dl><p className="muted">{text("Family values are returned by the server full-window aggregation; the member Top list is never summed here. CPU sum is not wall-clock time; audit memory max is not concurrent total memory.", "查询族数值由服务端全窗口聚合返回，绝不相加成员 Top 列表。CPU 累计不是墙钟时间；审计内存 max 不是并发总内存。")}</p></section>;
}

export function AuditFamilyDrawer({
  family,
  members,
  memberSql = null,
  memberSqlError = null,
  memberSqlLoading = false,
  onViewMemberSql,
  loading,
  error,
  selectedMemberKey,
  fetchedQueryIds,
  profileLoading,
  canDiagnose,
  canPlanCheck,
  baseline = "none",
  open = family !== null,
  restoreFocus,
  onClose,
  onSelectMember,
  onDiagnose,
  onSetBaseline,
  onCompareWithBaseline,
  onPlanCheck,
  onShowMembers,
  text,
}: {
  family: AuditFamilyPresentation | null;
  members: AuditOccurrenceResponse | null;
  /** Full SQL of the selected member, once read; null while hidden. */
  memberSql?: string | null;
  memberSqlError?: string | null;
  memberSqlLoading?: boolean;
  onViewMemberSql?(): void;
  loading: boolean;
  error: string | null;
  selectedMemberKey: string | null;
  fetchedQueryIds: ReadonlySet<string>;
  profileLoading: boolean;
  canDiagnose: boolean;
  canPlanCheck: boolean;
  baseline?: BaselineState;
  open?: boolean;
  restoreFocus?: boolean;
  onClose(): void;
  onSelectMember(key: string): void;
  onDiagnose(): void;
  onSetBaseline?(): void;
  onCompareWithBaseline?(): void;
  onPlanCheck?(): void;
  onShowMembers?(): void;
  text: LocalizedText;
}): JSX.Element {
  const exact = family?.membership.state === "exact_native_digest" ? family.membership : null;
  const sample = family?.statement_sample ?? null;
  const rows = familyDrawerMemberRows(members, selectedMemberKey);
  const topCount = Math.min(members?.rows.length ?? 0, 4);
  const hasSelectedOutsideTop = !onShowMembers && rows.length > topCount;
  const shownOf = hasSelectedOutsideTop
    ? text(`top ${topCount} + selected of ${family?.execution_count ?? "—"}`, `${family?.execution_count ?? "—"} 次中前 ${topCount} + 当前已选`)
    : text(`top ${topCount} of ${family?.execution_count ?? "—"}`, `${family?.execution_count ?? "—"} 次中前 ${topCount}`);

  return (
    <Dialog
      description={
        family ? (
          <span className="audit-drawer-desc">
            <BadgeStamp kind="official_digest" />
            <span>{`${text("Database", "数据库")} ${exact?.context.database ?? "—"} · Catalog ${exact?.context.catalog ?? "—"}`}</span>
          </span>
        ) : undefined
      }
      footer={
        family ? (
          <div className="audit-family-aside-actions">
            <InfoTip label={text("About fetching", "关于拉取")} side="top">
              {text("Reads existing Profiles only; SQL is never rerun. A Profile may have expired.", "只读取既有 Profile，不会重新执行 SQL。Profile 缓存可能已过期。")}
            </InfoTip>
            <Button disabled={!canDiagnose || profileLoading} onClick={onDiagnose} size="sm" variant="primary">
              {profileLoading ? text("Fetching Profile…", "正在拉取 Profile…") : text("Fetch Profile and diagnose", "拉取 Profile 并诊断")}
            </Button>
            {baseline === "comparable" && onCompareWithBaseline ? (
              <Button
                disabled={!canDiagnose || profileLoading}
                onClick={onCompareWithBaseline}
                size="sm"
                title={text("Compare the baseline with this execution; both are in this list", "将基线与这次执行对比；两者都在本列表中")}
                variant="secondary"
              >
                {text("Compare with baseline", "与基线对比")}
              </Button>
            ) : baseline === "selected" ? (
              <Button disabled size="sm" variant="secondary">
                {text("Baseline set", "已设为基线")}
              </Button>
            ) : (
              <Button
                disabled={!canDiagnose || profileLoading || !onSetBaseline}
                onClick={onSetBaseline}
                size="sm"
                title={
                  baseline === "elsewhere"
                    ? text("The current baseline is not in this list; this execution would replace it", "当前基线不在此列表中；设为基线会替换它")
                    : undefined
                }
                variant="secondary"
              >
                {text("Set as baseline", "设为基线")}
              </Button>
            )}
            {onPlanCheck && (
              <Button disabled={!canPlanCheck} onClick={onPlanCheck} size="sm" variant="secondary">
                {text("Plan check", "计划检查")}
              </Button>
            )}
            {onViewMemberSql && (
              <SqlToggleButton
                className="btn btn-secondary btn--sm"
                controls="audit-family-member-sql"
                disabled={!selectedMemberKey}
                expanded={memberSql !== null}
                hideLabel={text("Hide member SQL", "收起成员 SQL")}
                loading={memberSqlLoading}
                onToggle={onViewMemberSql}
                showLabel={text("View member full SQL", "查看成员完整 SQL")}
              />
            )}
          </div>
        ) : undefined
      }
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      open={open && family !== null}
      placement="right"
      restoreFocus={restoreFocus}
      title={
        family ? (
          <span className="audit-drawer-title">
            <span>{text("Selected query family · digest", "已选查询族 · digest")}</span>
            <code className="mono">{exact?.native_digest.slice(0, 12) ?? "—"}</code>
          </span>
        ) : (
          text("Selected query family", "已选查询族")
        )
      }
      width="lg"
    >
      {family && sample ? (
        <div className="audit-family-aside audit-family-aside-stack">
          {sample.state === "text" && sample.value.trim() !== "" ? (
            <SqlText
              copyLabel={sample.truncated ? text("Copy returned prefix", "复制已返回片段") : text("Copy SQL", "复制 SQL")}
              label={text("SQL of the most recent member, as audited", "最近一次成员的审计原文")}
              note={
                sample.truncated
                  ? text(
                      "Most recent member · sample truncated (prefix only); a member's full SQL is below",
                      "最近一次成员 · 样本已截断（只返回前缀），完整 SQL 可在下方成员中查看",
                    )
                  : text("Most recent member, as audited", "最近一次成员的审计原文")
              }
              value={sample.value}
            />
          ) : (
            <p className="muted">
              {sample.state === "text"
                ? text("Audited stmt is empty.", "审计 stmt 为空字符串。")
                : sample.state === "sql_null"
                  ? text("Audited stmt is NULL.", "审计 stmt 为空。")
                  : text("No SQL sample for this family.", "该查询族没有 SQL 示例。")}
            </p>
          )}

          <div className="audit-family-members-head">
            <span className="field-label">{text("Member executions · by duration", "成员执行 · 按耗时")}</span>
            {members ? (
              onShowMembers ? (
                <button
                  aria-label={`${shownOf} · ${text("All members and distribution", "全部成员与耗时分布")}`}
                  className="btn btn-ghost btn--sm"
                  onClick={onShowMembers}
                  title={text("All members and distribution", "全部成员与耗时分布")}
                  type="button"
                >
                  {shownOf}
                </button>
              ) : (
                <span className="muted">{shownOf}</span>
              )
            ) : (
              <span className="muted">{loading ? text("Loading…", "加载中…") : ""}</span>
            )}
          </div>

          {error && <div className="notice-banner notice-banner--danger" role="alert">{error}</div>}

          <ul className="item-list audit-family-members">
            {rows.map(({ key, row }) => {
              const fetch = row.query.profile_fetch;
              const state: FamilyMemberState =
                fetchedQueryIds.has(row.query.query_id) || (fetch.state === "available" && fetch.reason === "profile_fetched")
                  ? "fetched"
                  : fetch.state;
              const isSelected = key === selectedMemberKey;
              return (
                <li key={key}>
                  <button
                    aria-current={isSelected ? "true" : undefined}
                    className={`item-btn${isSelected ? " is-selected" : ""}`}
                    onClick={() => onSelectMember(key)}
                    title={row.query.query_id}
                    type="button"
                  >
                    <div className="item-row-top">
                      <code className="mono">{compactQueryId(row.query.query_id)}</code>
                      <strong className="mono" title={metricRawText(metric(row, "query_time"))}>
                        <AuditMetricValue text={text} value={metric(row, "query_time")} />
                      </strong>
                    </div>
                    <div className="item-row-meta">
                      <span>{familyMemberClock(row)} · {text("scanned", "扫描")} {familyMemberRows(row, text)}</span>
                      {state === "fetched" ? (
                        <span className="status-inline">
                          <span className="status-dot status-dot--ok" />
                          {text("Profile fetched", "Profile 已拉取")}
                        </span>
                      ) : state === "available" ? (
                        <span className="status-inline">
                          <span className="status-dot status-dot--warn" />
                          {text("Can try fetching", "可尝试拉取")}
                        </span>
                      ) : (
                        <span className="state-stamp">{text("Not fetchable yet", "暂不可拉取")}</span>
                      )}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>

          {memberSqlError && <div className="notice-banner notice-banner--danger" role="alert">{memberSqlError}</div>}
          {memberSql !== null && (
            <SqlText
              copyLabel={text("Copy SQL", "复制 SQL")}
              id="audit-family-member-sql"
              label={text("Selected member SQL", "所选成员 SQL")}
              value={memberSql}
            />
          )}
        </div>
      ) : null}
    </Dialog>
  );
}

function familyMemberClock(row: AuditOccurrencePresentation): string {
  const local = row.query.observed_at_local ?? row.query.observed_at_utc ?? "—";
  const match = /(\d\d:\d\d:\d\d)/.exec(local);
  return match ? match[1] : local;
}

function familyMemberRows(row: AuditOccurrencePresentation, text: LocalizedText): string {
  const scanRows = metric(row, "scan_rows");
  if (!scanRows || scanRows.state !== "exact" || !scanRows.canonical_number) return `— ${text("missing", "缺失")}`;
  return `${compactCountText(scanRows.canonical_number.value) ?? scanRows.canonical_number.value} ${text("rows", "行")}`;
}

export function SelectedAuditOccurrence({ doctorProfile, onClose, onDiagnose, onOpenDoctor, onViewSql, open, profile, profileError, profileLoading, occurrence, restoreFocus, sql, sqlError, sqlLoading, text }: {
  doctorProfile: ProfileFetchResponse | null;
  onClose?(): void;
  onDiagnose(): void;
  onOpenDoctor?: (profile: ProfileFetchResponse) => void;
  onViewSql(): void;
  open?: boolean;
  profile: ProfileFetchResponse | null;
  profileLoading: boolean;
  profileError?: string | null;
  occurrence: AuditOccurrencePresentation | null;
  restoreFocus?: boolean;
  sql: string | null;
  sqlError: string | null;
  sqlLoading: boolean;
  text: LocalizedText;
}): JSX.Element {
  const query = occurrence?.query ?? null;
  const isOpen = (open ?? true) && occurrence !== null && query !== null;
  const pending = occurrence ? metric(occurrence, "pending_time") : null;
  const errorCode = occurrence?.error_code;
  const pendingSource = !pending ? "not_returned" : pending.input.state === "text"
    ? `pendingTimeMs=${JSON.stringify(pending.input.value)} → ${metricRawText(pending)}`
    : pending.input.state === "unavailable" ? `unavailable · ${pending.input.reason}` : "sql_null";
  const errorCodeText = !errorCode ? "not_returned" : errorCode.state === "text"
    ? JSON.stringify(errorCode.value) : errorCode.state === "unavailable" ? `unavailable · ${errorCode.reason}` : "sql_null";
  const facts: [string, AuditMetricId][] = [
    [text("Query duration", "查询耗时"), "query_time"],
    [text("Scanned rows", "扫描行数"), "scan_rows"],
    [text("Scanned bytes", "扫描数据量"), "scan_bytes"],
    [text("CPU time", "CPU 耗时"), "cpu_cost"],
    [text("Returned rows", "返回行数"), "return_rows"],
    [text("Audit memory", "审计内存"), "memory_cost"],
  ];
  return <Dialog
    footer={occurrence && query ? (
      <div className="audit-selection-actions">
        {doctorProfile && onOpenDoctor
          ? <Button onClick={() => onOpenDoctor(doctorProfile)} size="sm" variant="primary">{text("Open diagnosis", "打开诊断")}</Button>
          : <Button disabled={query.profile_fetch.state !== "available" || profileLoading || !onOpenDoctor} onClick={onDiagnose} size="sm" title={query.profile_fetch.state === "available" ? undefined : query.profile_fetch.reason} variant="primary">{profileLoading ? text("Fetching Profile…", "正在拉取 Profile…") : text("Diagnose", "诊断")}</Button>}
        <SqlToggleButton className="btn btn-secondary btn--sm" controls="audit-selected-sql" expanded={sql !== null} loading={sqlLoading} onToggle={onViewSql} />
      </div>
    ) : undefined}
    onOpenChange={(nextOpen) => {
      if (!nextOpen) onClose?.();
    }}
    open={isOpen}
    placement="right"
    restoreFocus={restoreFocus}
    title={query ? (
      <span className="audit-selection-title-row">
        <span>{text("Execution details", "执行详情")}</span>
        <code className="mono" title={query.query_id}>{compactQueryId(query.query_id)}</code>
      </span>
    ) : text("Execution details", "执行详情")}
    width="lg"
  >
    {occurrence && query ? (
      <div className="audit-selection audit-selection-body">
        <p className="audit-selection-context"><span className="status-inline"><span className={statusDotClass(query.status)} />{auditStatusLabel(query.status, text)}</span> · <span className="mono">{query.database ?? "—"}</span> · <span className="mono">{observedAt(query)}</span></p>
        <dl className="kv-grid audit-selection-facts">{facts.map(([label, id]) => <div className="audit-selection-fact-row" key={id}><dt>{label}</dt><dd className="mono" title={metricRawText(metric(occurrence, id))}>{metricValue(metric(occurrence, id), text)}</dd></div>)}</dl>
        <p className="audit-selection-note muted">{query.profile_fetch.state === "available"
          ? text("Reads the existing Profile only; the SQL is never rerun.", "只读取已有 Profile，不会重新执行 SQL。")
          : `${text("No Profile to diagnose", "没有可诊断的 Profile")}: ${profileLabel(query, text)}`}</p>
        {profileError && <div className="notice-banner notice-banner--danger audit-sql-error" role="alert">{profileError}</div>}
        {profile && <p className="audit-profile-receipt mono">{text("Profile fetched", "Profile 已拉取")} · {profile.source_kind} · {shortId(profile.analysis_key)}</p>}
        {sqlError && <div className="notice-banner notice-banner--danger audit-sql-error" role="alert">{sqlError}</div>}
        {sql !== null && <SqlText className="audit-sql" copyLabel={text("Copy SQL", "复制 SQL")} id="audit-selected-sql" label={text("Selected audited SQL", "已选审计 SQL")} value={sql} />}
        <details className="disclosure audit-optional-facts"><summary>{text("Raw audit details", "原始审计详情")}</summary><div className="disclosure-body"><dl className="kv-grid">
          <dt>{text("Query ID", "查询 ID")}</dt><dd><code className="mono">{query.query_id}</code></dd>
          <dt>{text("Pending time", "排队耗时")} · pendingTimeMs</dt><dd className="mono">{metricText(pending)} · {pending?.state ?? "not_returned"}<div className="audit-fact-source muted">{text("Source", "原值")}: {pendingSource}</div></dd>
          <dt>errorCode · {text("source text", "源文本")}</dt><dd className="mono">{errorCodeText}</dd>
        </dl><p className="muted">{text("Facts only: an empty errorCode does not mean success; pending time alone does not prove blocking. sql_null, negative_sentinel and exact zero remain distinct.", "仅呈现事实：空 errorCode 不表示成功；排队耗时本身不证明阻塞。sql_null、negative_sentinel 与真零分别保留。")}</p></div></details>
      </div>
    ) : null}
  </Dialog>;
}

export function closeDrawerForNavigation<T extends { drawerOpen: boolean; restoreFocus: boolean }>(state: T): T;
export function closeDrawerForNavigation<T extends { drawerOpen: boolean; restoreFocus: boolean }>(state: T | null): T | null;
export function closeDrawerForNavigation<T extends { drawerOpen: boolean; restoreFocus: boolean }>(state: T | null): T | null {
  return state ? { ...state, drawerOpen: false, restoreFocus: false } : null;
}

export function openFamilyDrawer(family: AuditFamilyPresentation): FamilySelection {
  return { family, members: null, loading: true, error: null, memberKey: null, showAll: false, drawerOpen: true, restoreFocus: true };
}

export function selectFamilyMember(state: FamilySelection | null, key: string): FamilySelection | null {
  if (!state) return null;
  if (key === state.memberKey) return state.drawerOpen && state.restoreFocus ? state : { ...state, drawerOpen: true, restoreFocus: true };
  return { ...state, memberKey: key, error: null, drawerOpen: true, restoreFocus: true };
}

export function closeFamilyDrawer(
  state: FamilySelection | null,
  reason: "dismiss" | "diagnose" | "baseline" | "compare" | "plan_check" | "show_members" = "dismiss",
  fetched = true,
): FamilySelection | null {
  if (!state) return null;
  if (reason === "baseline") return state;
  if (reason === "diagnose") return fetched ? closeDrawerForNavigation(state) : state;
  if (reason === "compare" || reason === "plan_check") return closeDrawerForNavigation(state);
  if (reason === "show_members") return closeDrawerForNavigation({ ...state, showAll: true });
  return state.showAll ? { ...state, drawerOpen: false, restoreFocus: true } : null;
}

export function selectOccurrenceRow(
  state: OccurrenceSelection,
  key: string,
): { resetEvidence: boolean; selection: OccurrenceSelection } {
  if (state.key === key) {
    return {
      resetEvidence: false,
      selection: state.drawerOpen && state.restoreFocus ? state : { key, drawerOpen: true, restoreFocus: true },
    };
  }
  return { resetEvidence: true, selection: { key, drawerOpen: true, restoreFocus: true } };
}

export function closeOccurrenceDrawer(
  state: OccurrenceSelection,
  reason: "dismiss" | "diagnose" | "open_doctor" = "dismiss",
  fetched = true,
): OccurrenceSelection {
  if (reason === "diagnose") return fetched ? closeDrawerForNavigation(state) : state;
  if (reason === "open_doctor") return closeDrawerForNavigation(state);
  return { key: null, drawerOpen: false, restoreFocus: true };
}

export function familyDrawerMemberRows(
  members: AuditOccurrenceResponse | null,
  selectedMemberKey: string | null,
): { key: string; row: AuditOccurrencePresentation }[] {
  if (!members) return [];
  const all = members.rows.map((row, index) => ({ row, key: memberKey(row, index) }));
  const top = all.slice(0, 4);
  if (!selectedMemberKey || top.some((entry) => entry.key === selectedMemberKey)) return top;
  const selectedEntry = all.slice(4).find((entry) => entry.key === selectedMemberKey);
  return selectedEntry ? [...top, selectedEntry] : top;
}

/** The statement filter is on by default whenever the session proved it can apply it. */
export function defaultAuditScope(now = new Date(), statementFilterAvailable = true): AuditScope {
  const end_seconds = Math.floor(now.getTime() / 1000);
  return { window: { start_seconds: end_seconds - 3600, end_seconds }, state: null, user: null, native_digest: null, catalog: null, database: null, resource_group: null, statement_filter: statementFilterAvailable ? "exclude_doctor_and_non_query" : "all" };
}

export function auditScopeForWindowPreset(scope: AuditScope, preset: AuditWindowPreset, now = new Date()): AuditScope {
  const end_seconds = Math.floor(now.getTime() / 1000);
  return { ...scope, window: { start_seconds: end_seconds - windowPresetSeconds[preset], end_seconds } };
}

export function auditWindowPresetForScope(scope: AuditScope): AuditWindowPreset | null {
  const seconds = scope.window.end_seconds - scope.window.start_seconds;
  return windowPresets.find((preset) => windowPresetSeconds[preset] === seconds) ?? null;
}

export function auditScopeForFamily(scope: AuditScope, family: AuditFamilyPresentation): AuditScope | null {
  if (family.membership.state !== "exact_native_digest") return null;
  return { ...scope, native_digest: family.membership.native_digest, catalog: family.membership.context.catalog, database: family.membership.context.database };
}

export function auditParentScopeAfterFamilyEdits(parentScope: AuditScope, drillScope: AuditScope, currentScope: AuditScope): AuditScope {
  return {
    ...parentScope,
    window: currentScope.window,
    state: currentScope.state,
    user: currentScope.user,
    native_digest: parentScope.native_digest,
    catalog: currentScope.catalog === drillScope.catalog ? parentScope.catalog : currentScope.catalog,
    database: currentScope.database === drillScope.database ? parentScope.database : currentScope.database,
    resource_group: currentScope.resource_group,
    statement_filter: currentScope.statement_filter,
  };
}

export function auditFamilyMemberBrowseState(activeFamily: AuditFamilyPresentation, parentScope: AuditScope, detail: AuditFamilyDetailResponse): {
  activeFamily: AuditFamilyPresentation;
  drillScope: AuditScope;
  mode: "family";
  parentScope: AuditScope;
  requestScope: AuditScope;
} {
  return { activeFamily, parentScope, drillScope: detail.scope, requestScope: detail.scope, mode: "family" };
}

export function auditModeSwitchScopes({
  draftScope,
  familyDrillScope,
  familyParentScope,
  familyScope,
  mode,
  occurrenceScope,
}: {
  draftScope: AuditScope;
  familyDrillScope: AuditScope | null;
  familyParentScope: AuditScope | null;
  familyScope: AuditScope | null;
  mode: AuditMode;
  occurrenceScope: AuditScope | null;
}): { draftScope: AuditScope; requestScope: AuditScope } {
  if (mode === "family") {
    const restoredDraft = familyParentScope && familyDrillScope
      ? auditParentScopeAfterFamilyEdits(familyParentScope, familyDrillScope, draftScope)
      : clearFamilyDrillScope(draftScope);
    return {
      draftScope: restoredDraft,
      requestScope: familyParentScope ?? clearFamilyDrillScope(occurrenceScope ?? restoredDraft),
    };
  }
  return {
    draftScope,
    requestScope: (mode === "families" ? familyScope : occurrenceScope) ?? draftScope,
  };
}

function clearFamilyDrillScope(scope: AuditScope): AuditScope {
  return { ...scope, native_digest: null, catalog: null, database: null };
}

export function occurrenceRowKey(row: AuditOccurrencePresentation, index: number): string {
  return `${row.query.query_id}:${row.query.source_refs[0] ?? `audit-row:${index}`}`;
}

export function familyRowKey(row: AuditFamilyPresentation, index: number): string {
  return row.membership.state === "exact_native_digest" ? row.membership.family_key : `ungrouped:${row.membership.reason}:${index}`;
}

export function visibleSql(sql: SqlState | null, request: AuditSqlReadRequest): string | null {
  if (!sql || sql.binding.connection_id !== request.connection_id || sql.binding.discovery_generation !== request.discovery_generation || sql.binding.query_id !== request.query_id || scopeKey(sql.binding.scope) !== scopeKey(request.scope)) return null;
  return sql.result.query_id === request.query_id && sql.result.discovery_generation === request.discovery_generation ? sql.result.sql : null;
}

export function auditFamilyDetailRequest(connectionId: string, discoveryGeneration: number, parentScope: AuditScope, family: AuditFamilyPresentation): AuditFamilyDetailReadRequest | null {
  if (family.membership.state !== "exact_native_digest") return null;
  return { connection_id: connectionId, discovery_generation: discoveryGeneration, scope: parentScope, family_key: family.membership.family_key };
}

export function isAuditFamilyDetailBound(detail: AuditFamilyDetailResponse, request: AuditFamilyDetailReadRequest, family: AuditFamilyPresentation): boolean {
  const expectedScope = auditScopeForFamily(request.scope, family);
  return detail.connection_id === request.connection_id
    && detail.discovery_generation > request.discovery_generation
    && detail.family.state === "exact_native_digest"
    && detail.family.family_key === request.family_key
    && expectedScope !== null
    && scopeKey(detail.scope) === scopeKey(expectedScope);
}

export function familyDetailRepresentativeRows(detail: AuditFamilyDetailResponse): AuditOccurrencePresentation[] {
  return [detail.representatives.recent, detail.representatives.slowest, detail.representatives.profile_known]
    .flatMap((representative) => representative.state === "selected" ? [representative.occurrence] : []);
}

export function auditFamilyDetailSqlRequest(connectionId: string, familyKey: string, detail: AuditFamilyDetailResponse, occurrence: AuditOccurrencePresentation): AuditSqlReadRequest | null {
  if (detail.connection_id !== connectionId || detail.family.state !== "exact_native_digest" || detail.family.family_key !== familyKey || !familyDetailRepresentativeRows(detail).includes(occurrence)) return null;
  return { connection_id: connectionId, discovery_generation: detail.discovery_generation, scope: detail.scope, query_id: occurrence.query.query_id };
}

export function familyMemberSqlRequest(connectionId: string, members: AuditOccurrenceResponse | null, member: AuditOccurrencePresentation | null): AuditSqlReadRequest | null {
  if (!members || !member || !members.rows.includes(member)) return null;
  return { connection_id: connectionId, discovery_generation: members.discovery_generation, scope: members.scope, query_id: member.query.query_id };
}

function auditSqlRequestForOccurrence(connectionId: string, discovery: AuditActiveAtResponse | AuditOccurrenceResponse | null, detail: FamilyDetailState | null, occurrence: AuditOccurrencePresentation): AuditSqlReadRequest | null {
  if (detail) return auditFamilyDetailSqlRequest(connectionId, detail.familyKey, detail.response, occurrence);
  if (!discovery) return null;
  return { connection_id: connectionId, discovery_generation: discovery.discovery_generation, scope: discovery.scope, query_id: occurrence.query.query_id };
}

export function auditFamilyDetailDiscoveryGeneration(detail: FamilyDetailState, occurrence: AuditOccurrencePresentation, connectionId: string): number | null {
  return detail.response.connection_id === connectionId && familyDetailRepresentativeRows(detail.response).includes(occurrence)
    ? detail.response.discovery_generation
    : null;
}

export function auditReportForSelectedOccurrence(query: ConnectedQueryRef | null, discoveryGeneration: number | null, result: ProfileFetchResponse | null): AnalysisSnapshotReport | null {
  return auditProfileForSelectedOccurrence(query, discoveryGeneration, result)?.outcome.report ?? null;
}

export function auditProfileForSelectedOccurrence(query: ConnectedQueryRef | null, discoveryGeneration: number | null, result: ProfileFetchResponse | null): ProfileFetchResponse | null {
  if (!query || discoveryGeneration === null || result?.state !== "fetched" || result.query.connection_id !== query.connection_id || result.query.query_id !== query.query_id || result.query.discovery_generation !== discoveryGeneration) return null;
  return result;
}

export function isCurrentAuditRequest(activeGeneration: number, requestGeneration: number, signal: AbortSignal): boolean {
  return activeGeneration === requestGeneration && !signal.aborted;
}

export function isAuditRequestCurrentForConnection(activeGeneration: number, requestGeneration: number, signal: AbortSignal, requestConnectionId: string | null, currentConnectionId: string | null): boolean {
  return requestConnectionId !== null
    && requestConnectionId === currentConnectionId
    && isCurrentAuditRequest(activeGeneration, requestGeneration, signal);
}

export function auditResponseForConnection<T extends ConnectionScopedResponse>(response: T | null, connectionId: string | null): T | null {
  return response !== null && connectionId !== null && response.connection_id === connectionId ? response : null;
}

export function isConnectionExpiredFailure(cause: unknown): boolean {
  if (typeof cause === "object" && cause !== null) {
    const failure = cause as { code?: unknown; message_id?: unknown };
    // SessionManager discards RemoteTimeout in with_supported_session before returning.
    // Keep the original timeout label; only route the now-invalid session to recovery.
    return failure.code === "remote_timeout" || failure.code === "connection_expired" || failure.message_id === "connection_expired" || failure.message_id === "connection_session_expired";
  }
  return cause instanceof Error && (cause.message === "connection_expired" || cause.message === "connection_session_expired");
}

/** Why the session ended, so the header does not call a timed-out read an idle expiry. */
export function sessionEndReason(cause: unknown): "remote_timeout" | undefined {
  return typeof cause === "object" && cause !== null && (cause as { code?: unknown }).code === "remote_timeout" ? "remote_timeout" : undefined;
}

/** A missing Profile is the normal case (the query ran without enable_profile), so say so plainly. */
export function profileFetchFailureText(cause: unknown, text: LocalizedText): string {
  return auditErrorLabel(cause) === "not_found_or_expired_or_not_profiled"
    ? text("This query left no Profile: it ran without enable_profile, or its Profile has expired. Pick another query, or run it from the SQL workspace to capture one.", "这条查询没有留下 Profile：执行时未开启 enable_profile，或 Profile 已过期。请换一条查询，或在 SQL 工作区执行一次以采集 Profile。")
    : text("Profile fetch failed. The connection may be unavailable; refresh the list and try again.", "Profile 拉取失败，可能是连接异常；请刷新列表后重试。");
}

export function auditErrorLabel(cause: unknown): string {
  if (typeof cause === "object" && cause !== null && "message_id" in cause) {
    const failure = cause as { details?: { limitation?: unknown }; message_id?: unknown };
    return typeof failure.details?.limitation === "string" ? failure.details.limitation : String(failure.message_id);
  }
  return "audit_request_failed";
}

function metric(row: AuditOccurrencePresentation, id: AuditMetricId): AuditMetricFact | null {
  return row.metrics.find((item) => item.metric_id === id) ?? null;
}

function aggregateRawText(row: AuditFamilyPresentation, id: AuditMetricId, aggregation: "sum" | "max"): string {
  return metricRawText(row.aggregates.find((item) => item.metric.metric_id === id && item.aggregation === aggregation)?.metric ?? null);
}

function aggregateMetric(row: AuditFamilyPresentation, id: AuditMetricId, aggregation?: "sum" | "max"): AuditMetricFact | null {
  return row.aggregates.find((item) => item.metric.metric_id === id && (!aggregation || item.aggregation === aggregation))?.metric ?? null;
}

/** One line of the execution's own audited stmt prefix; every non-text state is a stamp, never an empty cell. */
export function OccurrenceStatementCell({ sample, text }: { sample: AuditStatementSample; text: LocalizedText }): JSX.Element {
  if (sample.state === "text" && sample.value.trim() !== "") {
    const shown = `${sample.value}${sample.truncated ? "…" : ""}`;
    return <td className="audit-cell-sql mono" title={shown}>{shown}</td>;
  }
  const [label, note]: [string, string] = sample.state === "text"
    ? [text("Empty stmt", "stmt 为空串"), text("The audited stmt is an empty string.", "审计 stmt 是空字符串。")]
    : sample.state === "sql_null"
    ? [text("stmt NULL", "stmt 为 NULL"), text("The audited stmt is SQL NULL.", "审计 stmt 为 SQL NULL。")]
    : sample.reason === "source_column_absent"
    ? [text("No stmt column", "无 stmt 列"), text("The audit table has no stmt column.", "审计表没有 stmt 列。")]
    : sample.reason === "conflicting_source_rows"
    ? [text("Rows disagree", "重复行不一致"), text("Duplicate audit rows of this execution disagree on stmt.", "该执行的重复审计行 stmt 不一致。")]
    : [text("Not read", "未读取"), text("This response did not read the stmt.", "本次响应未读取 stmt。")];
  return <td className="audit-cell-sql" title={note}><span className="state-stamp audit-value-tag">{label}</span></td>;
}

function metricValue(value: AuditMetricFact | null, text?: LocalizedText): ReactNode {
  return <AuditMetricValue text={text} value={value} />;
}

/** Same text as before split into a mono digest and a muted context; textContent is unchanged. */
function FamilyLabel({ family }: { family: AuditFamilyPresentation }): JSX.Element {
  const { text } = useLocale();
  if (family.membership.state !== "exact_native_digest") return <span className="audit-family-ungrouped" title={family.membership.reason}>{text("Not grouped", "未归组")} · {ungroupedReasonText(family.membership.reason, text)}</span>;
  const { context, native_digest } = family.membership;
  return <span className="audit-family-label"><span className="audit-family-digest mono">{shortId(native_digest)}</span><span className="audit-family-context"> · {context.catalog ?? "—"} / {context.database ?? "—"} · {context.cluster ?? "—"} / {context.engine_version ?? "—"}</span></span>;
}

/** Status dot modifier for server status strings; unknown future states stay hollow/neutral. */
function statusDotClass(status: string): string {
  if (status === "finished") return "status-dot status-dot--ok";
  if (status === "error") return "status-dot status-dot--critical";
  if (status === "running") return "status-dot status-dot--warn";
  return "status-dot status-dot--hollow";
}

function coverageMetricLine(coverage: AuditOccurrenceResponse["coverage"] | undefined, text: LocalizedText): string {
  if (!coverage) return text("Coverage not loaded", "覆盖未加载");
  return coverage.metric_inputs.map((item) => `${item.metric_id}: NULL ${item.sql_null_count}, ${text("nonnegative", "非负")} ${item.nonnegative_count}, ${text("negative", "负值")} ${item.negative_count}`).join(" · ");
}

function observedAt(query: ConnectedQueryRef): string {
  return query.observed_at_utc ?? query.observed_at_local ?? "—";
}

const AUDIT_STATUS_LABELS: Record<string, readonly [string, string]> = {
  finished: ["Finished", "已完成"],
  error: ["Failed", "执行失败"],
  unknown: ["Status unknown", "状态未知"],
  running: ["Running", "执行中"],
  cancelled: ["Cancelled", "已取消"],
};

export function auditStatusLabel(status: string, text: LocalizedText): string {
  return lookupEnumLabel(AUDIT_STATUS_LABELS, status, text);
}

const PROFILE_UNAVAILABLE_LABELS: Record<string, readonly [string, string]> = {
  query_not_finished: ["Query not finished", "查询尚未结束"],
  profile_unavailable: ["Profile unavailable", "暂无 Profile"],
  query_id_unavailable: ["Query ID unavailable", "缺少查询 ID"],
  unknown: ["Availability unknown", "可用性未知"],
};

function profileLabel(query: ConnectedQueryRef, text: LocalizedText): string {
  // Audit rows cannot prove a Profile was kept; "available" means a fetch may be tried.
  if (query.profile_fetch.state === "available") return text("Can try", "可尝试拉取");
  const known = PROFILE_UNAVAILABLE_LABELS[query.profile_fetch.reason];
  return known ? text(...known) : lookupEnumLabel(PROFILE_UNAVAILABLE_LABELS, query.profile_fetch.reason, text);
}

function formatUtcWindow(scope: AuditScope): string {
  return `${new Date(scope.window.start_seconds * 1000).toISOString()} → ${new Date(scope.window.end_seconds * 1000).toISOString()}`;
}

function scopeKey(scope: AuditScope): string {
  return JSON.stringify(scope);
}

function emptyToNull(value: string): string | null {
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
}

export function auditTerminalStateFromControl(value: string): AuditScope["state"] {
  return value === "finished" || value === "error" ? value : null;
}

const USER_FILTER_CAPABILITY_LABELS: Record<Exclude<ConnectionCapabilityState, "available">, readonly [string, string]> = {
  unavailable: ["User filter unavailable: the AuditLoader user column is unavailable.", "用户筛选不可用：AuditLoader 用户列不可用。"],
  forbidden: ["User filter forbidden for this connection.", "该连接禁止用户筛选。"],
  unknown: ["User filter availability is unknown; source column availability is unknown.", "用户筛选能力未知；源列可用性未知。"],
};

export function auditUserFilterNote(capability: ConnectionCapabilityState, text: LocalizedText): string {
  return capability === "available" ? "" : lookupEnumLabel(USER_FILTER_CAPABILITY_LABELS, capability, text);
}

const STATEMENT_FILTER_CAPABILITY_LABELS: Record<Exclude<ConnectionCapabilityState, "available">, readonly [string, string]> = {
  unavailable: ["Unavailable: the AuditLoader table lacks a TINYINT isQuery column or the stmt column, so all statements are shown.", "不可用：AuditLoader 表缺少 TINYINT 类型的 isQuery 列或 stmt 列，因此显示全部语句。"],
  forbidden: ["Statement filter forbidden for this connection; all statements are shown.", "该连接禁止语句筛选，显示全部语句。"],
  unknown: ["Statement filter availability is unknown; all statements are shown.", "语句筛选能力未知，显示全部语句。"],
};

export function auditStatementFilterNote(capability: ConnectionCapabilityState, text: LocalizedText): string {
  return capability === "available" ? "" : lookupEnumLabel(STATEMENT_FILTER_CAPABILITY_LABELS, capability, text);
}

const WINDOW_PRESET_LABELS: Record<AuditWindowPreset, readonly [string, string]> = {
  "1h": ["Last 1 h", "最近 1 小时"],
  "12h": ["Last 12 h", "最近 12 小时"],
  "1d": ["Last 1 day", "最近 1 天"],
  "3d": ["Last 3 days", "最近 3 天"],
  "7d": ["Last 7 days", "最近 7 天"],
  "30d": ["Last 1 month (30 days)", "最近 1 月（30 天）"],
};

export function windowPresetLabel(preset: AuditWindowPreset, text: LocalizedText): string {
  return lookupEnumLabel(WINDOW_PRESET_LABELS, preset, text);
}

function shortId(value: string): string {
  return value.length > 18 ? `${value.slice(0, 16)}…` : value;
}

const AUDIT_SORT_LABELS: Record<AuditOccurrenceSort | AuditFamilySort, readonly [string, string]> = {
  error_proportion_desc: ["error proportion descending", "错误占比降序"],
  query_time_desc: ["Longest duration first", "单次耗时最高"],
  scan_bytes_desc: ["Most scanned bytes first", "扫描数据量最大"],
  scan_rows_desc: ["Most scanned rows first", "扫描行数最多"],
  cpu_cost_desc: ["Highest CPU cost first", "CPU 消耗最高"],
  memory_cost_desc: ["Highest memory first", "内存占用最高"],
  newest_first: ["newest first", "最新优先"],
  cumulative_query_time_desc: ["Highest cumulative duration first", "累计耗时最高"],
  cumulative_scan_bytes_desc: ["Most cumulative scanned bytes first", "累计扫描量最大"],
  cumulative_cpu_cost_desc: ["Highest cumulative CPU first", "累计 CPU 消耗最高"],
  execution_count_desc: ["Most frequent first", "执行最频繁"],
  error_count_desc: ["error count descending", "错误次数降序"],
  max_query_time_desc: ["Highest peak duration first", "最慢一次耗时最高"],
  max_memory_cost_desc: ["Highest peak memory first", "峰值内存最高"],
};

export function sortLabel(sort: AuditOccurrenceSort | AuditFamilySort, text: LocalizedText): string {
  return lookupEnumLabel(AUDIT_SORT_LABELS, sort, text);
}

/** AuditUngroupedReason is `<field>_missing` or `<field>_invalid`. */
function ungroupedReasonText(reason: string, text: LocalizedText): string {
  const field = reason.replace(/_(missing|invalid)$/, "");
  const fieldLabel: Record<string, [string, string]> = {
    native_digest: ["digest", "digest"],
    cluster: ["cluster", "集群"],
    engine_version: ["version", "版本"],
    catalog: ["catalog", "catalog"],
    database: ["database", "数据库"],
  };
  const label = fieldLabel[field];
  if (!label) return reason;
  return reason.endsWith("_missing") ? text(`no ${label[0]}`, `缺少${label[1]}`) : text(`invalid ${label[0]}`, `${label[1]}无效`);
}

/** Where the selected member stands against the session's baseline (design board 01 aside). */
function baselineState(baselineQueryId: string | null, members: AuditOccurrenceResponse | null, selected: AuditOccurrencePresentation | null): BaselineState {
  if (!baselineQueryId) return "none";
  if (selected?.query.query_id === baselineQueryId) return "selected";
  return members?.rows.some((row) => row.query.query_id === baselineQueryId) ? "comparable" : "elsewhere";
}
