import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import type {
  AnalysisSnapshotReport,
  ConnectedProfileComparisonRequest,
  ConnectedProfileComparisonResponse,
  ConnectedQueryListRequest,
  ConnectedQueryListResponse,
  ConnectedQueryRef,
  ConnectedQuerySqlResponse,
  ConnectionSessionView,
  ExplainRequest,
  ProfileFetchResponse,
  QueryListStatementFilterCoverage,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, InfoTip } from "../../design";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { localizedEnum, lookupEnumLabel } from "../../i18n/enum-label";
import { limitationSummary } from "../../i18n/limitation-text";
import { LimitationNotes } from "../shell/LimitationNotes";
import { SqlText, SqlToggleButton } from "../shell/SqlText";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { durationNanosecondsText } from "../../lib/duration-display";
import { compactQueryId } from "../../lib/query-id";
import "./queries.css";

type QueryApi = Pick<DoctorApi, "compareConnectedProfiles" | "fetchConnectedProfile" | "fetchConnectedQuerySql" | "searchConnectedQueries">;

type QueriesWorkspaceProps = {
  api?: QueryApi;
  initialFetch?: ProfileFetchResponse | null;
  initialResponse?: ConnectedQueryListResponse | null;
  onConnect(): void;
  onImportProfile?(): void;
  onOpenCompare?(response: ConnectedProfileComparisonResponse): void;
  onOpenDoctor?(profile: ProfileFetchResponse): void;
  onOpenPlanCheck?(request: ExplainRequest): void;
  onDiscoveryChanged?(): void;
  onSessionExpired?(): void;
  session: ConnectionSessionView | null;
};

