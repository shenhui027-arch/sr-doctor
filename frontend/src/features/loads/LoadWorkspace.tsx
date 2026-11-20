// Design board 07-loads: live load overview (with Routine Load merged in, decided item 2),
// live load records, and 30-day local SQLite history. At most two navigation levels:
// view SegTabs + inline load-type select in `.workspace-bar`. No CardDeck.
import { useEffect, useId, useRef, useState } from "react";
import type { JSX } from "react";
import type {
  LoadGroupSummary,
  LoadKindFilter,
  LoadKindGroup,
  LoadRecord,
  LoadRecordsResponse,
  RoutineLoadJob,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, InfoTip, PageGrid, SegTabs } from "../../design";
import { useLocale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { useMonitorRead } from "../history/monitor-read";
import { MonitorReadError, RefreshBar } from "../history/MonitorStates";
import { LoadErrorLogDialog, type ErrorLogTarget } from "./LoadErrorLogDialog";
import { LoadHistoryPanel } from "./LoadHistoryPanel";
import { LoadOverviewPanel } from "./LoadOverviewPanel";
import { LoadRecordsPanel, type RecordFilters } from "./LoadRecordsPanel";
import { count, feTime, isLoadSessionExpired, loadFailureText } from "./load-text";
import "./loads.css";

export { isLoadSessionExpired };

const REFRESH_MS = 30_000;
export type PrimaryLoadView = "overview" | "records" | "history";

/** Where another workspace asks the load workspace to open (`view: "routine"` maps to Overview + Routine Load). */
export type LoadFocus =
  | { view: "overview" }
  | { view: "routine"; jobId: string }
  | {
      view: "history";
      target: string | null;
      table: string | null;
      kind: LoadKindGroup | null;
      jobId: string | null;
      errors: boolean;
    };

const NO_RECORD_FILTERS: RecordFilters = {
  kind: null,
  outcome: null,
  table: "",
  include_system: false,
  hour: null,
};

type TypeCounts = {
  connectionId: string;
  groups: LoadGroupSummary[];
  all: string;
  records: boolean;
  routine: boolean;
  jobs: string;
};

/** Pure predicate tested by unit tests: live 30s polling runs ONLY on an active, non-expired live view. */
export function shouldPollLiveLoads(
  connectionId: string | null,
  sessionExpired: boolean,
  view: PrimaryLoadView,
): boolean {
  return Boolean(connectionId) && !sessionExpired && view !== "history";
}

function initialViewOf(focus?: LoadFocus | null): PrimaryLoadView {
  if (focus?.view === "history") return "history";
  return "overview";
}

function initialKindOf(focus?: LoadFocus | null): LoadKindFilter | null {
  if (focus?.view === "routine") return "routine_load";
  if (focus?.view === "history" && !focus.errors && focus.kind && focus.kind !== "other") {
    return focus.kind;
  }
  return null;
}

export function LoadWorkspace({
  connectionId,
  focus = null,
  sessionExpired: externalSessionExpired = false,
  onPaused,
  onOpenQueries,
  onConnect,
  onSessionExpired,
  onOpenAlerts,
}: {
  /** The ready session; null shows how to connect while still allowing 30-day local history. */
  connectionId: string | null;
  focus?: LoadFocus | null;
  /** Optional host-controlled session-expired flag (in addition to live API detection). */
  sessionExpired?: boolean;
  /** Paused job count for the header badge. */
  onPaused(count: number): void;
  onOpenQueries(): void;
  onConnect?(): void;
  onSessionExpired?(): void;
  onOpenAlerts?(target: string | null): void;
}): JSX.Element {
  const { text } = useLocale();
  const typeSelectId = useId();
  const [view, setView] = useState<PrimaryLoadView>(() => initialViewOf(focus));
  const [kind, setKind] = useState<LoadKindFilter | null>(() => initialKindOf(focus));
  const [includeSystem, setIncludeSystem] = useState(false);
  const [routineJob, setRoutineJob] = useState<string | null>(
    focus?.view === "routine" ? focus.jobId : focus?.view === "history" ? focus.jobId : null,
  );
  const [tick, setTick] = useState(0);
  const [filters, setFilters] = useState<RecordFilters>(NO_RECORD_FILTERS);
  const [applied, setApplied] = useState<RecordFilters>(NO_RECORD_FILTERS);
  const [log, setLog] = useState<ErrorLogTarget | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [expiredForId, setExpiredForId] = useState<string | null>(null);

  const onSessionExpiredRef = useRef(onSessionExpired);
  useEffect(() => {
    onSessionExpiredRef.current = onSessionExpired;
  });

  const markSessionExpired = (forConnectionId: string | null) => {
    if (!forConnectionId) return;
    setExpiredForId((prev) => {
      if (prev !== forConnectionId) {
        onSessionExpiredRef.current?.();
      }
      return forConnectionId;
    });
  };

  const sessionExpired =
    externalSessionExpired || (connectionId !== null && expiredForId === connectionId);
  const pollingActive = shouldPollLiveLoads(connectionId, sessionExpired, view);

  useEffect(() => {
    if (!pollingActive) return;
    const timer = window.setInterval(() => {
      setTick((value) => value + 1);
      setNowMs(Date.now());
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [pollingActive]);

  useEffect(() => {
    const timer = window.setTimeout(() => setApplied(filters), 400);
    return () => window.clearTimeout(timer);
  }, [filters]);

  const guardLive = <T,>(work: () => Promise<T>): Promise<T> =>
    work().catch((cause: unknown) => {
      if (isLoadSessionExpired(cause)) {
        markSessionExpired(connectionId);
      }
      throw cause;
    });

  const liveReady = Boolean(connectionId) && !sessionExpired;
  const effectiveRecordsFilters: RecordFilters = {
    ...applied,
    kind,
    include_system: includeSystem,
  };

  const overview = useMonitorRead(
    liveReady && view === "overview"
      ? (signal) =>
          guardLive(() =>
            doctorApi.readLoadOverview(
              { connection_id: connectionId!, kind, include_system: includeSystem },
              signal,
            ),
          )
      : null,
    `overview:${connectionId ?? ""}:${kind ?? "all"}:${includeSystem}`,
    String(tick),
    "load_overview_failed",
  );

  const routine = useMonitorRead(
    liveReady && view === "overview" && kind === "routine_load"
      ? (signal) => guardLive(() => doctorApi.readRoutineLoadJobs(connectionId!, signal))
      : null,
    `routine:${connectionId ?? ""}`,
    String(tick),
    "routine_load_jobs_failed",
  );

  const records = useMonitorRead(
    liveReady && view === "records"
      ? (signal) =>
          guardLive(() =>
            doctorApi.readLoadRecords(
              {
                connection_id: connectionId!,
                kind: effectiveRecordsFilters.kind,
                outcome: effectiveRecordsFilters.outcome,
                table: effectiveRecordsFilters.table.trim() || null,
                include_system: effectiveRecordsFilters.include_system,
                created_hour: effectiveRecordsFilters.hour,
              },
              signal,
            ),
          )
      : null,
    `records:${connectionId ?? ""}:${JSON.stringify(effectiveRecordsFilters)}`,
    String(tick),
    "load_records_failed",
  );

  useEffect(() => {
    if (
      connectionId &&
      !sessionExpired &&
      (isLoadSessionExpired(overview.error) ||
        isLoadSessionExpired(routine.error) ||
        isLoadSessionExpired(records.error))
    ) {
      markSessionExpired(connectionId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectionId, sessionExpired, overview.error, routine.error, records.error]);

  const [typeCounts, setTypeCounts] = useState<TypeCounts | null>(null);
  useEffect(() => {
    if (!overview.data || !connectionId) return;
    const routineOverview = overview.data.routine;
    setTypeCounts({
      connectionId,
      groups: overview.data.groups,
      all: overview.data.all_loads,
      records: overview.data.support !== "unsupported_version",
      routine: routineOverview.support !== "unsupported_version",
      jobs: routineOverview.jobs,
    });
  }, [overview.data, connectionId]);

  const paused = routine.data
    ? Number(routine.data.summary.paused)
    : overview.data && overview.data.routine.support !== "unsupported_version"
      ? Number(overview.data.routine.paused)
      : null;

  const [shownRecords, setShownRecords] = useState<{ connectionId: string; data: LoadRecordsResponse } | null>(null);
  useEffect(() => {
    if (records.data && connectionId) setShownRecords({ connectionId, data: records.data });
  }, [records.data, connectionId]);
  const recordsView =
    records.data ?? (shownRecords && shownRecords.connectionId === connectionId ? shownRecords.data : null);

  const onPausedRef = useRef(onPaused);
  useEffect(() => {
    onPausedRef.current = onPaused;
  });
  useEffect(() => {
    if (paused !== null) onPausedRef.current(paused);
  }, [paused]);

  const openRecords = (next: Partial<RecordFilters>) => {
    const merged: RecordFilters = { ...NO_RECORD_FILTERS, ...next, kind, include_system: includeSystem };
    setFilters(merged);
    setApplied(merged);
    setView("records");
  };

  const openRoutineJobInOverview = (jobId: string) => {
    setRoutineJob(jobId);
    setKind("routine_load");
    setView("overview");
  };

  const openRoutineLog = (job: RoutineLoadJob) => {
    if (!connectionId) return;
    setLog({
      connectionId,
      jobId: job.job_id,
      title: job.name,
      meta: text(
        `Routine Load · ${job.db}.${job.table} · job_id ${job.job_id} · recent tasks' error logs`,
        `Routine Load · ${job.db}.${job.table} · job_id ${job.job_id} · 最近任务的错误日志（ERROR_LOG_URLS）`,
      ),
    });
  };

  const openRecordLog = (record: LoadRecord) => {
    if (!connectionId) return;
    setLog({
      connectionId,
      jobId: record.job_id,
      title: record.label,
      meta: `${record.kind} · ${record.db}.${record.table} · job_id ${record.job_id} · ${feTime(record.create_time)} · ${record.state}`,
    });
  };

  const countsForSession = typeCounts && typeCounts.connectionId === connectionId ? typeCounts : null;
  const groupCount = (group: string) =>
    countsForSession?.groups.find((item) => item.group === group)?.loads ?? null;

  const currentLiveRead =
    view === "overview"
      ? kind === "routine_load" && routine.loading
        ? routine
        : overview
      : view === "records"
        ? records
        : null;

  const sourceNote =
    view === "overview"
      ? text(
          "Read live every 30 s from information_schema.loads (3 days / ~1000 records per kind) and routine_load_jobs; read-only, never auto-mutated.",
          "每 30 秒只读读取 information_schema.loads（保留约 3 天 / 每类约 1000 条）与 routine_load_jobs；不落盘、不修改作业。",
        )
      : view === "records"
        ? text(
            "Live records from information_schema.loads: Stream Load, Broker Load, INSERT, and Routine Load tasks.",
            "实时读取 information_schema.loads：包含 Stream Load、Broker Load、INSERT 与 Routine Load 提交任务。",
          )
        : text(
            "30-day history from the local SQLite store collected by the single bounded collector; reads nothing live from StarRocks.",
            "来自本机 SQLite 历史库（单一有界采集任务保留 30 天）；此视图不直接读取 StarRocks。",
          );

  const reconnectAction = onConnect ?? onOpenQueries;

  return (
    <section aria-label={text("Load monitoring workspace", "导入监控工作区")} className="workspace-stage load-stage">
      <div className="workspace-bar load-toolbar">
        <div className="load-inline-filters">
          <SegTabs
            aria-label={text("Load views", "导入视图")}
            items={[
              { value: "overview", label: text("Overview", "概览") },
              { value: "records", label: text("Load records", "导入记录") },
              { value: "history", label: text("30-day history", "30 天历史") },
            ]}
            onValueChange={setView}
            size="sm"
            value={view}
          />

          <label className="filter-chip-input" htmlFor={typeSelectId}>
            <span>{text("Type", "类型")}</span>
            <select
              id={typeSelectId}
              onChange={(event) => {
                const nextKind = (event.target.value || null) as LoadKindFilter | null;
                setKind(nextKind);
                if (nextKind !== "routine_load") setRoutineJob(null);
              }}
              value={kind ?? ""}
            >
              <option value="">
                {countsForSession
                  ? `${text("All", "全部")} (${count(countsForSession.all)})`
                  : text("All types", "全部类型")}
              </option>
              <option value="stream_load">
                {groupCount("stream_load") !== null
                  ? `Stream Load (${count(groupCount("stream_load")!)})`
                  : "Stream Load"}
              </option>
              <option value="routine_load">
                {countsForSession
                  ? `Routine Load (${text(`${countsForSession.jobs} jobs`, `${countsForSession.jobs} 作业`)})`
                  : "Routine Load"}
              </option>
              <option value="insert">
                {groupCount("insert") !== null ? `INSERT (${count(groupCount("insert")!)})` : "INSERT"}
              </option>
              <option value="broker">
                {groupCount("broker") !== null ? `Broker Load (${count(groupCount("broker")!)})` : "Broker Load"}
              </option>
            </select>
          </label>

          <InfoTip label={text("Data source & retention", "数据来源与保留窗口")}>
            {sourceNote}
          </InfoTip>
        </div>

        <div className="load-inline-filters">
          <label className="load-check-inline">
            <input
              checked={includeSystem}
              onChange={(event) => setIncludeSystem(event.target.checked)}
              type="checkbox"
            />
            <span>{text("System loads", "含系统任务")}</span>
          </label>
          {currentLiveRead && liveReady ? (
            <RefreshBar
              error={currentLiveRead.error}
              loading={currentLiveRead.loading}
              onRefresh={() => {
                overview.refresh();
                if (kind === "routine_load") routine.refresh();
                if (view === "records") records.refresh();
              }}
              text={text}
              updatedSeconds={currentLiveRead.updatedSeconds}
            />
          ) : null}
        </div>
      </div>

      {sessionExpired ? (
        <div className="notice-banner notice-banner--warn load-session-expired" role="alert">
          <span>
            <strong>{text("Connection session expired.", "连接会话已过期。")}</strong>{" "}
            {text(
              "Live 30-second load polling has stopped. Reconnect to inspect live loads, or view the 30-day local history.",
              "30 秒实时导入轮询已停止。请重新连接查看实时导入，或切换至「30 天历史」查看本机已采集记录。",
            )}
          </span>
          <div className="load-inline-filters">
            <Button onClick={reconnectAction} size="sm" variant="primary">
              {onConnect ? text("Reconnect", "重新连接") : text("Go to Queries to connect", "去「查询」重新连接")}
            </Button>
            {view !== "history" ? (
              <Button onClick={() => setView("history")} size="sm" variant="secondary">
                {text("Open 30-day history", "查看 30 天历史")}
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}

      <div className="load-panel">
        {view === "history" ? (
          <LoadHistoryPanel
            connectionId={sessionExpired ? null : connectionId}
            focus={focus?.view === "history" ? focus : null}
            generation={String(tick)}
            includeSystem={includeSystem}
            kind={(kind as LoadKindGroup | null) ?? null}
            onOpenAlerts={onOpenAlerts}
            onOpenLog={setLog}
            text={text}
          />
        ) : sessionExpired ? (
          <PageGrid>
            <Card
              span={{ xl: 12, lg: 12, md: 8, sm: 4 }}
              title={text("Live loads unavailable · session expired", "实时导入暂不可用 · 会话已过期")}
              variant="detail"
            >
              <div className="state-box">
                <p>
                  {text(
                    "The read-only StarRocks session expired, so automatic polling was stopped. Reconnect to resume live overview and load records.",
                    "只读 StarRocks 连接会话已过期，自动轮询已停止。重新连接后即可恢复实时概览与导入记录读取。",
                  )}
                </p>
              </div>
            </Card>
          </PageGrid>
        ) : !connectionId ? (
          <PageGrid>
            <Card
              footer={
                <div className="load-inline-filters">
                  <Button onClick={reconnectAction} size="sm" variant="primary">
                    {onConnect ? text("Connect", "连接集群") : text("Go to Queries to connect", "去「查询」连接")}
                  </Button>
                  <Button onClick={() => setView("history")} size="sm" variant="secondary">
                    {text("View 30-day history", "查看 30 天历史")}
                  </Button>
                </div>
              }
              span={{ xl: 12, lg: 12, md: 8, sm: 4 }}
              title={text("Not connected to StarRocks", "尚未连接 StarRocks")}
              variant="detail"
            >
              <div className="state-box">
                <p>
                  {text(
                    "Connect to StarRocks to inspect live load overview, Routine Load partition lag, and load records. Local 30-day history remains available offline.",
                    "连接 StarRocks 后可实时查看导入概览、Routine Load 分区位点积压与导入记录；未连接时仍可查看本机 30 天历史。",
                  )}
                </p>
              </div>
            </Card>
          </PageGrid>
        ) : view === "overview" ? (
          overview.data ? (
            <LoadOverviewPanel
              initialRoutineJob={routineJob}
              kind={kind}
              nowMs={nowMs}
              onOpenAlerts={onOpenAlerts ? () => onOpenAlerts(null) : undefined}
              onOpenHour={(hour) => openRecords({ hour })}
              onOpenJob={openRoutineJobInOverview}
              onOpenRoutineLog={openRoutineLog}
              onOpenTable={(table) => openRecords({ table })}
              overview={overview.data}
              routineJobs={kind === "routine_load" ? routine.data : null}
              text={text}
            />
          ) : overview.error ? (
            <MonitorReadError
              error={overview.error}
              onRetry={overview.refresh}
              text={text}
              title={loadFailureText(overview.error, text)}
            />
          ) : (
            <p className="workspace-loading" role="status">
              {text("Loading…", "正在加载…")}
            </p>
          )
        ) : recordsView ? (
          <div aria-busy={records.loading} className="load-view-stack">
            {!records.data ? (
              records.error ? (
                <div className="notice-banner notice-banner--warn" role="alert">
                  <span>
                    {text(
                      `Reading with the new filters failed (${loadFailureText(records.error, text)}). Showing previous results.`,
                      `按新条件读取失败（${loadFailureText(records.error, text)}）；下方仍为上一次筛选结果。`,
                    )}
                  </span>
                  <Button onClick={records.refresh} size="sm" variant="secondary">
                    {text("Retry", "重试")}
                  </Button>
                </div>
              ) : (
                <div className="notice-banner" role="status">
                  <span>
                    {text(
                      "Reading with the new filters… showing previous results.",
                      "正在按新条件读取…下方为上一次筛选结果。",
                    )}
                  </span>
                </div>
              )
            ) : null}
            <LoadRecordsPanel
              filters={{ ...filters, kind, include_system: includeSystem }}
              hideSharedFilters
              onFilters={(next) => {
                if (next.kind !== kind) setKind(next.kind);
                if (next.include_system !== includeSystem) setIncludeSystem(next.include_system);
                setFilters(next);
              }}
              onOpenLog={openRecordLog}
              records={recordsView}
              text={text}
            />
          </div>
        ) : records.error ? (
          <MonitorReadError
            error={records.error}
            onRetry={records.refresh}
            text={text}
            title={loadFailureText(records.error, text)}
          />
        ) : (
          <p className="workspace-loading" role="status">
            {text("Loading…", "正在加载…")}
          </p>
        )}
      </div>

      {log ? (
        <LoadErrorLogDialog
          connectionId={log.stored ? connectionId : sessionExpired ? null : connectionId}
          onClose={() => setLog(null)}
          onSessionExpired={() => markSessionExpired(connectionId)}
          target={log}
          text={text}
        />
      ) : null}
    </section>
  );
}
