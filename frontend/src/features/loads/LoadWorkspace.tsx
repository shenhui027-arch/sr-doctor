// The load workspace (tasks 51-52, design boards I1-I4): an overview, Routine
// Load jobs and load records, read live through the connected session every
// 30 seconds while open. Nothing here changes StarRocks or stores what it reads.
import { useEffect, useRef, useState } from "react";
import type { JSX, KeyboardEvent } from "react";
import type { LoadKindFilter, LoadRecord, LoadRecordsResponse, RoutineLoadJob } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { useMonitorRead } from "../history/monitor-read";
import { MonitorReadError, RefreshBar } from "../history/MonitorStates";
import { LoadErrorLogDialog, type ErrorLogTarget } from "./LoadErrorLogDialog";
import { LoadOverviewPanel, type OverviewFilters } from "./LoadOverviewPanel";
import { LoadRecordsPanel, type RecordFilters } from "./LoadRecordsPanel";
import { RoutineLoadPanel } from "./RoutineLoadPanel";
import { feTime, loadFailureText } from "./load-text";
import "../history/history.css";
import "../history/notify.css";
import "./loads.css";

const REFRESH_MS = 30_000;
type LoadView = "overview" | "routine" | "records";
const VIEWS: LoadView[] = ["overview", "routine", "records"];
/** Where another workspace asks the load workspace to open. */
export type LoadFocus = { view: "overview" } | { view: "routine"; jobId: string };

const NO_RECORD_FILTERS: RecordFilters = { kind: null, outcome: null, table: "", include_system: false, hour: null };