export function QueriesWorkspace({
  api = doctorApi,
  initialFetch = null,
  initialResponse = null,
  onConnect,
  onImportProfile,
  onOpenCompare,
  onOpenDoctor,
  onOpenPlanCheck,
  onDiscoveryChanged,
  onSessionExpired,
  session,
}: QueriesWorkspaceProps): JSX.Element {
  const { text } = useLocale();
  const [response, setResponse] = useState<ConnectedQueryListResponse | null>(initialResponse);
  const [selectedKey, setSelectedKey] = useState(initialResponse?.queries[0] ? queryRowKey(initialResponse.queries[0], 0) : null);
  const [status, setStatus] = useState("finished");
  const [digest, setDigest] = useState("");
  const [hideOwnStatements, setHideOwnStatements] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fetchingId, setFetchingId] = useState<string | null>(null);
  const [fetchResult, setFetchResult] = useState<ProfileFetchResponse | null>(initialFetch);
  const [baselineQuery, setBaselineQuery] = useState<ConnectedQueryRef | null>(null);
  const [candidateQuery, setCandidateQuery] = useState<ConnectedQueryRef | null>(null);
  const [comparing, setComparing] = useState(false);
  const [compareError, setCompareError] = useState<string | null>(null);
  const [querySql, setQuerySql] = useState<ConnectedQuerySqlResponse | null>(null);
  const [querySqlError, setQuerySqlError] = useState<string | null>(null);
  const [querySqlLoading, setQuerySqlLoading] = useState(false);
  const autoLoadedConnection = useRef<string | null>(null);
  const requestGeneration = useRef(0);
  const requestControllers = useRef(new Set<AbortController>());

  const invalidateRequests = useCallback(() => {
    requestGeneration.current += 1;
    for (const controller of requestControllers.current) controller.abort();
    requestControllers.current.clear();
  }, []);

  const beginRequest = useCallback(() => {
    for (const controller of requestControllers.current) controller.abort();
    requestControllers.current.clear();
    // Every action that supersedes the in-flight request also clears its busy state, the SQL read included.
    setLoading(false);
    setFetchingId(null);
    setComparing(false);
    setQuerySqlLoading(false);
    const controller = new AbortController();
    requestControllers.current.add(controller);
    return { controller, generation: ++requestGeneration.current };
  }, []);

  const requestIsCurrent = useCallback((generation: number, controller: AbortController) => (
    isCurrentRequest(requestGeneration.current, generation, controller.signal)
  ), []);

  const clearQuerySql = useCallback(() => {
    setQuerySql(null);
    setQuerySqlError(null);
    setQuerySqlLoading(false);
  }, []);

  const selected = useMemo(
    () => response?.queries.find((query, index) => queryRowKey(query, index) === selectedKey) ?? response?.queries[0] ?? null,
    [response, selectedKey],
  );

  const load = useCallback(async () => {
    if (!session || session.state !== "ready") return;
    const { controller, generation } = beginRequest();
    setLoading(true);
    setError(null);
    clearQuerySql();
    try {
      const next = await api.searchConnectedQueries(queryListRequest(session.connection_id, status, digest, hideOwnStatements), controller.signal);
      if (!requestIsCurrent(generation, controller)) return;
      onDiscoveryChanged?.();
      setResponse(next);
      setSelectedKey((current) => next.queries.some((query, index) => queryRowKey(query, index) === current)
        ? current
        : next.queries[0] ? queryRowKey(next.queries[0], 0) : null);
      setFetchResult(null);
      setBaselineQuery(null);
      setCandidateQuery(null);
      setCompareError(null);
    } catch (cause) {
      if (requestIsCurrent(generation, controller)) {
        const label = errorLabel(cause);
        setError(label);
        if (isConnectionExpiredFailure(cause)) onSessionExpired?.();
      }
    } finally {
      requestControllers.current.delete(controller);
      if (requestIsCurrent(generation, controller)) setLoading(false);
    }
  }, [api, beginRequest, clearQuerySql, digest, hideOwnStatements, onDiscoveryChanged, onSessionExpired, requestIsCurrent, session, status]);

  useEffect(() => {
    invalidateRequests();
    autoLoadedConnection.current = null;
    setResponse(initialResponse);
    setSelectedKey(initialResponse?.queries[0] ? queryRowKey(initialResponse.queries[0], 0) : null);
    setFetchResult(initialFetch);
    setBaselineQuery(null);
    setCandidateQuery(null);
    setCompareError(null);
    clearQuerySql();
    setError(null);
    setLoading(false);
    setFetchingId(null);
    setComparing(false);
  }, [clearQuerySql, initialFetch, initialResponse, invalidateRequests, session?.connection_id, session?.state]);

  useEffect(() => {
    if (initialResponse || !session || session.state !== "ready" || autoLoadedConnection.current === session.connection_id) return;
    autoLoadedConnection.current = session.connection_id;
    void load();
  }, [initialResponse, load, session]);

  useEffect(() => () => invalidateRequests(), [invalidateRequests]);

  async function fetchProfile(): Promise<void> {
    if (!session || !selected || selected.discovery_generation === null || selected.profile_fetch.state !== "available") return;
    const { controller, generation } = beginRequest();
    setFetchingId(selected.query_id);
    setError(null);
    try {
      const result = await api.fetchConnectedProfile({
        connection_id: session.connection_id,
        query_id: selected.query_id,
        discovery_generation: selected.discovery_generation,
        kind: "baseline",
      }, controller.signal);
      if (requestIsCurrent(generation, controller)) setFetchResult(result);
    } catch (cause) {
      if (requestIsCurrent(generation, controller)) {
        const label = errorLabel(cause);
        setError(label);
        if (isConnectionExpiredFailure(cause)) onSessionExpired?.();
      }
    } finally {
      requestControllers.current.delete(controller);
      if (requestIsCurrent(generation, controller)) setFetchingId(null);
    }
  }

  async function viewQuerySql(): Promise<void> {
    if (!session || session.state !== "ready" || !selected || selected.connection_id !== session.connection_id || selected.discovery_generation === null || querySqlLoading) return;
    if (querySql?.query_id === selected.query_id && querySql.discovery_generation === selected.discovery_generation) {
      clearQuerySql();
      return;
    }
    const { controller, generation } = beginRequest();
    setQuerySql(null);
    setQuerySqlError(null);
    setQuerySqlLoading(true);
    try {
      const result = await api.fetchConnectedQuerySql({
        connection_id: session.connection_id,
        query_id: selected.query_id,
        discovery_generation: selected.discovery_generation,
      }, controller.signal);
      if (requestIsCurrent(generation, controller)) setQuerySql(result);
    } catch (cause) {
      if (requestIsCurrent(generation, controller)) {
        const label = errorLabel(cause);
        setQuerySqlError(label);
        if (isConnectionExpiredFailure(cause)) onSessionExpired?.();
      }
    } finally {
      requestControllers.current.delete(controller);
      if (requestIsCurrent(generation, controller)) setQuerySqlLoading(false);
    }
  }

  function setComparisonSlot(side: "baseline" | "candidate"): void {
    if (!session || session.state !== "ready" || !selected || selected.connection_id !== session.connection_id || selected.profile_fetch.state !== "available") return;
    invalidateRequests();
    setLoading(false);
    setFetchingId(null);
    setComparing(false);
    setQuerySqlLoading(false);
    const other = side === "baseline" ? candidateQuery : baselineQuery;
    if (other && sameQueryId(other.query_id, selected.query_id)) {
      setCompareError("connected_comparison_query_ids_must_differ");
      return;
    }
    if (side === "baseline") setBaselineQuery(selected);
    else setCandidateQuery(selected);
    setCompareError(null);
  }

  async function compareSelectedProfiles(): Promise<void> {
    const request = connectedComparisonRequest(session, baselineQuery, candidateQuery);
    if (!request || comparing) return;
    const { controller, generation } = beginRequest();
    setComparing(true);
    setCompareError(null);
    try {
      const result = await api.compareConnectedProfiles(request, controller.signal);
      if (requestIsCurrent(generation, controller)) onOpenCompare?.(result);
    } catch (cause) {
      if (requestIsCurrent(generation, controller)) {
        const label = errorLabel(cause);
        setCompareError(label);
        if (isConnectionExpiredFailure(cause)) onSessionExpired?.();
      }
    } finally {
      requestControllers.current.delete(controller);
      if (requestIsCurrent(generation, controller)) setComparing(false);
    }
  }

  if (!session) {
    return <section className="card card--detail queries-empty">
      <div className="card-bd state-box">
        <strong>{text("Connect to list recent queries and their Profiles", "连接 StarRocks 后可列出最近的查询及其 Profile")}</strong>
        <p>{text("Only fixed, limited list reads are sent; no SQL is accepted here.", "这里只发送固定且有上限的列表读取，不接受任何 SQL。")}</p>
        <div className="queries-empty-actions">
          <Button onClick={onConnect} size="sm" variant="primary">{text("Connect StarRocks", "连接 StarRocks")}</Button>
          {onImportProfile && <Button onClick={onImportProfile} size="sm" variant="secondary">{text("Or import a Profile file", "或导入 Profile 文件")}</Button>}
        </div>
      </div>
    </section>;
  }

  const ready = session.state === "ready";
  return <div className="queries-workspace">
    <ConnectionEvidence session={session} />

    <div className="workbench-split queries-split">
      <section aria-busy={loading} className="card card--table query-browser" aria-labelledby="query-browser-title">
        <header className="card-hd query-filters-head">
          <div className="card-title-group">
            <h2 className="card-title" id="query-browser-title">{text("Query discovery", "查询发现")}</h2>
            <span className="card-subtitle">{text("Source", "来源")} · {sourceLabel(response?.source, text)}</span>
          </div>
          <form className="card-hd-actions query-filters" onSubmit={(event) => { event.preventDefault(); void load(); }}>
            <div className="filter-chip-input"><label htmlFor="q-filter-status">{text("Status", "状态")}</label><input disabled={!ready || loading} id="q-filter-status" onChange={(event) => setStatus(event.target.value)} size={8} value={status} /></div>
            <div className="filter-chip-input"><label htmlFor="q-filter-digest">{text("Digest", "指纹")}</label><input disabled={!ready || loading} id="q-filter-digest" onChange={(event) => setDigest(event.target.value)} placeholder={text("prefix or contains", "前缀或包含")} size={9} value={digest} /></div>
            <label className="check-inline query-statement-filter"><input checked={hideOwnStatements} disabled={!ready || loading} onChange={(event) => setHideOwnStatements(event.target.checked)} type="checkbox" /><span>{text("Hide this tool's queries and non-query statements", "隐藏本工具查询与非查询语句")}</span></label>
            <Button disabled={!ready || loading} onClick={() => { setStatus("finished"); setDigest(""); setHideOwnStatements(true); }} size="sm" variant="ghost">{text("Clear", "清除")}</Button>
            <Button disabled={!ready || loading} size="sm" type="submit" variant="secondary">{loading ? text("Refreshing…", "刷新中…") : text("Refresh", "刷新")}</Button>
          </form>
        </header>
        <div className="card-bd card-bd--flush query-browser-body">
          {response && <StatementFilterEvidence coverage={response.statement_filter} text={text} />}
          {error && <div className="notice-banner notice-banner--danger query-error" role="alert">{error}</div>}
          <QueryTable onSelect={(rowKey) => { invalidateRequests(); setLoading(false); setSelectedKey(rowKey); setFetchResult(null); setFetchingId(null); setComparing(false); setCompareError(null); clearQuerySql(); }} queries={response?.queries ?? []} selectedKey={selected ? queryRowKey(selected, response?.queries.indexOf(selected) ?? 0) : null} />
          {!loading && response && response.queries.length === 0 && <p className="state-box queries-empty-row">{text("No matching queries were returned.", "没有返回匹配的查询。")}</p>}
          {!ready && <p className="state-box queries-empty-row">{text("Version-sensitive query discovery is unavailable for this connection state:", "当前连接状态无法执行版本敏感的查询发现：")} {localizedEnum(session.state, text)}</p>}
        </div>
        <footer className="card-ft query-receipt">
          <span>{text("100 by default · up to 500", "默认 100 条 · 最多 500 条")} · {text("Actual source / scope:", "实际来源 / 范围：")} {sourceLabel(response?.source, text)} · {response ? localizedEnum(response.filter_scope, text) : text("not loaded", "未加载")}</span>
          {response?.limitations.length ? <LimitationNotes codes={response.limitations} label={text("Source limitations", "来源限制")} text={text} /> : <span>{text("No source limitation reported", "未报告来源限制")}</span>}
        </footer>
      </section>

      <SelectedQuery
        fetchResult={fetchResult}
        fetching={fetchingId === selected?.query_id}
        baselineQuery={baselineQuery}
        candidateQuery={candidateQuery}
        compareError={compareError}
        comparing={comparing}
        onFetch={() => void fetchProfile()}
        onViewSql={() => void viewQuerySql()}
        onCompare={() => void compareSelectedProfiles()}
        onOpenDoctor={onOpenDoctor}
        onOpenPlanCheck={onOpenPlanCheck}
        query={selected}
        querySql={querySql}
        querySqlError={querySqlError}
        querySqlLoading={querySqlLoading}
        ready={ready}
        onSetSlot={setComparisonSlot}
        canCompare={Boolean(onOpenCompare) && connectedComparisonRequest(session, baselineQuery, candidateQuery) !== null}
      />
    </div>
  </div>;
}

