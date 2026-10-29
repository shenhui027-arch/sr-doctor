import type {
  AccessSessionStatus,
  AnalysisOutcome,
  AnalysisSnapshotReport,
  AuditActiveAtReadRequest,
  AuditActiveAtResponse,
  AuditFamilyDetailReadRequest,
  AuditFamilyDetailResponse,
  AuditFamilyReadRequest,
  AuditFingerprintFamilyResponse,
  AuditSimilarSqlReadRequest,
  AuditSimilarSqlResponse,
  AlertActionRequest,
  GovernanceChangeRequest,
  GovernanceCreateRequest,
  GovernanceItem,
  GovernanceItemRequest,
  GovernanceListResponse,
  GovernanceTransitionRequest,
  HistoryAlertsResponse,
  NotifyChannelSaveRequest,
  NotifyDeliveriesRequest,
  NotifyDeliveriesResponse,
  NotifyOverviewResponse,
  NotifySettings,
  NotifyTargetInput,
  NotifyTestResponse,
  HistoryCollectionRequest,
  HistorySeriesRequest,
  HistorySeriesResponse,
  HistoryTrendsRequest,
  HistoryLoadRequest,
  HistoryLoadResponse,
  HistoryTrendsResponse,
  HistoryStatusResponse,
  AuditOccurrenceReadRequest,
  AuditOccurrenceResponse,
  AuditOverviewReadRequest,
  AuditPerformanceReadRequest,
  AuditPerformanceResponse,
  AuditOverviewResponse,
  AuditSqlReadRequest,
  Capabilities,
  Comparison,
  ConnectionCreateRequest,
  ConnectionSessionView,
  ConnectedProfileComparisonRequest,
  ConnectedProfileComparisonResponse,
  ConnectedQueryListRequest,
  ConnectedQueryListResponse,
  ConnectedQuerySqlRequest,
  ConnectedQuerySqlResponse,
  ErrorEnvelope,
  ExplainRequest,
  ExplainResponse,
  ExportKind,
  ExportRequest,
  OfficialProfileReadRequest,
  OfficialProfileReadResponse,
  PlanAdvisorRequest,
  PlanAdvisorResponse,
  ProfileFetchRequest,
  ProfileFetchResponse,
  RuntimeProfileRequest,
  RunningProfileCandidateListRequest,
  RunningProfileCandidateListResponse,
  RunningProfileSnapshotRequest,
  RunningProfileSnapshotResponse,
} from "../../../contracts/generated/doctor-contracts";

export type ApiFailure = Pick<ErrorEnvelope, "code" | "stage" | "message_id" | "recoverability" | "details">;
export type ReportDownload = { blob: Blob; filename: "sr-doctor-report.json"; kind: ExportKind; manifestHash: string };

const configuredBase = import.meta.env.VITE_API_BASE ?? "/api/v1";

function sameOriginBase(base: string): string {
  const probeOrigin = "http://local.invalid";
  let resolved: URL;
  try {
    resolved = new URL(base, probeOrigin);
  } catch {
    throw new Error("The API base must be a same-origin path.");
  }
  if (!base.startsWith("/") || base.startsWith("//") || base.includes("\\") || resolved.origin !== probeOrigin) {
    throw new Error("The API base must be a same-origin path.");
  }
  return base.replace(/\/$/, "");
}

async function responseJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch (error) {
    if (isAbortError(error)) throw error;
    return null;
  }
}

