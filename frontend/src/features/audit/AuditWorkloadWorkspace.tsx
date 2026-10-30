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
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { AuditFamilyDetail } from "./AuditFamilyDetail";
import { AuditBoundedSourceNote, AuditDigestGuidance, auditSegmentAt } from "./AuditOverviewCharts";
import { AuditFamilyAside, AuditFamilyList, AuditFingerprintFamilyList, AuditKpiStrip, memberKey, type BaselineState } from "./AuditOverviewPanels";
import { AuditSimilarAside, AuditSimilarSqlList } from "./AuditSimilarSql";
import { AuditPerformanceOverview } from "./AuditPerformanceOverview";
import { AuditHistoryLoad } from "./AuditHistoryLoad";
import { LoadSummaryCard } from "../loads/LoadSummaryCard";
import type { LoadFocus } from "../loads/LoadWorkspace";
import { countText } from "../../lib/duration-display";
import { compactQueryId } from "../../lib/query-id";
import { SqlText, SqlToggleButton } from "../shell/SqlText";
import { auditClockText, auditDigestUnavailable, auditMillisecondsText, errorProportionText, metricRawText, metricText, metricTone, positiveCount } from "./audit-display";
export { metricText } from "./audit-display";
import "./audit-workload.css";

type AuditApi = Pick<DoctorApi, "compareConnectedProfiles" | "fetchConnectedProfile" | "readAuditSql" | "readAuditFamilyDetail" | "readAuditOverview" | "readAuditActiveAt" | "readAuditOccurrences"> & Partial<Pick<DoctorApi, "readAuditFingerprintFamilies" | "readAuditSimilarSql" | "readAuditPerformance" | "readHistoryLoad" | "readLoadOverview">>;
/** Unofficial fingerprint groups of the digest-missing executions in the admitted families window. */
type FingerprintState = { response: AuditFingerprintFamilyResponse | null; loading: boolean; error: string | null };
/** The family selected in the repeated-SQL table and its member ranking (design board 01 aside). */
type FamilySelection = { family: AuditFamilyPresentation; members: AuditOccurrenceResponse | null; loading: boolean; error: string | null; memberKey: string | null; showAll: boolean };
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
  const [overviewShown, setOverviewShown] = useState(true);
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
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
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
    setSelectedKey(null);
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
    setScope(defaultAuditScope(new Date(), statementFilterAvailable));
    setWindowPreset("1h");
    setMode("families");
    setFamilySelection(null);
    setFetchedQueryIds(new Set());
    setOccurrences(null);
    setFamilies(null);
    setFingerprints(null);
    setOverview(null);
    setActiveAt(null);
    setActiveFamily(null);
    setFamilyDetail(null);
    setResponseAdmitted(false);
    setFamilyParentScope(null);
    setFamilyDrillScope(null);
    setSelectedKey(null);
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
    setSelectedKey(null);
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
    invalidateRequests();
    clearTransientEvidence();
    setSelectedKey(rowKey);
    setError(null);
  }

  function closeOccurrence(): void {
    invalidateRequests();
    clearTransientEvidence();
    setSelectedKey(null);
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
    setSelectedKey(null);
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
    const stale = familySelection?.members !== null && familySelection?.members !== undefined;
    const base = stale ? await loadOverview(currentOverview.scope, mode === "occurrences" ? "occurrences" : "families") : currentOverview;
    if (!base) return;
    const { controller, requestGeneration } = beginRequest();
    setLoading(true);
    setSelectedKey(null);
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
    if (nextKey !== null && nextKey !== selectedKey) {
      invalidateRequests();
      clearTransientEvidence();
      setSelectedKey(nextKey);
    }
  }

  /** Selecting a family reads its member ranking (one occurrence read scoped to the exact family). */
  async function selectFamily(family: AuditFamilyPresentation): Promise<void> {
    if (!session || loading || family.membership.state !== "exact_native_digest") return;
    const memberScope = auditScopeForFamily(currentFamilies?.scope ?? scope, family);
    if (!memberScope) return;
    const { controller, requestGeneration } = beginRequest();
    setSelectedKey(null);
    setError(null);
    clearTransientEvidence();
    setActiveAt(null);
    setFamilySelection({ family, members: null, loading: true, error: null, memberKey: null, showAll: false });
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
    setFamilySelection(null);
  }

  const selectedMember = familySelection?.members?.rows.find((row, index) => memberKey(row, index) === familySelection.memberKey) ?? null;

  /** Profile fetch for the selected member, bound to the member read that returned it. */
  async function fetchMemberProfile(after: "diagnose" | "baseline"): Promise<void> {
    const members = familySelection?.members;
    if (!session || !members || !selectedMember || selectedMember.query.profile_fetch.state !== "available" || profileLoading) return;
    const { controller, requestGeneration } = beginRequest();
    setProfileError(null);
    setProfileLoading(true);
    try {
      const result = await api.fetchConnectedProfile({ connection_id: session.connection_id, query_id: selectedMember.query.query_id, discovery_generation: members.discovery_generation, kind: "baseline" }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
      if (result.state === "fetched") setFetchedQueryIds((current) => new Set(current).add(result.query.query_id));
      if (after === "diagnose") onOpenDoctor?.(result);
      else onSetBaseline?.(result);
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
    setProfileLoading(true);
    try {
      const result = await api.compareConnectedProfiles({ connection_id: session.connection_id, baseline_query_id: baselineQueryId, candidate_query_id: selectedMember.query.query_id, discovery_generation: members.discovery_generation }, controller.signal);
      if (!requestIsCurrent(requestGeneration, controller)) return;
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
  const currentSimilar = similar && (!similar.response || similar.response.connection_id === sessionConnectionId) ? similar : null;
  const selectedCluster = currentSimilar?.response?.clusters.find((cluster) => cluster.cluster_key === currentSimilar.selectedKey) ?? null;
  const title = {
    active_at: text("In flight at the selected time", "所选时刻的在途查询"),
    family_detail: text("Query family detail", "查询族详情"),
    members: text("Family members · by duration", "查询族成员 · 按耗时"),
    families: text("Repeated SQL · frequency and cumulative cost", "同类 SQL · 频次与累计消耗"),
    similar: text("Similar SQL · templates merged by structure", "相似 SQL · 按结构归并的模板"),
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
  const viewSwitch = <div aria-label={text("View", "视图")} className="audit-view-switch" role="group">
    {readPerformance && <button aria-pressed={overviewView} disabled={loading} onClick={() => { setOverviewShown(true); if (mode === "similar") chooseMode("families"); }} type="button">{text("Overview", "概览")}</button>}
    {(!digestUnavailable || mode === "families") && <button aria-pressed={view === "families" && !overviewView} disabled={loading} onClick={() => { setOverviewShown(false); if (!(overviewView && mode === "families")) chooseMode("families"); }} type="button">{text("Repeated SQL", "同类 SQL")}</button>}
    {api.readAuditSimilarSql && <button aria-pressed={view === "similar"} disabled={loading} onClick={() => { setOverviewShown(false); chooseSimilar(); }} type="button">{text("Similar SQL", "相似 SQL")}</button>}
    <button aria-pressed={view === "occurrences" && !overviewView} disabled={loading} onClick={() => { setOverviewShown(false); if (!(overviewView && mode === "occurrences")) chooseMode("occurrences"); }} type="button">{text("Individual executions", "单次执行")}</button>
  </div>;
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
  };
  const filterDimension = (dimension: AuditPerformanceDimensionId, value: string): void => {
    const base = currentOverview?.scope ?? scope;
    const field = dimension === "resource_group" ? "resource_group" : dimension;
    const nextScope: AuditScope = { ...base, [field]: value };
    setScope(nextScope);
    loadForMode(nextScope);
  };

  return <section className="audit-workload audit-workload-overview" aria-busy={loading} aria-labelledby="audit-board-title">
    <form className="audit-filter-bar" aria-label={text("Filters", "筛选")} onSubmit={(event) => { event.preventDefault(); refresh(); }}>
      <label>{text("Time range (UTC)", "时间范围（UTC）")}<select aria-label={text("Audit window", "审计窗口")} className="audit-filter-window" onChange={(event) => setWindowPreset(event.target.value as AuditWindowPreset)} title={response ? formatUtcWindow(response.scope) : undefined} value={windowPreset}>{windowPresets.map((preset) => <option key={preset} value={preset}>{windowPresetLabel(preset, text)}</option>)}</select></label>
      <label>{text("Database", "数据库")}<input aria-label={text("Database filter", "数据库筛选")} className="audit-filter-database" onChange={(event) => setScope((current) => ({ ...current, database: emptyToNull(event.target.value) }))} placeholder={text("All databases", "全部数据库")} value={scope.database ?? ""} /></label>
      <label>{text("User (exact)", "用户（精确匹配）")}<input aria-describedby={userFilterAvailable ? undefined : "audit-user-filter-capability"} aria-label={text("Exact user filter", "精确用户筛选")} className="audit-filter-user" disabled={!userFilterAvailable} onChange={(event) => setScope((current) => ({ ...current, user: emptyToNull(event.target.value) }))} placeholder={text("All users", "全部用户")} title={userFilterAvailable ? undefined : auditUserFilterNote(userFilterCapability, text)} value={scope.user ?? ""} /></label>
      <label>{text("State", "状态")}<select aria-label={text("Audit source state", "审计源状态")} className="audit-filter-state" onChange={(event) => setScope((current) => ({ ...current, state: auditTerminalStateFromControl(event.target.value) }))} value={scope.state ?? ""}><option value="">{text("All terminal states", "全部终态")}</option><option value="finished">{text("Finished", "已完成")}</option><option value="error">{text("Failed", "执行失败")}</option></select></label>
      <label>{text("Top limit", "Top 上限")}<select aria-label={text("Top limit", "Top 上限")} className="audit-filter-limit" onChange={(event) => setLimit(Number(event.target.value) as (typeof limits)[number])} value={limit}>{limits.map((value) => <option key={value} value={value}>{text(`${value} rows`, `${value} 条`)}</option>)}</select></label>
      {mode === "similar" && <fieldset className="audit-similar-dimensions"><legend>{text("Merge basis", "归并依据")}</legend>{([["tables", text("Tables", "表集合")], ["join", text("Join shape", "Join 形态")], ["predicates", text("Predicate columns", "谓词列集合")]] as const).map(([key, label]) => <label key={key}><input checked={similarDimensions[key]} disabled={similarDimensions[key] && Object.values(similarDimensions).filter(Boolean).length === 1} onChange={(event) => setSimilarDimensions((current) => ({ ...current, [key]: event.target.checked }))} type="checkbox" />{label}</label>)}</fieldset>}
      <button aria-controls={moreFiltersId} aria-expanded={moreFilters} className="audit-more-filters" onClick={() => setMoreFilters((open) => !open)} type="button">{text("More filters", "更多筛选")}</button>
      <div className="audit-filter-actions">
        <span>{text("Manual refresh · up to the last 30 days", "手动刷新 · 最多最近 30 天")}</span>
        <button className="button secondary" disabled={loading} onClick={refresh} type="button">{loading ? text("Refreshing…", "刷新中…") : text("Refresh", "刷新")}</button>
        <button className="button dark" disabled={loading} type="submit">{text("Apply filters", "应用筛选")}</button>
      </div>
      <div className="audit-filter-more" hidden={!moreFilters} id={moreFiltersId}>
        <div className="audit-statement-filter">
          <label><input aria-describedby="audit-statement-filter-note" aria-label={text("Hide this tool's queries and non-query statements", "隐藏本工具查询与非查询语句")} checked={scope.statement_filter === "exclude_doctor_and_non_query"} disabled={!statementFilterAvailable} onChange={(event) => setScope((current) => ({ ...current, statement_filter: event.target.checked ? "exclude_doctor_and_non_query" : "all" }))} type="checkbox" />{text("Hide this tool's queries and non-query statements", "隐藏本工具查询与非查询语句")}</label>
          <span className="audit-filter-note" id="audit-statement-filter-note">{statementFilterAvailable
            ? text("Keeps rows with isQuery = 1 only. SET/SHOW/DDL/INSERT and other non-query statements, and statements sent by this tool, are hidden; the hidden counts are shown in the table subtitle.", "仅保留 isQuery = 1 的记录；SET/SHOW/DDL/INSERT 等非查询语句及本工具自身发出的语句会被隐藏，隐藏条数见表格副标题。")
            : auditStatementFilterNote(statementFilterCapability, text)}</span>
        </div>
        <label>{text("Catalog", "Catalog")}<input aria-label={text("Catalog filter", "Catalog 筛选")} onChange={(event) => setScope((current) => ({ ...current, catalog: emptyToNull(event.target.value) }))} value={scope.catalog ?? ""} /></label>
        <label>{text("Resource group", "资源组")}<input aria-label={text("Resource group filter", "资源组筛选")} onChange={(event) => setScope((current) => ({ ...current, resource_group: emptyToNull(event.target.value) }))} value={scope.resource_group ?? ""} /></label>
        <label>{text("Rank individual executions", "单次执行：优先看什么")}<select aria-label={text("Rank individual executions", "单次执行：优先看什么")} onChange={(event) => setOccurrenceSort(event.target.value as AuditOccurrenceSort)} value={occurrenceSort}>{occurrenceSorts.map((sort) => <option key={sort} value={sort}>{sortLabel(sort, text)}</option>)}</select></label>
        <label>{text("Rank repeated SQL", "同类 SQL：优先看什么")}<select aria-label={text("Rank repeated SQL", "同类 SQL：优先看什么")} onChange={(event) => setFamilySort(event.target.value as AuditFamilySort)} value={familySort}>{familySorts.map((sort) => <option key={sort} value={sort}>{sortLabel(sort, text)}</option>)}</select></label>
        {!userFilterAvailable && <span className="audit-filter-note" id="audit-user-filter-capability">{auditUserFilterNote(userFilterCapability, text)}</span>}
        {windowPreset === "30d" && <p className="audit-filter-note" role="note">{text("A 30-day read may cover every retained partition. Narrow the window or filters if the query exceeds its budget.", "30 天查询可能覆盖全部保留分区；若查询超出预算，请缩小时间窗或筛选范围。")}</p>}
      </div>
    </form>

    {!familyDetailShown && view !== "similar" && <AuditWindowEvidence compact response={response} scope={scope} windowPreset={windowPreset} text={text} />}
    {error && <p className="audit-error" role="alert">{error === "not_found_or_expired_or_not_profiled" ? text("No Profile is available for this query. It may not have been collected or may have expired.", "这条查询没有可用的 Profile：可能未采集，或已过期。请选择另一条查询。") : error}</p>}
    {currentOverview && !familyDetailShown && view !== "similar" && !overviewView && <AuditKpiStrip concurrency={currentOverview.concurrency} coverage={currentOverview.coverage} disabled={loading} onSelectTime={(at) => void selectTime(at)} text={text} />}

    {overviewView && <div className="audit-overview-head">
      <div><h1 id="audit-board-title">{text("Overview · audit performance", "概览 · 审计性能")}</h1><p data-audit-statement-filter={statementFilter?.state} title={hiddenTitle}>{filterNote}</p></div>
      <div className="audit-overview-head-controls">
        {readHistoryLoad && <div aria-label={text("Overview range", "概览范围")} className="audit-overview-range" role="group">
          <button aria-pressed={overviewRange === "window"} onClick={() => setOverviewRange("window")} type="button">{text("Selected window · audit table", "所选时间窗 · 审计表")}</button>
          <button aria-pressed={overviewRange === "history"} onClick={() => setOverviewRange("history")} type="button">{text("30 days · history", "30 天 · 历史库")}</button>
        </div>}
        {viewSwitch}
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
        onOpenFamilies={() => { setOverviewShown(false); if (mode !== "families") chooseMode("families"); }}
        loadCard={readLoadOverview && onOpenLoads ? <LoadSummaryCard api={{ readLoadOverview: readLoadOverview.bind(api) }} connectionId={session.connection_id} describeFailure={describePerformanceFailure} onOpen={onOpenLoads} text={text} /> : undefined}
        scope={currentOverview.scope}
        text={text}
      />
      : !error && <p className="audit-empty" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>)}
    {!overviewView && <div className="audit-board-layout">
      <section aria-labelledby="audit-board-title" className="audit-board-main">
        <div className="audit-board-main-head">
          <div><h1 id="audit-board-title">{title}</h1><p data-audit-statement-filter={statementFilter?.state} title={hiddenTitle}>{subtitle}</p></div>
          {view === "families" || view === "occurrences" || view === "similar"
            ? viewSwitch
            : <button className="button secondary" disabled={loading} onClick={() => {
              if (view === "members") setFamilySelection((current) => current ? { ...current, showAll: false } : current);
              else if (view === "family_detail") returnToParent();
              else loadForMode(currentOverview?.scope ?? scope);
            }} type="button">{text("Back to the list", "返回列表")}</button>}
        </div>
        {currentActiveAt && <AuditActiveAtEvidence chartCount={currentOverview?.concurrency.series.state === "exact" ? auditSegmentAt(currentOverview.concurrency.series.points, currentActiveAt.at_milliseconds)?.active_count : undefined} response={currentActiveAt} text={text} />}
        {view === "family_detail" && currentFamilyDetail
          ? <AuditFamilyDetail detail={currentFamilyDetail.response} onBrowseMembers={browseFamilyMembers} onFetchProfile={(occurrence) => void fetchProfile(occurrence)} onReturn={returnToParent} onSelectRepresentative={selectFamilyRepresentative} onViewSql={(occurrence) => void viewSql(occurrence)} shownSqlQueryId={shownSql !== null ? selected?.query.query_id ?? null : null} parentWindow={formatUtcWindow(currentFamilyDetail.parentScope)} profileLoading={profileLoading} sqlLoading={sqlLoading} text={text} />
          : view === "members" && familySelection?.members
          ? <><AuditOccurrenceTable disabled={loading} occurrences={familySelection.members} onSelect={(key) => setFamilySelection((current) => current ? { ...current, memberKey: key } : current)} selectedKey={familySelection.memberKey} text={text} /><p className="audit-members-detail"><button className="button secondary" disabled={loading} onClick={() => openFamily(familySelection.family, familySelection.members)} type="button">{text("Distribution and representatives", "耗时分布与代表执行")}</button></p></>
          : view === "similar"
          ? currentSimilar?.error
            ? <p className="audit-error" role="alert">{currentSimilar.error}</p>
            : currentSimilar?.response
            ? <AuditSimilarSqlList onSelect={(cluster: SimilarSqlCluster) => setSimilar((current) => current ? { ...current, selectedKey: cluster.cluster_key } : current)} response={currentSimilar.response} selectedKey={currentSimilar.selectedKey} text={text} />
            : <p className="audit-empty" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>
          : view === "families"
          ? <>{digestUnavailable
            ? <AuditDigestGuidance text={text} />
            : loading && !currentFamilies
            ? <p className="audit-empty" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>
            : currentFamilies && currentFamilies.rows.length === 0
            ? <p className="audit-empty">{text("No exact query families were returned for this fixed window.", "此固定窗口未返回精确查询族。")}</p>
            : <AuditFamilyList disabled={loading} families={currentFamilies?.rows ?? []} familyTop={currentOverview?.family_top ?? null} onSelect={(family) => void selectFamily(family)} selectedKey={familySelection && familySelection.family.membership.state === "exact_native_digest" ? familySelection.family.membership.family_key : null} sort={familySort} text={text} />}
            {currentFingerprints && <AuditFingerprintFamilyList error={currentFingerprints.error} loading={currentFingerprints.loading} response={currentFingerprints.response} text={text} />}</>
          : <AuditOccurrenceTable activeAt={currentActiveAt} disabled={loading} occurrences={currentOccurrences} onSelect={selectOccurrence} selectedKey={selected ? occurrenceRowKey(selected, occurrenceRows?.indexOf(selected) ?? 0) : null} text={text} />}
        {activeFamily && mode === "family" && !familyDetail && <FamilySummary family={activeFamily} onReturn={returnToParent} text={text} />}
        <div className="audit-board-main-foot"><span>{footLeft}</span><span>{text("Audit records may be ingested late", "审计记录可能延迟入库")}</span></div>
      </section>
      {view === "similar"
        ? selectedCluster && currentSimilar?.response && <AuditSimilarAside
          cluster={selectedCluster}
          dimensions={currentSimilar.response.dimensions}
          key={selectedCluster.cluster_key}
          loading={loading}
          onClose={() => setSimilar((current) => current ? { ...current, selectedKey: null } : current)}
          onDetach={(keys) => void loadSimilar(currentSimilar.response!.scope, [...currentSimilar.detached, ...keys], currentSimilar.refined, selectedCluster.cluster_key)}
          onRefine={() => void loadSimilar(currentSimilar.response!.scope, currentSimilar.detached, [...currentSimilar.refined, selectedCluster.cluster_key])}
          onUndo={() => void loadSimilar(currentSimilar.response!.scope, [], currentSimilar.refined, selectedCluster.cluster_key)}
          splitCount={currentSimilar.detached.length}
          text={text}
        />
        : familySelection && !currentActiveAt && !familyDetailShown
        ? <AuditFamilyAside
          canDiagnose={selectedMember?.query.profile_fetch.state === "available" && Boolean(onOpenDoctor)}
          canPlanCheck={selectedMember?.query.plan_check.state === "available"}
          error={familySelection.error}
          family={familySelection.family}
          fetchedQueryIds={fetchedQueryIds}
          loading={familySelection.loading}
          members={familySelection.members}
          memberSql={memberSql}
          memberSqlError={memberSqlRequest ? sqlError : null}
          memberSqlLoading={sqlLoading}
          onClose={closeFamily}
          onViewMemberSql={() => void viewMemberSql()}
          onDiagnose={() => void fetchMemberProfile("diagnose")}
          onPlanCheck={onOpenPlanCheck ? planCheckMember : undefined}
          onSelectMember={(key) => setFamilySelection((current) => current ? { ...current, memberKey: key } : current)}
          baseline={baselineState(baselineQueryId, familySelection.members, selectedMember)}
          onCompareWithBaseline={onOpenCompare ? () => void compareWithBaseline() : undefined}
          onSetBaseline={onSetBaseline ? () => void fetchMemberProfile("baseline") : undefined}
          onShowMembers={() => setFamilySelection((current) => current ? { ...current, showAll: true } : current)}
          profileLoading={profileLoading}
          selectedMemberKey={familySelection.memberKey}
          text={text}
        />
        // The detail panel opens only once a row is chosen; the list keeps the full width until then.
        : selected && <SelectedAuditOccurrence
          doctorProfile={doctorProfile}
          onClose={closeOccurrence}
          onDiagnose={() => void fetchProfile(selected, true)}
          onOpenDoctor={onOpenDoctor}
          onViewSql={() => void viewSql()}
          profileError={profileError}
          profile={visibleProfile}
          profileLoading={profileLoading}
          occurrence={selected}
          sql={shownSql}
          sqlError={sqlError}
          sqlLoading={sqlLoading}
          text={text}
        />}
    </div>}
  </section>;
}

function AuditUnavailable({ onConnect, session, text }: { onConnect(): void; session?: ConnectionSessionView; text: LocalizedText }): JSX.Element {
  const detail = !session
    ? text("Connect to a database with AuditLoader to read its query history.", "连接已安装 AuditLoader 的数据库后可读取查询历史。")
    : text(`Audit workload is unavailable for this connection: ${session.capabilities.audit}. The existing Queries fallback remains separate.`, `该连接的 Audit 工作负载不可用：${session.capabilities.audit}。既有 Queries 降级入口保持独立。`);
  return <section className="audit-unavailable panel"><p className="eyebrow">AuditLoader</p><h2>{text("Audit workload unavailable", "审计工作负载不可用")}</h2><p>{detail}</p><button className="button accent" onClick={onConnect} type="button">{text("Connect StarRocks", "连接 StarRocks")}</button></section>;
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
  return <section className="audit-window-evidence" aria-label={text("Audit window coverage", "审计窗口覆盖")}>
    {truncated && <p><strong>{text("Partial results; narrow the filters or increase the limit.", "仅展示部分结果，可缩小筛选范围或增加条数上限。")}</strong> <span title={formatUtcWindow(resultScope)}>{text("Returned", "已返回")} {response?.top.returned_count ?? "—"} / {response?.top.full_window_count ?? "—"}</span></p>}
    {pendingDraft && <p role="status">{text("Filters changed. Apply them to update the results.", "筛选条件已修改，点击应用后更新结果。")}</p>}
    {conflicting && <p role="status">{text("Some audit records conflict; affected metrics may be unavailable.", "部分审计记录存在冲突，相关指标可能缺失。")}</p>}
    {ungrouped && <p>{text("Some queries lack a native SQL digest; they are grouped separately below by unofficial fingerprint.", "部分查询缺少原生 SQL digest，已在下方按非官方指纹单独归组。")}</p>}
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
    <h3 id="audit-occurrences-title">{title}</h3>
    <div className="audit-table-wrap"><table aria-label={activeAt ? title : text("Audited query rankings", "审计查询排行")} className={activeAt ? undefined : "audit-occurrence-table"}><thead><tr>
      <th>{text("Query ID", "查询 ID")}</th><th>SQL</th><th>{text("Start / reader zone", "开始 / 读取时区")}</th><th>{text("Source state", "源状态")}</th>
      {activeAt && <><th>{text("Approx. end / reader zone", "近似结束 / 读取时区")}</th><th>{text("Elapsed at selected time", "截至所选时刻已持续")}</th></>}
      <th title="queryTime">{text("Query duration", "查询耗时")}</th><th title="scanRows">{text("Scanned rows", "扫描行数")}</th><th title="returnRows">{text("Returned rows", "返回行数")}</th><th title="scanBytes">{text("Scanned bytes", "扫描数据量")}</th><th title="cpuCost">{text("CPU time", "CPU 耗时")}</th><th title="memoryCost">{text("Audit memory", "审计内存")}</th><th>Profile</th>
    </tr></thead><tbody>{rows.map((row, index) => {
      const key = occurrenceRowKey(row, index);
      const participant = activeAt?.participants.rows[index];
      return <tr className={`audit-row${key === selectedKey ? " selected" : ""}`} key={key} onClick={() => { if (!disabled && key !== selectedKey) onSelect(key); }}>
        <td className="audit-cell-id"><button aria-label={`${text("Select audited query", "选择审计查询")} ${row.query.query_id}`} aria-pressed={key === selectedKey} disabled={disabled} onClick={(event) => { event.stopPropagation(); onSelect(key); }} title={row.query.query_id} type="button">{compactQueryId(row.query.query_id)}</button></td>
        <OccurrenceStatementCell sample={row.statement_sample} text={text} />
        <td className="audit-cell-time">{observedAt(row.query)}</td><td className={`audit-cell-status audit-status-${statusTone(row.query.status)}`} title={row.query.status}>{auditStatusLabel(row.query.status, text)}</td>
        {participant && <><td className="audit-cell-num" title={`${participant.end_milliseconds} ms`}>{auditClockText(participant.end_milliseconds)}</td><td className="audit-cell-num" title={`${participant.elapsed_milliseconds} ms`}>{auditMillisecondsText(participant.elapsed_milliseconds)}</td></>}
        {(["query_time", "scan_rows", "return_rows", "scan_bytes", "cpu_cost", "memory_cost"] as const).map((id) => <td className="audit-cell-num" key={id} title={metricRawText(metric(row, id))}>{metricValue(metric(row, id))}</td>)}<td className={`audit-cell-profile audit-profile-${row.query.profile_fetch.state}`} title={row.query.profile_fetch.state === "available" ? undefined : row.query.profile_fetch.reason}>{profileLabel(row.query, text)}</td>
      </tr>;
    })}</tbody></table></div>
    {disabled && rows.length === 0 && <p className="audit-empty" role="status">{text("Reading audit records…", "正在读取审计记录…")}</p>}
    {!disabled && (activeAt || occurrences) && rows.length === 0 && <p className="audit-empty">{activeAt ? text("No retained query interval covers this time.", "没有保留查询区间覆盖该时刻。") : text("No occurrences were returned for this fixed window.", "此固定窗口未返回执行记录。")}</p>}
    {!activeAt && <details className="audit-coverage"><summary>{text("Metric coverage details", "指标覆盖详情")}</summary><p>{coverageMetricLine(occurrences?.coverage, text)}</p></details>}
  </section>;
}

export function FamilySummary({ family, onReturn, text }: { family: AuditFamilyPresentation; onReturn?(): void; text: LocalizedText }): JSX.Element {
  return <section className="audit-family-summary"><div><h3>{text("Exact native digest family", "精确原生 digest 查询族")}</h3>{onReturn && <button className="button secondary" onClick={() => onReturn()} type="button">{text("Return to parent window", "返回父窗口")}</button>}</div><p><FamilyLabel family={family} /></p><dl>
    <div><dt>{text("Execution count", "执行次数")}</dt><dd>{family.execution_count ?? "—"}</dd><dt>{text("Error proportion · ERR / total", "错误占比 · ERR / 总数")}</dt><dd title={errorProportionText(family.error_proportion)}>{errorProportionText(family.error_proportion)}</dd></div>
    <div><dt>{text("Cumulative query time", "累计查询耗时")}</dt><dd title={aggregateRawText(family, "query_time", "sum")}>{metricValue(aggregateMetric(family, "query_time", "sum"))}</dd><dt>{text("Cumulative CPU", "累计 CPU")}</dt><dd title={aggregateRawText(family, "cpu_cost", "sum")}>{metricValue(aggregateMetric(family, "cpu_cost", "sum"))}</dd></div>
    <div><dt>{text("Maximum query time", "最大查询耗时")}</dt><dd title={aggregateRawText(family, "query_time", "max")}>{metricValue(aggregateMetric(family, "query_time", "max"))}</dd><dt>{text("Maximum audit memory", "最大审计内存")}</dt><dd title={aggregateRawText(family, "memory_cost", "max")}>{metricValue(aggregateMetric(family, "memory_cost", "max"))}</dd></div>
    <div><dt>{text("Cumulative scanned bytes", "累计扫描数据量")}</dt><dd title={aggregateRawText(family, "scan_bytes", "sum")}>{metricValue(aggregateMetric(family, "scan_bytes", "sum"))}</dd></div>
  </dl><p>{text("Family values are returned by the server full-window aggregation; the member Top list is never summed here. CPU sum is not wall-clock time; audit memory max is not concurrent total memory.", "查询族数值由服务端全窗口聚合返回，绝不相加成员 Top 列表。CPU 累计不是墙钟时间；审计内存 max 不是并发总内存。")}</p></section>;
}

export function SelectedAuditOccurrence({ doctorProfile, onClose, onDiagnose, onOpenDoctor, onViewSql, profile, profileError, profileLoading, occurrence, sql, sqlError, sqlLoading, text }: {
  doctorProfile: ProfileFetchResponse | null;
  onClose?(): void;
  onDiagnose(): void;
  onOpenDoctor?: (profile: ProfileFetchResponse) => void;
  onViewSql(): void;
  profile: ProfileFetchResponse | null;
  profileLoading: boolean;
  profileError?: string | null;
  occurrence: AuditOccurrencePresentation | null;
  sql: string | null;
  sqlError: string | null;
  sqlLoading: boolean;
  text: LocalizedText;
}): JSX.Element {
  const query = occurrence?.query ?? null;
  if (!occurrence || !query) {
    return <aside className="audit-selection audit-selection-empty" aria-labelledby="audit-selection-title">
      <h3 id="audit-selection-title">{text("Execution details", "执行详情")}</h3>
      <p>{text("Click a row to see its facts, read its SQL and diagnose its Profile.", "点击表格中的一行，查看详情、读取 SQL 并诊断其 Profile。")}</p>
    </aside>;
  }
  const pending = metric(occurrence, "pending_time");
  const errorCode = occurrence.error_code;
  const pendingSource = !pending ? "not_returned" : pending.input.state === "text"
    ? `pendingTimeMs=${JSON.stringify(pending.input.value)} → ${metricRawText(pending)}`
    : pending.input.state === "unavailable" ? `unavailable · ${pending.input.reason}` : "sql_null";
  const errorCodeText = !errorCode ? "not_returned" : errorCode.state === "text"
    ? JSON.stringify(errorCode.value) : errorCode.state === "unavailable" ? `unavailable · ${errorCode.reason}` : "sql_null";
  const profileAvailable = query.profile_fetch.state === "available";
  const facts: [string, AuditMetricId][] = [
    [text("Query duration", "查询耗时"), "query_time"],
    [text("Scanned rows", "扫描行数"), "scan_rows"],
    [text("Scanned bytes", "扫描数据量"), "scan_bytes"],
    [text("CPU time", "CPU 耗时"), "cpu_cost"],
    [text("Returned rows", "返回行数"), "return_rows"],
    [text("Audit memory", "审计内存"), "memory_cost"],
  ];
  return <aside className="audit-selection" aria-labelledby="audit-selection-title">
    <header>
      <h3 id="audit-selection-title">{text("Execution details", "执行详情")}</h3>
      <span className="audit-selection-id">
        <code title={query.query_id}>{compactQueryId(query.query_id)}</code>
        {onClose && <button aria-label={text("Close details", "关闭详情")} className="audit-selection-close" onClick={onClose} type="button"><svg aria-hidden="true" fill="none" height="16" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="16"><path d="M6 6l12 12M18 6L6 18" /></svg></button>}
      </span>
    </header>
    <p className="audit-selection-context"><span className={`audit-status-chip audit-status-${statusTone(query.status)}`}>{auditStatusLabel(query.status, text)}</span> · {query.database ?? "—"} · {observedAt(query)}</p>
    <dl className="audit-selection-facts">{facts.map(([label, id]) => <div key={id}><dt>{label}</dt><dd title={metricRawText(metric(occurrence, id))}>{metricValue(metric(occurrence, id))}</dd></div>)}</dl>
    <div className="audit-selection-actions">
      {doctorProfile && onOpenDoctor
        ? <button className="button accent" onClick={() => onOpenDoctor(doctorProfile)} type="button">{text("Open diagnosis", "打开诊断")}</button>
        : <button className="button accent" disabled={!profileAvailable || profileLoading || !onOpenDoctor} onClick={onDiagnose} title={profileAvailable ? undefined : query.profile_fetch.reason} type="button">{profileLoading ? text("Fetching Profile…", "正在拉取 Profile…") : text("Diagnose", "诊断")}</button>}
      <SqlToggleButton controls="audit-selected-sql" expanded={sql !== null} loading={sqlLoading} onToggle={onViewSql} />
    </div>
    <p className="audit-selection-note">{profileAvailable
      ? text("Reads the existing Profile only; the SQL is never rerun.", "只读取已有 Profile，不会重新执行 SQL。")
      : `${text("No Profile to diagnose", "没有可诊断的 Profile")}: ${profileLabel(query, text)}`}</p>
    {profileError && <p className="audit-sql-error" role="alert">{profileError}</p>}
    {profile && <p className="audit-profile-receipt">{text("Profile fetched", "Profile 已拉取")} · {profile.source_kind} · {shortId(profile.analysis_key)}</p>}
    {sqlError && <p className="audit-sql-error" role="alert">{sqlError}</p>}
    {sql !== null && <SqlText className="audit-sql" copyLabel={text("Copy SQL", "复制 SQL")} id="audit-selected-sql" label={text("Selected audited SQL", "已选审计 SQL")} value={sql} />}
    <details className="audit-optional-facts"><summary>{text("Raw audit details", "原始审计详情")}</summary><dl>
      <div><dt>{text("Query ID", "查询 ID")}</dt><dd><code>{query.query_id}</code></dd></div>
      <div><dt>{text("Pending time", "排队耗时")} · pendingTimeMs</dt><dd>{metricText(pending)} · {pending?.state ?? "not_returned"}</dd><dd className="audit-fact-source">{text("Source", "原值")}: {pendingSource}</dd></div>
      <div><dt>errorCode · {text("source text", "源文本")}</dt><dd>{errorCodeText}</dd></div>
    </dl><p>{text("Facts only: an empty errorCode does not mean success; pending time alone does not prove blocking. sql_null, negative_sentinel and exact zero remain distinct.", "仅呈现事实：空 errorCode 不表示成功；排队耗时本身不证明阻塞。sql_null、negative_sentinel 与真零分别保留。")}</p></details>
  </aside>;
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

/** Exact facts stay bare text; anything else is a dashed tag around the same metricText (never 0). */
/** One line of the execution's own audited stmt prefix; every non-text state is a dashed tag, never an empty cell. */
export function OccurrenceStatementCell({ sample, text }: { sample: AuditStatementSample; text: LocalizedText }): JSX.Element {
  if (sample.state === "text" && sample.value.trim() !== "") {
    const shown = `${sample.value}${sample.truncated ? "…" : ""}`;
    return <td className="audit-cell-sql" title={shown}>{shown}</td>;
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
  return <td className="audit-cell-sql" title={note}><span className="audit-value-tag">{label}</span></td>;
}

function metricValue(value: AuditMetricFact | null): ReactNode {
  const shown = metricText(value);
  const tone = metricTone(value);
  return tone === "exact" ? shown : <span className={`audit-value-tag audit-value-${tone}`}>{shown}</span>;
}

/** Same text as before split into a mono digest and a muted context; textContent is unchanged. */
function FamilyLabel({ family }: { family: AuditFamilyPresentation }): JSX.Element {
  const { text } = useLocale();
  if (family.membership.state !== "exact_native_digest") return <span className="audit-family-ungrouped" title={family.membership.reason}>{text("Not grouped", "未归组")} · {ungroupedReasonText(family.membership.reason, text)}</span>;
  const { context, native_digest } = family.membership;
  return <span className="audit-family-label"><span className="audit-family-digest">{shortId(native_digest)}</span><span className="audit-family-context"> · {context.catalog ?? "—"} / {context.database ?? "—"} · {context.cluster ?? "—"} / {context.engine_version ?? "—"}</span></span>;
}

/** Visual tone of the server status string; unknown future states stay neutral. */
function statusTone(status: string): "ok" | "critical" | "accent" | "neutral" {
  if (status === "finished") return "ok";
  if (status === "error") return "critical";
  if (status === "running") return "accent";
  return "neutral";
}

function coverageMetricLine(coverage: AuditOccurrenceResponse["coverage"] | undefined, text: LocalizedText): string {
  if (!coverage) return text("Coverage not loaded", "覆盖未加载");
  return coverage.metric_inputs.map((item) => `${item.metric_id}: NULL ${item.sql_null_count}, ${text("nonnegative", "非负")} ${item.nonnegative_count}, ${text("negative", "负值")} ${item.negative_count}`).join(" · ");
}

function observedAt(query: ConnectedQueryRef): string {
  return query.observed_at_utc ?? query.observed_at_local ?? "—";
}

export function auditStatusLabel(status: string, text: LocalizedText): string {
  const labels: Record<string, [string, string]> = {
    finished: ["Finished", "已完成"], error: ["Failed", "执行失败"], unknown: ["Status unknown", "状态未知"],
    running: ["Running", "执行中"], cancelled: ["Cancelled", "已取消"],
  };
  return labels[status] ? text(...labels[status]) : status;
}

function profileLabel(query: ConnectedQueryRef, text: LocalizedText): string {
  // Audit rows cannot prove a Profile was kept; "available" means a fetch may be tried.
  if (query.profile_fetch.state === "available") return text("Can try", "可尝试拉取");
  const labels: Record<string, [string, string]> = {
    query_not_finished: ["Query not finished", "查询尚未结束"],
    profile_unavailable: ["Profile unavailable", "暂无 Profile"],
    query_id_unavailable: ["Query ID unavailable", "缺少查询 ID"],
    unknown: ["Availability unknown", "可用性未知"],
  };
  const label = labels[query.profile_fetch.reason];
  return label ? text(...label) : text("Not available to fetch", "暂不可拉取");
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

export function auditUserFilterNote(capability: ConnectionCapabilityState, text: LocalizedText): string {
  const labels: Record<Exclude<ConnectionCapabilityState, "available">, [string, string]> = {
    unavailable: ["User filter unavailable: the AuditLoader user column is unavailable.", "用户筛选不可用：AuditLoader 用户列不可用。"],
    forbidden: ["User filter forbidden for this connection.", "该连接禁止用户筛选。"],
    unknown: ["User filter availability is unknown; source column availability is unknown.", "用户筛选能力未知；源列可用性未知。"],
  };
  return capability === "available" ? "" : text(...labels[capability]);
}

export function auditStatementFilterNote(capability: ConnectionCapabilityState, text: LocalizedText): string {
  const labels: Record<Exclude<ConnectionCapabilityState, "available">, [string, string]> = {
    unavailable: ["Unavailable: the AuditLoader table lacks a TINYINT isQuery column or the stmt column, so all statements are shown.", "不可用：AuditLoader 表缺少 TINYINT 类型的 isQuery 列或 stmt 列，因此显示全部语句。"],
    forbidden: ["Statement filter forbidden for this connection; all statements are shown.", "该连接禁止语句筛选，显示全部语句。"],
    unknown: ["Statement filter availability is unknown; all statements are shown.", "语句筛选能力未知，显示全部语句。"],
  };
  return capability === "available" ? "" : text(...labels[capability]);
}

function windowPresetLabel(preset: AuditWindowPreset, text: LocalizedText): string {
  const labels: Record<AuditWindowPreset, [string, string]> = {
    "1h": ["Last 1 h", "最近 1 小时"],
    "12h": ["Last 12 h", "最近 12 小时"],
    "1d": ["Last 1 day", "最近 1 天"],
    "3d": ["Last 3 days", "最近 3 天"],
    "7d": ["Last 7 days", "最近 7 天"],
    "30d": ["Last 1 month (30 days)", "最近 1 月（30 天）"],
  };
  return text(...labels[preset]);
}

function shortId(value: string): string {
  return value.length > 18 ? `${value.slice(0, 16)}…` : value;
}

function sortLabel(sort: AuditOccurrenceSort | AuditFamilySort, text: LocalizedText): string {
  const labels: Record<string, [string, string]> = {
    error_proportion_desc: ["error proportion descending", "错误占比降序"],
    query_time_desc: ["Longest duration first", "单次耗时最高"], scan_bytes_desc: ["Most scanned bytes first", "扫描数据量最大"], scan_rows_desc: ["Most scanned rows first", "扫描行数最多"], cpu_cost_desc: ["Highest CPU cost first", "CPU 消耗最高"], memory_cost_desc: ["Highest memory first", "内存占用最高"], newest_first: ["newest first", "最新优先"], cumulative_query_time_desc: ["Highest cumulative duration first", "累计耗时最高"], cumulative_scan_bytes_desc: ["Most cumulative scanned bytes first", "累计扫描量最大"], cumulative_cpu_cost_desc: ["Highest cumulative CPU first", "累计 CPU 消耗最高"], execution_count_desc: ["Most frequent first", "执行最频繁"], error_count_desc: ["error count descending", "错误次数降序"], max_query_time_desc: ["Highest peak duration first", "最慢一次耗时最高"], max_memory_cost_desc: ["Highest peak memory first", "峰值内存最高"],
  };
  return text(...labels[sort]);
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