function ConnectionEvidence({ session }: { session: ConnectionSessionView }): JSX.Element {
  const { text } = useLocale();
  const facts = [
    [text("Session", "会话"), shortId(session.connection_id), `${Math.ceil(session.idle_timeout_seconds / 60)}${text("m idle timeout", " 分钟空闲时限")}`],
    [text("Detected", "检测版本"), session.detected_version?.value ?? "unknown", localizedEnum(session.state, text)],
    [text("Database time zone", "数据库时区"), session.time_zone?.effective_time_zone ?? "unknown", session.time_zone ? formatUtcOffset(session.time_zone.current_utc_offset_seconds) : text("probe unavailable", "探测不可用")],
  ];
  return <details className="disclosure connection-details"><summary>{text("Connection details", "连接详情")} · {session.detected_version?.value ?? text("version unknown", "版本未知")} · {session.time_zone?.effective_time_zone ?? text("time zone unknown", "时区未知")}</summary><section className="disclosure-body connection-evidence" aria-label={text("Connection capability evidence", "连接能力证据")}>
    <div className="connection-evidence-grid">
      {facts.map(([label, value, detail]) => <div key={label}><span className="muted">{label}</span> <strong className="mono">{value}</strong> <small className="muted">{detail}</small></div>)}
      {Object.entries(session.capabilities).map(([name, state]) => <div key={name}><span className="muted" title={name}>{capabilityLabel(name, text)}</span> <span className="status-inline"><span className={state === "available" ? "status-dot status-dot--ok" : state === "forbidden" ? "status-dot status-dot--critical" : "status-dot status-dot--hollow"} /><strong>{localizedEnum(state, text)}</strong></span></div>)}
    </div>
    <LimitationNotes codes={session.limitations} text={text} />
  </section></details>;
}