function isAbortError(error: unknown): boolean {
  return typeof error === "object" && error !== null && "name" in error && error.name === "AbortError";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** Fired when a network deployment answers that the login session is missing or expired. */
export const AUTH_REQUIRED_EVENT = "doctor:auth-required";

function asFailure(value: unknown): ApiFailure {
  if (isRecord(value) && value.code === "auth_required" && typeof window !== "undefined") {
    window.dispatchEvent(new Event(AUTH_REQUIRED_EVENT));
  }
  if (typeof value === "object" && value !== null && "code" in value && "stage" in value) {
    const candidate = value as Partial<ApiFailure>;
    return {
      code: candidate.code ?? "request_failed",
      stage: candidate.stage ?? "request",
      message_id: candidate.message_id ?? "request_failed",
      recoverability: candidate.recoverability ?? "retry_same",
      details: candidate.details ?? {},
    };
  }
  return { code: "request_failed", stage: "request", message_id: "request_failed", recoverability: "retry_same", details: {} };
}

function analysisReport(value: unknown): AnalysisSnapshotReport {
  if (typeof value === "object" && value !== null && "outcome" in value) {
    const outcome = (value as { outcome?: AnalysisOutcome }).outcome;
    if (outcome?.report) return outcome.report;
  }
  throw asFailure(null);
}

function comparisonSnapshot(value: unknown): Comparison {
  if (typeof value === "object" && value !== null && "comparison" in value) {
    const comparison = (value as { comparison?: unknown }).comparison;
    if (typeof comparison === "object" && comparison !== null) return comparison as Comparison;
  }
  throw asFailure(null);
}

function exportMetadata(value: unknown, expectedKind: ExportKind): Pick<ReportDownload, "kind" | "manifestHash"> {
  if (typeof value !== "object" || value === null) throw invalidExportResponse();
  const artifact = value as { subject?: { kind?: unknown }; manifest?: { kind?: unknown; manifest_hash?: unknown } };
  const kind = artifact.subject?.kind;
  const manifestKind = artifact.manifest?.kind;
  const manifestHash = artifact.manifest?.manifest_hash;
  if (kind !== expectedKind || manifestKind !== expectedKind || typeof manifestHash !== "string" || !/^[0-9a-f]{64}$/.test(manifestHash)) {
    throw invalidExportResponse();
  }
  return { kind: expectedKind, manifestHash };
}

function invalidExportResponse(): ApiFailure {
  return { code: "invalid_export_response", stage: "export", message_id: "invalid_export_response", recoverability: "reanalyze_input", details: {} };
}

function unknownExecutionResponse(): ApiFailure {
  return { code: "execution_response_unknown", stage: "query_execution", message_id: "execution_response_unknown", recoverability: "status_unknown", details: {} };
}

function executionResponse<T>(value: unknown, keys: string[]): T {
  if (!isRecord(value) || keys.some(key => !(key in value))) throw unknownExecutionResponse();
  return value as T;
}

function executionProfileResponse(value: unknown, origin = "connected_sql_runtime"): ProfileFetchResponse {
  const response = executionResponse<ProfileFetchResponse>(value, ["state", "input_origin", "outcome"]);
  if (response.state !== "fetched" || response.input_origin !== origin || !isRecord(response.outcome) || !isRecord(response.outcome.report)) throw unknownExecutionResponse();
  return response;
}

export class DoctorApi {
  private readonly base: string;
  private readonly request: typeof fetch;
  private nonce: string | null = null;

  constructor(base = configuredBase, request?: typeof fetch) {
    this.base = sameOriginBase(base);
    this.request = request ?? ((input, init) => globalThis.fetch(input, init));
  }

  async capabilities(signal?: AbortSignal): Promise<Capabilities> {
    const response = await this.request(`${this.base}/capabilities`, {
      headers: { Accept: "application/json" },
      signal,
    });
    const body = await responseJson(response);
    if (!response.ok) throw asFailure(body);
    if (typeof body !== "object" || body === null || !("csrf" in body)) throw asFailure(null);

    const capabilities = body as Capabilities;
    this.nonce = capabilities.csrf.nonce;
    return capabilities;
  }

  /** Whether the server asks for a login and whether this browser has a session. */
  async accessStatus(signal?: AbortSignal): Promise<AccessSessionStatus> {
    const response = await this.request(`${this.base}/auth/session`, { headers: { Accept: "application/json" }, signal });
    const body = await responseJson(response);
    if (!response.ok || !isRecord(body) || typeof body.required !== "boolean" || typeof body.authenticated !== "boolean") throw asFailure(body);
    return { required: body.required, authenticated: body.authenticated, default_account: body.default_account === true };
  }

  /** Exchanges the login for an HttpOnly session cookie; the password is not kept. */
  async login(username: string, password: string): Promise<void> {
    const response = await this.request(`${this.base}/auth/session`, {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    if (!response.ok) throw asFailure(await responseJson(response));
    this.nonce = null;
  }

  async logout(): Promise<void> {
    if (!this.nonce) await this.capabilities();
    const response = await this.request(`${this.base}/auth/session`, {
      method: "DELETE",
      headers: { Accept: "application/json", "Content-Type": "application/json", "X-Doctor-CSRF": this.nonce! },
    });
    if (!response.ok) throw asFailure(await responseJson(response));
    this.nonce = null;
  }

  async postJson<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
    return this.post<T>(path, JSON.stringify(body), { "Content-Type": "application/json" }, signal);
  }

  async postForm<T>(path: string, body: FormData, signal?: AbortSignal): Promise<T> {
    return this.post<T>(path, body, {}, signal);
  }

  async analyzeJson(body: unknown): Promise<AnalysisSnapshotReport> {
    return analysisReport(await this.postJson<unknown>("/analyses", body));
  }

  async analyzeForm(body: FormData): Promise<AnalysisSnapshotReport> {
    return analysisReport(await this.postForm<unknown>("/analyses", body));
  }

  async compareForm(body: FormData): Promise<Comparison> {
    return comparisonSnapshot(await this.postForm<unknown>("/comparisons", body));
  }

  async createConnection(request: ConnectionCreateRequest, signal?: AbortSignal): Promise<ConnectionSessionView> {
    return this.postJson<ConnectionSessionView>("/connections", request, signal);
  }

  async closeConnection(connectionId: string): Promise<void> {
    if (!this.nonce) await this.capabilities();
    const response = await this.request(`${this.base}/connections/${encodeURIComponent(connectionId)}`, {
      method: "DELETE",
      headers: { Accept: "application/json", "Content-Type": "application/json", "X-Doctor-CSRF": this.nonce! },
    });
    if (!response.ok) {
      const body = await responseJson(response);
      throw asFailure(body);
    }
  }

  /**
   * Best-effort close while the page unloads. A reload otherwise leaves the
   * server session open until it expires, and the local session limit is small.
   * `keepalive` lets the request outlive the page; failures are ignored.
   */
  releaseConnectionOnUnload(connectionId: string): void {
    if (!this.nonce) return;
    try {
      void this.request(`${this.base}/connections/${encodeURIComponent(connectionId)}`, {
        method: "DELETE",
        keepalive: true,
        headers: { Accept: "application/json", "Content-Type": "application/json", "X-Doctor-CSRF": this.nonce },
      }).catch(() => undefined);
    } catch {
      // Unload is best effort; the server still expires the session.
    }
  }

  async searchConnectedQueries(request: ConnectedQueryListRequest, signal?: AbortSignal): Promise<ConnectedQueryListResponse> {
    return this.postConnection<ConnectedQueryListResponse>("queries/search", request, signal);
  }

  async fetchConnectedQuerySql(request: ConnectedQuerySqlRequest, signal?: AbortSignal): Promise<ConnectedQuerySqlResponse> {
    return this.postConnection<ConnectedQuerySqlResponse>("queries/sql", request, signal);
  }

  async readAuditOccurrences(request: AuditOccurrenceReadRequest, signal?: AbortSignal): Promise<AuditOccurrenceResponse> {
    return this.postConnection<AuditOccurrenceResponse>("audit/occurrences", request, signal);
  }

  async readAuditSql(request: AuditSqlReadRequest, signal?: AbortSignal): Promise<ConnectedQuerySqlResponse> {
    return this.postConnection<ConnectedQuerySqlResponse>("audit/sql", request, signal);
  }

  async readAuditFamilyDetail(request: AuditFamilyDetailReadRequest, signal?: AbortSignal): Promise<AuditFamilyDetailResponse> {
    const value = await this.postConnection<unknown>("audit/family-detail", request, signal);
    // Bind the envelope before exposing a new discovery; metrics remain backend facts.
    if (!isRecord(value) || value.connection_id !== request.connection_id
      || !Number.isInteger(value.discovery_generation) || (value.discovery_generation as number) <= request.discovery_generation
      || !isRecord(value.family) || value.family.state !== "exact_native_digest" || value.family.family_key !== request.family_key
      || !isRecord(value.scope) || !isRecord(value.distribution) || !isRecord(value.representatives)
      || !isRecord(value.profile_evidence_coverage) || !Array.isArray(value.limitations)) {
      throw { code: "invalid_audit_family_detail_response", stage: "query_source", message_id: "invalid_audit_family_detail_response", recoverability: "change_input", details: {} } satisfies ApiFailure;
    }
    return value as AuditFamilyDetailResponse;
  }

  async readAuditFingerprintFamilies(request: AuditFamilyReadRequest, signal?: AbortSignal): Promise<AuditFingerprintFamilyResponse> {
    const value = await this.postConnection<unknown>("audit/fingerprint-families", request, signal);
    // Only a product-unofficial list bound to this connection may render beside exact families.
    if (!isRecord(value) || value.connection_id !== request.connection_id || value.authority !== "product_unofficial"
      || !Array.isArray(value.rows) || !isRecord(value.coverage) || !isRecord(value.top) || !Array.isArray(value.limitations)) {
      throw { code: "invalid_audit_fingerprint_response", stage: "query_source", message_id: "invalid_audit_fingerprint_response", recoverability: "change_input", details: {} } satisfies ApiFailure;
    }
    return value as AuditFingerprintFamilyResponse;
  }

  async readAuditSimilarSql(request: AuditSimilarSqlReadRequest, signal?: AbortSignal): Promise<AuditSimilarSqlResponse> {
    const value = await this.postConnection<unknown>("audit/similar-sql", request, signal);
    if (!isRecord(value) || value.connection_id !== request.connection_id || !Array.isArray(value.clusters)
      || !Array.isArray(value.independent) || !Array.isArray(value.unavailable) || !isRecord(value.coverage) || !isRecord(value.dimensions)) {
      throw { code: "invalid_audit_similar_response", stage: "query_source", message_id: "invalid_audit_similar_response", recoverability: "change_input", details: {} } satisfies ApiFailure;
    }
    return value as AuditSimilarSqlResponse;
  }

  /** Stage B: local history store and this session's collector; reads no StarRocks data. */
  async readHistoryStatus(connectionId: string | null, signal?: AbortSignal): Promise<HistoryStatusResponse> {
    const value = await this.postJson<unknown>("/history/status", { connection_id: connectionId }, signal);
    if (!isRecord(value) || !isRecord(value.store) || !isRecord(value.collection) || !Array.isArray(value.coverage) || !Array.isArray(value.runs)) {
      throw { code: "invalid_history_status", stage: "history", message_id: "invalid_history_status", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as HistoryStatusResponse;
  }

  async setHistoryCollection(request: HistoryCollectionRequest, signal?: AbortSignal): Promise<void> {
    await this.postJson<unknown>("/history/collection", request, signal);
  }

  async readHistoryLoad(request: HistoryLoadRequest, signal?: AbortSignal): Promise<HistoryLoadResponse> {
    return this.postJson<HistoryLoadResponse>("/history/load", request, signal);
  }

  async readHistoryTrends(request: HistoryTrendsRequest, signal?: AbortSignal): Promise<HistoryTrendsResponse> {
    const value = await this.postJson<unknown>("/history/trends", request, signal);
    if (!isRecord(value) || !Array.isArray(value.rows) || !isRecord(value.counts) || !isRecord(value.rule)) {
      throw { code: "invalid_history_trends", stage: "history", message_id: "invalid_history_trends", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as HistoryTrendsResponse;
  }

  async readHistorySeries(request: HistorySeriesRequest, signal?: AbortSignal): Promise<HistorySeriesResponse> {
    const value = await this.postJson<unknown>("/history/series", request, signal);
    if (!isRecord(value) || !Array.isArray(value.points) || !isRecord(value.row) || !Array.isArray(value.exemplars)) {
      throw { code: "invalid_history_series", stage: "history", message_id: "invalid_history_series", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as HistorySeriesResponse;
  }

  async readHistoryAlerts(target: string | null, signal?: AbortSignal): Promise<HistoryAlertsResponse> {
    const value = await this.postJson<unknown>("/history/alerts", { target }, signal);
    if (!isRecord(value) || !Array.isArray(value.alerts) || !isRecord(value.counts) || !Array.isArray(value.channels)) {
      throw { code: "invalid_history_alerts", stage: "history", message_id: "invalid_history_alerts", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as HistoryAlertsResponse;
  }

  async alertAction(request: AlertActionRequest, signal?: AbortSignal): Promise<void> {
    await this.postJson<unknown>("/history/alerts/action", request, signal);
  }

  /** Resolves the target's active alerts raised under an older rule version. */
  async resolveRuleChangedAlerts(target: string | null, signal?: AbortSignal): Promise<void> {
    await this.postJson<unknown>("/history/alerts/resolve-rule-changed", { target }, signal);
  }

  private notifyOverview(value: unknown): NotifyOverviewResponse {
    if (!isRecord(value) || !isRecord(value.settings) || !Array.isArray(value.channels) || !Array.isArray(value.targets)) {
      throw { code: "invalid_notify_overview", stage: "notify", message_id: "invalid_notify_overview", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as NotifyOverviewResponse;
  }

  /** Channels, targets and rules. Secrets come back only as "saved". */
  async readNotifyOverview(signal?: AbortSignal): Promise<NotifyOverviewResponse> {
    return this.notifyOverview(await this.postJson<unknown>("/notify/overview", {}, signal));
  }

  async saveNotifySettings(settings: NotifySettings, signal?: AbortSignal): Promise<NotifyOverviewResponse> {
    return this.notifyOverview(await this.postJson<unknown>("/notify/settings", settings, signal));
  }

  async saveNotifyChannel(request: NotifyChannelSaveRequest, signal?: AbortSignal): Promise<NotifyOverviewResponse> {
    return this.notifyOverview(await this.postJson<unknown>("/notify/channel", request, signal));
  }

  async saveNotifyTarget(target: NotifyTargetInput, signal?: AbortSignal): Promise<NotifyOverviewResponse> {
    return this.notifyOverview(await this.postJson<unknown>("/notify/target", target, signal));
  }

  async deleteNotifyTarget(targetId: string, signal?: AbortSignal): Promise<NotifyOverviewResponse> {
    return this.notifyOverview(await this.postJson<unknown>("/notify/target/delete", { target_id: targetId }, signal));
  }

  /** Sends one test message to a saved or unsaved target; nothing is saved. */
  async testNotifyTarget(target: NotifyTargetInput, signal?: AbortSignal): Promise<NotifyTestResponse> {
    const value = await this.postJson<unknown>("/notify/test", { target }, signal);
    if (!isRecord(value) || typeof value.state !== "string") {
      throw { code: "invalid_notify_test", stage: "notify", message_id: "invalid_notify_test", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as NotifyTestResponse;
  }

  async readNotifyDeliveries(request: NotifyDeliveriesRequest, signal?: AbortSignal): Promise<NotifyDeliveriesResponse> {
    const value = await this.postJson<unknown>("/notify/deliveries", request, signal);
    if (!isRecord(value) || !Array.isArray(value.deliveries) || !Array.isArray(value.sms_usage)) {
      throw { code: "invalid_notify_deliveries", stage: "notify", message_id: "invalid_notify_deliveries", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as NotifyDeliveriesResponse;
  }

  async readGovernance(target: string | null, signal?: AbortSignal): Promise<GovernanceListResponse> {
    const value = await this.postJson<unknown>("/governance/list", { target }, signal);
    if (!isRecord(value) || !Array.isArray(value.items)) {
      throw { code: "invalid_governance_list", stage: "history", message_id: "invalid_governance_list", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as GovernanceListResponse;
  }

  private async governanceItem(path: string, request: unknown, signal?: AbortSignal): Promise<GovernanceItem> {
    const value = await this.postJson<unknown>(`/governance/${path}`, request, signal);
    if (!isRecord(value) || typeof value.item_id !== "string" || !Array.isArray(value.events)) {
      throw { code: "invalid_governance_item", stage: "history", message_id: "invalid_governance_item", recoverability: "retry_same", details: {} } satisfies ApiFailure;
    }
    return value as GovernanceItem;
  }

  createGovernanceItem(request: GovernanceCreateRequest, signal?: AbortSignal): Promise<GovernanceItem> { return this.governanceItem("create", request, signal); }
  transitionGovernanceItem(request: GovernanceTransitionRequest, signal?: AbortSignal): Promise<GovernanceItem> { return this.governanceItem("transition", request, signal); }
  pinGovernanceSnapshot(request: GovernanceItemRequest, signal?: AbortSignal): Promise<GovernanceItem> { return this.governanceItem("pin", request, signal); }
  recordGovernanceChange(request: GovernanceChangeRequest, signal?: AbortSignal): Promise<GovernanceItem> { return this.governanceItem("change", request, signal); }
  verifyGovernanceItem(request: GovernanceItemRequest, signal?: AbortSignal): Promise<GovernanceItem> { return this.governanceItem("verify", request, signal); }

  async clearHistory(signal?: AbortSignal): Promise<void> {
    await this.postJson<unknown>("/history/clear", {}, signal);
  }

  async readAuditOverview(request: AuditOverviewReadRequest, signal?: AbortSignal): Promise<AuditOverviewResponse> {
    return this.postConnection<AuditOverviewResponse>("audit/overview", request, signal);
  }

  async readAuditPerformance(request: AuditPerformanceReadRequest, signal?: AbortSignal): Promise<AuditPerformanceResponse> {
    return this.postConnection<AuditPerformanceResponse>("audit/performance", request, signal);
  }

  async readAuditActiveAt(request: AuditActiveAtReadRequest, signal?: AbortSignal): Promise<AuditActiveAtResponse> {
    return this.postConnection<AuditActiveAtResponse>("audit/active-at", request, signal);
  }

  async fetchConnectedProfile(request: ProfileFetchRequest, signal?: AbortSignal): Promise<ProfileFetchResponse> {
    return this.postConnection<ProfileFetchResponse>("profiles/fetch", request, signal);
  }

  async readRunningProfileCandidates(request: RunningProfileCandidateListRequest, signal?: AbortSignal): Promise<RunningProfileCandidateListResponse> {
    return this.postConnection<RunningProfileCandidateListResponse>("profiles/running-candidates", request, signal);
  }

  async readRunningProfileSnapshot(request: RunningProfileSnapshotRequest, signal?: AbortSignal): Promise<RunningProfileSnapshotResponse> {
    return this.postConnection<RunningProfileSnapshotResponse>("profiles/running-snapshot", request, signal);
  }

  async readOfficialProfile(request: OfficialProfileReadRequest, signal?: AbortSignal): Promise<OfficialProfileReadResponse> {
    return this.postConnection<OfficialProfileReadResponse>("profiles/official-analysis", request, signal);
  }

  async compareConnectedProfiles(request: ConnectedProfileComparisonRequest, signal?: AbortSignal): Promise<ConnectedProfileComparisonResponse> {
    return this.postConnection<ConnectedProfileComparisonResponse>("profiles/compare", request, signal);
  }

  /**
   * Read-only `SHOW PLAN ADVISOR` projection for one ready session. The guide
   * is FE-local and experimental; it is displayed, never fed to product rules.
   */
  async readAdvisorGuides(request: PlanAdvisorRequest, signal?: AbortSignal): Promise<PlanAdvisorResponse> {
    const response = await this.request(`${this.base}/connections/${encodeURIComponent(request.connection_id)}/advisor-guides`, {
      headers: { Accept: "application/json" },
      signal,
    });
    const body = await responseJson(response);
    if (!response.ok) throw asFailure(body);
    if (!isRecord(body) || typeof body.capability !== "string" || !Array.isArray(body.recommendations) || !Array.isArray(body.limitations)) throw asFailure(null);
    return body as PlanAdvisorResponse;
  }

  async explain(request: ExplainRequest, signal?: AbortSignal): Promise<ExplainResponse> {
    return this.postConnection<ExplainResponse>("explain", request, signal);
  }

  async explainCosts(request: ExplainRequest, signal?: AbortSignal): Promise<ExplainResponse> {
    return this.explain(request, signal);
  }

  async analyzeRuntimeProfile(request: RuntimeProfileRequest, signal?: AbortSignal): Promise<ProfileFetchResponse> {
    return this.postConnectionExecution("runtime-profile", request, signal, value => {
      const response = executionProfileResponse(value);
      const eligibility = isRecord(value) ? value.official_profile_eligibility : null;
      if (!request.execution_attempt_id || !isRecord(eligibility)
        || eligibility.method !== "standard_profile"
        || eligibility.execution_attempt_id !== request.execution_attempt_id
        || !isRecord(response.query) || response.query.connection_id !== request.connection_id
        || typeof response.query.query_id !== "string" || !response.query.query_id
        || response.query.discovery_generation !== null || response.query.discovery_source !== null) {
        throw unknownExecutionResponse();
      }
      return response;
    });
  }

  async exportReport(request: ExportRequest): Promise<ReportDownload> {
    if (!this.nonce) await this.capabilities();
    const response = await this.request(`${this.base}/exports`, {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json", "X-Doctor-CSRF": this.nonce! },
      body: JSON.stringify(request),
    });
    if (!response.ok) throw asFailure(await responseJson(response));
    if (!response.headers.get("content-type")?.toLowerCase().startsWith("application/json")) throw invalidExportResponse();
    const blob = await response.blob();
    let value: unknown;
    try {
      value = JSON.parse(await blob.text());
    } catch {
      throw invalidExportResponse();
    }
    const metadata = exportMetadata(value, request.kind);
    return { blob, filename: "sr-doctor-report.json", ...metadata };
  }

  private postConnection<T>(path: string, request: { connection_id: string }, signal?: AbortSignal): Promise<T> {
    return this.postJson<T>(`/connections/${encodeURIComponent(request.connection_id)}/${path}`, request, signal);
  }

  private postConnectionExecution<T>(path: string, request: { connection_id: string }, signal: AbortSignal | undefined, guard: (value: unknown) => T): Promise<T> {
    return this.postExecution(`/connections/${encodeURIComponent(request.connection_id)}/${path}`, JSON.stringify(request), { "Content-Type": "application/json" }, signal, guard);
  }

  private async postExecution<T>(path: string, body: BodyInit, headers: HeadersInit, signal: AbortSignal | undefined, guard: (value: unknown) => T): Promise<T> {
    if (!this.nonce) await this.capabilities(signal);
    if (!this.nonce) throw asFailure(null);
    try {
      return guard(await this.post<unknown>(path, body, headers, signal));
    } catch (error) {
      if (isRecord(error) && "recoverability" in error && error.recoverability !== "retry_same") throw error;
      throw unknownExecutionResponse();
    }
  }

  private async post<T>(path: string, body: BodyInit, headers: HeadersInit = {}, signal?: AbortSignal): Promise<T> {
    if (!this.nonce) await this.capabilities(signal);
    const response = await this.request(`${this.base}${path}`, {
      method: "POST",
      headers: { Accept: "application/json", "X-Doctor-CSRF": this.nonce!, ...headers },
      body,
      signal,
    });
    const result = await responseJson(response);
    if (!response.ok) throw asFailure(result);
    return result as T;
  }
}

export const doctorApi = new DoctorApi();