export function LoadWorkspace({ connectionId, focus, onPaused, onOpenQueries }: {
  /** The ready session; null shows how to connect. */
  connectionId: string | null;
  focus?: LoadFocus | null;
  /** Paused job count for the header badge. */
  onPaused(count: number): void;
  onOpenQueries(): void;
}): JSX.Element {
  const { text } = useLocale();
  const [view, setView] = useState<LoadView>(focus?.view ?? "overview");
  const [routineJob, setRoutineJob] = useState<string | null>(focus?.view === "routine" ? focus.jobId : null);
  const [tick, setTick] = useState(0);
  const [overviewFilters, setOverviewFilters] = useState<OverviewFilters>({ kind: null, include_system: false });
  const [filters, setFilters] = useState<RecordFilters>(NO_RECORD_FILTERS);
  const [applied, setApplied] = useState<RecordFilters>(filters);
  const [log, setLog] = useState<ErrorLogTarget | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [focusPanel, setFocusPanel] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  // The overview entry a drill started from, to return focus to it.
  const drillOrigin = useRef<string | null>(null);
  useEffect(() => {
    if (!focusPanel) return;
    // Land on the target's heading once it is drawn; it names the result.
    const heading = panel.current?.querySelector<HTMLElement>(view === "records" ? "#load-records" : "#routine-detail");
    if (!heading) return;
    setFocusPanel(false);
    heading.focus();
  });
  useEffect(() => {
    const timer = window.setInterval(() => { setTick((value) => value + 1); setNowMs(Date.now()); }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, []);
  // Typing in the table box reads once it settles, not on every key.
  useEffect(() => {
    const timer = window.setTimeout(() => setApplied(filters), 400);
    return () => window.clearTimeout(timer);
  }, [filters]);
  const overview = useMonitorRead(connectionId && view === "overview" ? (signal) => doctorApi.readLoadOverview({ connection_id: connectionId, kind: overviewFilters.kind, include_system: overviewFilters.include_system }, signal) : null, `overview:${connectionId ?? ""}:${JSON.stringify(overviewFilters)}`, String(tick), "load_overview_failed");
  const routine = useMonitorRead(connectionId && view === "routine" ? (signal) => doctorApi.readRoutineLoadJobs(connectionId, signal) : null, `routine:${connectionId ?? ""}`, String(tick), "routine_load_jobs_failed");
  const records = useMonitorRead(connectionId && view === "records" ? (signal) => doctorApi.readLoadRecords({ connection_id: connectionId, kind: applied.kind, outcome: applied.outcome, table: applied.table.trim() || null, include_system: applied.include_system, created_hour: applied.hour }, signal) : null, `records:${connectionId ?? ""}:${JSON.stringify(applied)}`, String(tick), "load_records_failed");
  // Back on the overview, focus returns to the hour, table or lag entry.
  useEffect(() => {
    if (view !== "overview" || !overview.data || !drillOrigin.current) return;
    const origin = Array.from(panel.current?.querySelectorAll<HTMLElement>("[data-drill]") ?? []).find((element) => element.dataset.drill === drillOrigin.current);
    drillOrigin.current = null;
    origin?.focus();
  }, [view, overview.data]);
  const paused = routine.data ? Number(routine.data.summary.paused)
    : overview.data && overview.data.routine.support !== "unsupported_version" ? Number(overview.data.routine.paused) : null;
  // A new filter starts a new read; keep the last records (and the filter
  // inputs with them) on screen until it returns, marked as updating.
  const [shownRecords, setShownRecords] = useState<{ connectionId: string; data: LoadRecordsResponse } | null>(null);
  useEffect(() => { if (records.data && connectionId) setShownRecords({ connectionId, data: records.data }); }, [records.data, connectionId]);
  const recordsView = records.data ?? (shownRecords && shownRecords.connectionId === connectionId ? shownRecords.data : null);
  // The parent passes a new callback each render; report only when the count changes.
  const onPausedRef = useRef(onPaused);
  useEffect(() => { onPausedRef.current = onPaused; });
  useEffect(() => { if (paused !== null) onPausedRef.current(paused); }, [paused]);
  if (!connectionId) {
    return <div className="history-page load-page">
      <p className="history-banner neutral">{text("Connect to StarRocks to see Routine Load jobs and load records. They are read live and not stored.", "连接 StarRocks 后可查看 Routine Load 作业与导入记录；内容实时读取，不保存。")}</p>
      <button className="button secondary" onClick={onOpenQueries} type="button">{text("Go to Queries to connect", "去“查询”连接")}</button>
    </div>;
  }
  const current = view === "overview" ? overview : view === "routine" ? routine : records;
  const routineCount = routine.data?.jobs.length;
  const chooseView = (next: LoadView) => { if (next === "routine") setRoutineJob(null); setView(next); };
  // Tabs follow the ARIA tabs pattern: arrows, Home and End move and select.
  const onTabKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = VIEWS.indexOf(view);
    const next = event.key === "ArrowRight" ? VIEWS[(at + 1) % VIEWS.length] : event.key === "ArrowLeft" ? VIEWS[(at + VIEWS.length - 1) % VIEWS.length]
      : event.key === "Home" ? VIEWS[0] : event.key === "End" ? VIEWS[VIEWS.length - 1] : null;
    if (!next) return;
    event.preventDefault();
    chooseView(next);
    document.getElementById(`load-tab-${next}`)?.focus();
  };
  // A drill from the overview lands focus on the panel it opened.
  const drill = (next: LoadView) => {
    drillOrigin.current = document.activeElement instanceof HTMLElement ? document.activeElement.dataset.drill ?? null : null;
    setView(next);
    setFocusPanel(true);
  };
  const openRecords = (next: RecordFilters) => { setFilters(next); setApplied(next); drill("records"); };
  const openRoutineLog = (job: RoutineLoadJob) => setLog({ connectionId, jobId: job.job_id, title: job.name, meta: text(`Routine Load · ${job.db}.${job.table} · job_id ${job.job_id} · recent tasks' error logs`, `Routine Load · ${job.db}.${job.table} · job_id ${job.job_id} · 最近任务的错误日志（ERROR_LOG_URLS）`) });
  const openRecordLog = (record: LoadRecord) => setLog({ connectionId, jobId: record.job_id, title: record.label, meta: `${record.kind} · ${record.db}.${record.table} · job_id ${record.job_id} · ${feTime(record.create_time)} · ${record.state}` });
  const note = view === "overview" ? text("Read live from information_schema.loads and routine_load_jobs; read-only, not stored; summed by the server", "实时读取 information_schema.loads 与 routine_load_jobs，只读、不落盘；服务端汇总")
    : view === "routine" ? text("Read live from information_schema.routine_load_jobs; read-only, every 30 s or on demand", "实时读取 information_schema.routine_load_jobs，只读；每 30 秒或手动刷新")
      : text("Read live from information_schema.loads: Stream Load, Broker Load, INSERT and each Routine Load task", "读取 information_schema.loads，只读；Stream Load、Broker Load、INSERT 与 Routine Load 的每个提交任务");
  return <div className="history-page load-page">
    <div className="history-card-head load-toolbar">
      <div aria-label={text("Load views", "导入视图")} className="history-tabs" onKeyDown={onTabKey} role="tablist">
        {VIEWS.map((item) => <button aria-controls="load-panel" aria-selected={view === item} id={`load-tab-${item}`} key={item} onClick={() => chooseView(item)} role="tab" tabIndex={view === item ? 0 : -1} type="button">
          {item === "overview" ? text("Overview", "概览") : item === "routine" ? <>Routine Load{routineCount !== undefined && <> <span className="tab-count">{routineCount}</span></>}</> : text("Load records", "导入记录")}
        </button>)}
      </div>
      <span className="history-muted">{note}</span>
      <span className="trend-toolbar">
        {view === "overview" && <>
          <label className="notify-filter">{text("Type", "类型")}
            <select onChange={(event) => setOverviewFilters({ ...overviewFilters, kind: (event.target.value || null) as LoadKindFilter | null })} value={overviewFilters.kind ?? ""}>
              <option value="">{text("All", "全部")}</option><option value="stream_load">STREAM_LOAD</option><option value="routine_load">ROUTINE_LOAD</option><option value="broker">BROKER</option><option value="insert">INSERT</option>
            </select>
          </label>
          <label className="notify-check"><input checked={overviewFilters.include_system} onChange={(event) => setOverviewFilters({ ...overviewFilters, include_system: event.target.checked })} type="checkbox" /> {text("System loads", "含系统任务")}</label>
        </>}
        <RefreshBar error={current.error} loading={current.loading} onRefresh={current.refresh} text={text} updatedSeconds={current.updatedSeconds} />
      </span>
    </div>
    <div aria-labelledby={`load-tab-${view}`} className="load-panel" id="load-panel" ref={panel} role="tabpanel" tabIndex={-1}>
    {view === "overview" && (overview.data ? <LoadOverviewPanel nowMs={nowMs} onOpenHour={(hour) => openRecords({ ...NO_RECORD_FILTERS, kind: overviewFilters.kind, include_system: overviewFilters.include_system, hour })} onOpenJob={(jobId) => { setRoutineJob(jobId); drill("routine"); }} onOpenTable={(table) => openRecords({ ...NO_RECORD_FILTERS, kind: overviewFilters.kind, include_system: overviewFilters.include_system, table })} overview={overview.data} text={text} />
      : overview.error ? <MonitorReadError error={overview.error} onRetry={overview.refresh} text={text} title={loadFailureText(overview.error, text)} />
        : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>)}
    {view === "routine" && (routine.data ? <RoutineLoadPanel initialJob={routineJob} jobs={routine.data} key={routineJob ?? ""} nowMs={nowMs} onOpenLog={openRoutineLog} text={text} />
      : routine.error ? <MonitorReadError error={routine.error} onRetry={routine.refresh} text={text} title={loadFailureText(routine.error, text)} />
        : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>)}
    {view === "records" && (recordsView ? <div aria-busy={records.loading}>
      {!records.data && (records.error
        ? <p className="history-banner load-stale-note" role="alert">{text(`Reading with the new filters failed (${loadFailureText(records.error, text)}). The rows below are still from the previous filters.`, `按新条件读取失败（${loadFailureText(records.error, text)}）。下面仍是上一次筛选的结果。`)} <button className="button text" onClick={records.refresh} type="button">{text("Retry", "重试")}</button></p>
        : <p className="history-banner neutral load-stale-note" role="status">{text("Reading with the new filters… the rows below are from the previous filters.", "正在按新条件读取…下面是上一次筛选的结果。")}</p>)}
      <div className={records.data ? undefined : "load-updating"}><LoadRecordsPanel filters={filters} onFilters={setFilters} onOpenLog={openRecordLog} records={recordsView} text={text} /></div>
    </div>
      : records.error ? <MonitorReadError error={records.error} onRetry={records.refresh} text={text} title={loadFailureText(records.error, text)} />
        : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>)}
    </div>
    {log && log.connectionId === connectionId && <LoadErrorLogDialog connectionId={connectionId} onClose={() => setLog(null)} target={log} text={text} />}
  </div>;
}