export function queryListRequest(connectionId: string, status: string, digest: string, hideOwnStatements: boolean): ConnectedQueryListRequest {
  return {
    connection_id: connectionId,
    limit: 100,
    status: status.trim() || null,
    digest: digest.trim() || null,
    statement_filter: hideOwnStatements ? "exclude_doctor_and_non_query" : "all",
  };
}

// Renders only what the backend reports; counts stay the server's exact decimals.
function StatementFilterEvidence({ coverage, text }: { coverage: QueryListStatementFilterCoverage; text: LocalizedText }): JSX.Element {
  const message = coverage.state === "applied"
    ? text(`Hidden: ${coverage.hidden_doctor_row_count} statements sent by this tool and ${coverage.hidden_non_query_row_count} non-query statements (raw audit rows in the window, not limited by the list size).`, `已隐藏本工具发出的语句 ${coverage.hidden_doctor_row_count} 条、非查询语句 ${coverage.hidden_non_query_row_count} 条（时间窗内的原始审计行，不受列表条数上限影响）。`)
    : coverage.state === "doctor_only_applied"
    ? text(`Hidden: ${coverage.hidden_doctor_row_count} statements sent by this tool among the Profile list rows read. The Profile list has no statement-type column, so non-query statements cannot be told apart and stay listed.`, `已隐藏本工具发出的语句 ${coverage.hidden_doctor_row_count} 条（本次读取的 Profile 列表行内）。Profile 列表没有语句类型列，无法区分非查询语句，它们仍会列出。`)
    : coverage.state === "indistinguishable"
    ? `${text("This tool's statements cannot be told apart here, so nothing was hidden.", "此来源无法区分本工具发出的语句，未隐藏任何行。")} ${statementFilterReason(coverage.reason, text)}`
    : text("All statements are shown; nothing was hidden.", "显示全部语句，未隐藏任何行。");
  return <p className="query-statement-filter-evidence" data-query-statement-filter={coverage.state}>{message}</p>;
}

function statementFilterReason(reason: string, text: LocalizedText): string {
  if (reason === "audit_statement_filter_unavailable") return text("The audit table does not expose the isQuery and stmt columns this filter needs.", "审计表缺少该筛选所需的 isQuery 与 stmt 列。");
  return reason;
}

function formatUtcOffset(offsetSeconds: number): string {
  const sign = offsetSeconds < 0 ? "-" : "+";
  const absolute = Math.abs(offsetSeconds);
  const hours = Math.floor(absolute / 3600).toString().padStart(2, "0");
  const minutes = Math.floor((absolute % 3600) / 60).toString().padStart(2, "0");
  return `UTC${sign}${hours}:${minutes}`;
}

function QueryTable({ onSelect, queries, selectedKey }: { onSelect(rowKey: string): void; queries: ConnectedQueryRef[]; selectedKey: string | null }): JSX.Element {
  const { text } = useLocale();
  return <div className="query-table-wrap"><table aria-label={text("Available queries", "可用查询")} className="data-table query-table">
    <thead><tr><th>{text("Started", "开始时间")}</th><th>{text("Query ID", "查询 ID")}</th><th>{text("Source", "来源")}</th><th>{text("State", "状态")}</th><th>{text("Database", "数据库")}</th><th>Digest</th><th>{text("Duration", "耗时")}</th><th>{text("Resource group", "资源组")}</th><th>Profile</th></tr></thead>
    <tbody>{queries.map((query, index) => {
      const rowKey = queryRowKey(query, index);
      const selected = rowKey === selectedKey;
      const dotClass = query.status === "finished" ? "status-dot status-dot--ok" : query.status === "error" ? "status-dot status-dot--critical" : query.status === "running" ? "status-dot status-dot--warn" : "status-dot status-dot--hollow";
      return <tr className={`query-row${selected ? " is-selected" : ""}`} key={rowKey} onClick={() => { if (!selected) onSelect(rowKey); }}>
      <td><button aria-pressed={selected} aria-label={`${text("Select query", "选择查询")} ${query.query_id}`} className="row-pick-btn mono" onClick={(event) => { event.stopPropagation(); onSelect(rowKey); }} title={localizedEnum(query.timestamp_state, text)} type="button">{observedTimeLabel(query)}</button></td>
      <td title={query.query_id}>{compactQueryId(query.query_id)}</td>
      <td>{sourceLabel(query.discovery_source, text)}</td>
      <td><span className="status-inline"><span className={dotClass} />{localizedEnum(query.status, text)}</span></td>
      <td>{query.database ?? "—"}</td>
      <td title={query.digest ?? "unavailable"}>{query.digest ? shortId(query.digest) : "—"}</td>
      <td>{queryDurationText(query.query_time_ms)}</td>
      <td>{query.resource_group ?? "—"}</td>
      <td>{query.profile_fetch.state === "available" ? <span className="status-inline"><span className="status-dot status-dot--ok" />{profileAvailabilityLabel(query, text)}</span> : <span className="state-stamp">{profileAvailabilityLabel(query, text)}</span>}</td>
    </tr>;
    })}</tbody>
  </table></div>;
}

function queryDurationText(milliseconds: number | null): string {
  if (milliseconds === null) return "—";
  if (!Number.isSafeInteger(milliseconds) || milliseconds < 0) return `${milliseconds} ms`;
  return durationNanosecondsText((BigInt(milliseconds) * 1_000_000n).toString())!;
}

/** Local UI identity is row evidence, not canonical Query ID: duplicate IDs remain independently inspectable. */
export function queryRowKey(query: ConnectedQueryRef, index: number): string {
  return `${query.query_id}:${query.source_refs[0] ?? `retained-row:${index}`}`;
}

function SelectedQuery({ baselineQuery, candidateQuery, canCompare, compareError, comparing, fetchResult, fetching, onCompare, onFetch, onOpenDoctor, onOpenPlanCheck, onSetSlot, onViewSql, query, querySql, querySqlError, querySqlLoading, ready }: {
  baselineQuery: ConnectedQueryRef | null;
  candidateQuery: ConnectedQueryRef | null;
  canCompare: boolean;
  compareError: string | null;
  comparing: boolean;
  fetchResult: ProfileFetchResponse | null;
  fetching: boolean;
  onCompare(): void;
  onFetch(): void;
  onOpenDoctor?: (profile: ProfileFetchResponse) => void;
  onOpenPlanCheck?: (request: ExplainRequest) => void;
  onSetSlot(side: "baseline" | "candidate"): void;
  onViewSql(): void;
  query: ConnectedQueryRef | null;
  querySql: ConnectedQuerySqlResponse | null;
  querySqlError: string | null;
  querySqlLoading: boolean;
  ready: boolean;
}): JSX.Element {
  const { text } = useLocale();
  const doctorProfile = profileForSelectedQuery(query, fetchResult);
  const doctorReport = doctorProfile?.outcome.report ?? null;
  const fetchAvailable = query?.profile_fetch.state === "available";
  const planAvailable = query?.plan_check.state === "available" && query.discovery_generation !== null;
  const visibleSql = querySql && query && querySql.query_id === query.query_id && querySql.discovery_generation === query.discovery_generation ? querySql.sql : null;
  const unavailable = text("unavailable", "不可用");
  // Display kind only: "mono" for machine text, "missing" for an absent value (dashed tag, same text as before).
  const fact = (label: string, value: string | null | undefined, missing: string, kind: "mono" | "text" = "mono"): EvidenceFact => value === null || value === undefined ? [label, missing, "missing"] : [label, value, kind];
  const queryFacts: readonly EvidenceFact[] = [
    fact(text("Evidence source", "证据来源"), query ? sourceLabel(query.discovery_source, text) : null, unavailable, "text"),
    fact(text("Query ID", "查询 ID"), query?.query_id, unavailable),
    fact("Digest", query?.digest, "unavailable"),
    fact(text("Database", "数据库"), query?.database, unavailable, "text"),
    fact(text("Started", "开始时间"), query ? observedTimeLabel(query) : null, unavailable),
  ];
  const profileFacts: readonly EvidenceFact[] = [
    fact(text("Availability", "可用性"), fetching ? localizedEnum("fetching", text) : fetchResult ? localizedEnum(fetchResult.state, text) : query ? profileAvailabilityLabel(query, text) : null, unavailable, "text"),
    fact(text("Source kind", "来源类型"), fetchResult?.source_kind, unavailable),
    fact(text("Analysis key", "分析键"), fetchResult?.analysis_key ? shortId(fetchResult.analysis_key) : null, unavailable),
    [text("Limitations", "限制"), fetchResult?.limitations.length ? limitationSummary(fetchResult.limitations, text) : text("Profile availability is not claimed before fetch.", "拉取前不声明 Profile 可用性。"), "text"],
    fact(text("Baseline slot", "基线槽位"), baselineQuery ? compactQueryId(baselineQuery.query_id) : null, text("not selected", "未选择")),
    fact(text("Candidate slot", "候选槽位"), candidateQuery ? compactQueryId(candidateQuery.query_id) : null, text("not selected", "未选择")),
  ];
  return <section className="card card--detail selected-query" aria-labelledby="selected-query-title">
    {/* Task 69: a fixed title, one scrolling body (facts and SQL), the actions fixed at the bottom. */}
    <header className="card-hd selected-query-head">
      <h2 className="card-title" id="selected-query-title">{text("Selected query evidence", "已选查询证据")}</h2>
      <InfoTip label={text("Comparison boundary", "对比边界")} side="bottom">{text("Candidate Profile must come from a user-side controlled rerun. The product sends only query references and does not execute SQL.", "候选 Profile 必须来自用户侧受控重跑。产品只发送查询引用，不执行 SQL。")}</InfoTip>
    </header>
    <div className="card-bd selected-query-body">
      <EvidenceFacts facts={queryFacts} />
      <div className="selected-query-section"><span className="field-label">{text("Profile evidence", "Profile 证据")}</span><EvidenceFacts facts={profileFacts} /></div>
      {visibleSql !== null && <SqlText className="query-sql" copyLabel={text("Copy SQL", "复制 SQL")} id="selected-query-sql" label={text("Selected query SQL", "已选查询 SQL")} value={visibleSql} />}
      {query?.plan_check.state === "unavailable" ? <p className="connected-action-reason muted">{planAvailabilityLabel(query, text)}</p> : null}
      <p className="connected-compare-boundary muted" role={compareError ? "alert" : undefined}>{compareError ? comparisonErrorLabel(compareError, text) : text("Candidate Profile must come from a user-side controlled rerun. The product sends only query references and does not execute SQL.", "候选 Profile 必须来自用户侧受控重跑。产品只发送查询引用，不执行 SQL。")}</p>
      {querySqlError && <div className="notice-banner notice-banner--danger query-sql-error" role="alert">{querySqlError}</div>}
    </div>
    <footer aria-busy={fetching || comparing || querySqlLoading} className="card-ft selected-query-actions">
      <Button disabled={!ready || !fetchAvailable || fetching} onClick={onFetch} size="sm" title={query ? profileAvailabilityLabel(query, text) : undefined} variant="primary">{fetching ? text("Fetching…", "拉取中…") : text("Fetch profile", "拉取 Profile")}</Button>
      <Button disabled={!doctorProfile || !onOpenDoctor} onClick={() => { if (doctorProfile) onOpenDoctor?.(doctorProfile); }} size="sm" title={doctorProfile && onOpenDoctor ? undefined : text("A fetched report payload for the selected query is required before Doctor can open.", "打开诊断前需要当前所选查询的已拉取报告载荷。")} variant="secondary">{text("Open Doctor", "打开诊断")}</Button>
      <SqlToggleButton className="btn btn-secondary btn--sm" controls="selected-query-sql" disabled={!ready || !query || query.discovery_generation === null} expanded={visibleSql !== null} loading={querySqlLoading} onToggle={onViewSql} />
      <Button disabled={!ready || !planAvailable || !onOpenPlanCheck} onClick={() => {
        if (query && query.discovery_generation !== null && query.plan_check.state === "available") onOpenPlanCheck?.({
          input_kind: "plan_sql_reference",
          connection_id: query.connection_id,
          mode: "costs",
          query_id: query.query_id,
          discovery_generation: query.discovery_generation,
          plan_sql_ref: query.plan_check.plan_sql_ref,
        });
      }} size="sm" title={query ? planAvailabilityLabel(query, text) : undefined} variant="secondary">{text("Plan Check", "计划检查")}</Button>
      <Button disabled={!ready || !fetchAvailable || comparing || Boolean(candidateQuery && query && sameQueryId(candidateQuery.query_id, query.query_id))} onClick={() => onSetSlot("baseline")} size="sm" variant="secondary">{baselineQuery && query && sameQueryId(baselineQuery.query_id, query.query_id) ? text("Baseline selected", "已设为基线") : text("Set baseline", "设为基线")}</Button>
      <Button disabled={!ready || !fetchAvailable || comparing || Boolean(baselineQuery && query && sameQueryId(baselineQuery.query_id, query.query_id))} onClick={() => onSetSlot("candidate")} size="sm" variant="secondary">{candidateQuery && query && sameQueryId(candidateQuery.query_id, query.query_id) ? text("Candidate selected", "已设为候选项") : text("Set candidate", "设为候选项")}</Button>
      <Button disabled={!canCompare || comparing} onClick={onCompare} size="sm" variant="primary">{comparing ? text("Comparing…", "对比中…") : text("Compare selected Profiles", "对比已选 Profile")}</Button>
    </footer>
  </section>;
}

type EvidenceFact = readonly [label: string, value: string, kind: "mono" | "text" | "missing"];

/** dd textContent stays the plain value; the kind only picks mono, sans or a stamp missing tag. */
function EvidenceFacts({ facts }: { facts: readonly EvidenceFact[] }): JSX.Element {
  return <dl className="kv-grid">{facts.map(([label, value, kind]) => <div className="selected-query-fact-row" key={label}><dt>{label}</dt><dd className={`fact-${kind}${kind === "mono" ? " mono" : ""}`}>{kind === "missing" ? <span className="state-stamp fact-missing-tag">{value}</span> : value}</dd></div>)}</dl>;
}

export function reportForSelectedQuery(
  query: ConnectedQueryRef | null,
  fetchResult: ProfileFetchResponse | null,
): AnalysisSnapshotReport | null {
  return profileForSelectedQuery(query, fetchResult)?.outcome.report ?? null;
}

export function profileForSelectedQuery(
  query: ConnectedQueryRef | null,
  fetchResult: ProfileFetchResponse | null,
): ProfileFetchResponse | null {
  if (!query || query.discovery_generation === null || fetchResult?.state !== "fetched") return null;
  if (fetchResult.query.connection_id !== query.connection_id || fetchResult.query.query_id !== query.query_id || fetchResult.query.discovery_generation !== query.discovery_generation) return null;
  return fetchResult;
}

export function connectedComparisonRequest(
  session: ConnectionSessionView | null,
  baseline: ConnectedQueryRef | null,
  candidate: ConnectedQueryRef | null,
): ConnectedProfileComparisonRequest | null {
  if (!session || session.state !== "ready" || !baseline || !candidate) return null;
  if (baseline.connection_id !== session.connection_id || candidate.connection_id !== session.connection_id || sameQueryId(baseline.query_id, candidate.query_id)) return null;
  if (baseline.profile_fetch.state !== "available" || candidate.profile_fetch.state !== "available") return null;
  if (baseline.discovery_generation === null || candidate.discovery_generation === null || baseline.discovery_generation !== candidate.discovery_generation) return null;
  return {
    connection_id: session.connection_id,
    baseline_query_id: baseline.query_id,
    candidate_query_id: candidate.query_id,
    discovery_generation: baseline.discovery_generation,
  };
}

function sameQueryId(left: string, right: string): boolean {
  return left.toLowerCase() === right.toLowerCase();
}

export function isCurrentRequest(activeGeneration: number, requestGeneration: number, signal: AbortSignal): boolean {
  return activeGeneration === requestGeneration && !signal.aborted;
}

function comparisonErrorLabel(error: string, text: LocalizedText): string {
  if (error === "connected_comparison_query_ids_must_differ") return text("Choose two different query IDs before comparing.", "请先选择两个不同的查询 ID 再进行对比。");
  return error;
}

function sourceLabel(source: ConnectedQueryListResponse["source"] | ConnectedQueryRef["discovery_source"] | undefined, text: LocalizedText): string {
  if (source === "audit") return "AuditLoader";
  if (source === "profile_list") return "ProfileList";
  return text("not loaded", "未加载");
}

function formatObservedAt(value: string): string {
  const match = value.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})/);
  return match ? `${match[1]} ${match[2]}` : value;
}

function observedTimeLabel(query: ConnectedQueryRef): string {
  if (query.observed_at_utc) return formatObservedAt(query.observed_at_utc);
  return query.observed_at_local ?? "—";
}

const PROFILE_AVAILABILITY_REASONS: Record<string, readonly [string, string]> = {
  query_id_invalid: ["Invalid query ID", "查询 ID 无效"],
  query_id_duplicate: ["Duplicate query ID", "查询 ID 重复"],
  query_not_finished: ["Query not finished", "查询尚未完成"],
};

function profileAvailabilityLabel(query: ConnectedQueryRef, text: LocalizedText): string {
  if (query.profile_fetch.state === "available") return text("Available to fetch", "可尝试拉取");
  return lookupEnumLabel(PROFILE_AVAILABILITY_REASONS, query.profile_fetch.reason, text);
}

const PLAN_AVAILABILITY_REASONS: Record<string, readonly [string, string]> = {
  statement_missing_or_not_select: ["Statement is missing or is not a single SELECT/WITH", "语句缺失或不是单条 SELECT/WITH"],
  query_metadata_unavailable: ["Query metadata is unavailable", "查询元数据不可用"],
  query_id_invalid: ["Invalid query ID", "查询 ID 无效"],
  query_id_duplicate: ["Duplicate query ID", "查询 ID 重复"],
};

function planAvailabilityLabel(query: ConnectedQueryRef, text: LocalizedText): string {
  if (query.plan_check.state === "available") return text("Available for Plan Check", "可执行计划检查");
  return lookupEnumLabel(PLAN_AVAILABILITY_REASONS, query.plan_check.reason, text);
}

function shortId(value: string): string {
  return value.length > 18 ? `${value.slice(0, 16)}…` : value;
}

function errorLabel(cause: unknown): string {
  if (typeof cause === "object" && cause !== null && "message_id" in cause) {
    const limitation = "details" in cause && typeof cause.details === "object" && cause.details !== null && "limitation" in cause.details
      ? String(cause.details.limitation)
      : null;
    return limitation ?? String(cause.message_id);
  }
  return cause instanceof Error ? cause.message : "request_failed";
}

export function isConnectionExpiredFailure(cause: unknown): boolean {
  if (typeof cause === "object" && cause !== null) {
    const failure = cause as { code?: unknown; message_id?: unknown };
    return failure.code === "connection_expired"
      || failure.message_id === "connection_expired"
      || failure.message_id === "connection_session_expired";
  }
  return cause instanceof Error
    && (cause.message === "connection_expired" || cause.message === "connection_session_expired");
}

const capabilityLabels: Record<string, readonly [string, string]> = {
  version_probe: ["Version check", "版本探测"],
  time_zone: ["Time zone", "时区"],
  audit: ["Audit log", "审计日志"],
  audit_user_filter: ["Audit user filter", "审计按用户筛选"],
  audit_statement_filter: ["Audit statement filter", "审计按语句类型筛选"],
  profile_list: ["Profile list", "Profile 列表"],
  profile_fetch: ["Profile fetch", "Profile 拉取"],
  explain_costs: ["EXPLAIN COSTS", "EXPLAIN COSTS"],
  plan_advisor: ["Plan Advisor", "Plan Advisor"],
};

function capabilityLabel(name: string, text: LocalizedText): string {
  return lookupEnumLabel(capabilityLabels, name, text);
}
