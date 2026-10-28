import { useEffect, useRef, useState, type JSX } from "react";
import type { ConnectedQueryRef, ConnectionSessionView, QueryListStatementFilterCoverage, RunningProfileSnapshotResponse } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type ApiFailure, type DoctorApi } from "../../lib/api";
import { OfficialProfileReadDetails, type OfficialDisplayState } from "../execution/OfficialProfilePanel";
import { admitRunningOfficialBinding } from "../execution/running-official-binding";
import { LimitationNotes } from "../shell/LimitationNotes";
import { createRunningProfileEntryController, type RunningProfileEntryApi } from "./running-profile-entry-controller";
import "./running-profile-entry.css";

export type RunningProfileEntryProps = Readonly<{
  api?: Pick<DoctorApi, "readRunningProfileCandidates" | "readRunningProfileSnapshot" | "readOfficialProfile">;
  session: ConnectionSessionView | null;
  onDiscoveryChanged?(): void;
  onSessionExpired?(): void;
  onOpenDoctor?(snapshot: RunningProfileSnapshotResponse): void;
}>;

/**
 * A bounded, manual Running Profile entry below the shared Queries chrome.
 * It never executes SQL, polls, or turns one explicit read into another.
 */
export function RunningProfileEntry({
  api = doctorApi,
  session,
  onDiscoveryChanged,
  onSessionExpired,
  onOpenDoctor,
}: RunningProfileEntryProps): JSX.Element {
  const { text } = useLocale();
  const [, setRevision] = useState(0);
  const [hideOwnStatements, setHideOwnStatements] = useState(true);
  const sessionRef = useRef(session);
  const callbacksRef = useRef({ onDiscoveryChanged, onSessionExpired });
  sessionRef.current = session;
  callbacksRef.current = { onDiscoveryChanged, onSessionExpired };
  const controllerRef = useRef<ReturnType<typeof createRunningProfileEntryController> | null>(null);
  if (!controllerRef.current) {
    controllerRef.current = createRunningProfileEntryController({
      api: api as RunningProfileEntryApi,
      getSession: () => sessionRef.current,
      onChange: () => setRevision((revision) => revision + 1),
      onDiscoveryChanged: () => callbacksRef.current.onDiscoveryChanged?.(),
      onSessionExpired: () => callbacksRef.current.onSessionExpired?.(),
    });
  }
  const controller = controllerRef.current;
  const sessionKey = `${session?.connection_id ?? "none"}:${session?.state ?? "none"}`;

  useEffect(() => {
    controller.reset();
  }, [controller, sessionKey]);
  useEffect(() => () => controller.dispose(), [controller]);

  const state = controller.snapshot();
  const ready = session?.state === "ready";
  const binding = state.list.status === "success"
    ? admitRunningOfficialBinding({ session, list: state.list.response, selected: state.selected })
    : null;
  const snapshotLoading = state.snapshot.status === "loading";
  const officialLoading = state.official.status === "loading";

  return <section aria-labelledby="running-profile-entry-title" className="running-profile-entry">
    <header className="running-profile-entry-heading">
      <div>
        <h2 id="running-profile-entry-title">{text("Running queries", "运行中的查询")}</h2>
        <p>{text("Read the queries that are running now, then read a Profile snapshot or the official analysis of one of them. Nothing here executes SQL.", "读取正在运行的查询，再读取其中一条的 Profile 快照或官方解读。这里不会执行任何 SQL。")}</p>
      </div>
      <span className="running-profile-mock">{ready ? text("Connected", "已连接") : text("Not connected", "未连接")}</span>
    </header>

    <div className={`running-profile-grid${state.selected ? "" : " no-selection"}`}>
      <section className="running-profile-candidates" aria-busy={state.list.status === "loading"} aria-labelledby="running-profile-candidates-title">
        <header>
          <div>
            <h3 id="running-profile-candidates-title">{text("Running now", "当前运行中")}</h3>
            <p>{text("One SHOW PROFILELIST read per click; the list never refreshes on its own.", "每次点击读取一次 SHOW PROFILELIST，列表不会自动刷新。")}</p>
          </div>
          <label className="running-profile-statement-filter"><input checked={hideOwnStatements} disabled={!ready || state.list.status === "loading"} onChange={(event) => setHideOwnStatements(event.target.checked)} type="checkbox" />{text("Hide this tool's queries", "隐藏本工具查询")}</label>
          <button className="button secondary" disabled={!ready || state.list.status === "loading"} onClick={() => { void controller.readCandidates(hideOwnStatements ? "exclude_doctor_and_non_query" : "all"); }} type="button">
            {state.list.status === "loading" ? text("Reading…", "读取中…") : text("Read running queries", "读取运行中查询")}
          </button>
        </header>

        {state.list.status === "idle" && <EmptyCandidateState text={text} />}
        {state.list.status === "failed" && <RequestFailure error={state.list.error} text={text} />}
        {state.list.status === "success" && <>
          <p className="running-profile-receipt">
            {text("Returned", "返回")} <code>{state.list.response.returned}</code> / {text("limit", "上限")} <code>{state.list.response.limit}</code>
            {state.list.response.rejected > 0 ? <> · {text("rejected", "未采用")} <code>{state.list.response.rejected}</code></> : null}
            {state.list.response.at_limit || state.list.response.local_truncated ? <> · {text("list truncated", "列表已截断")}</> : null}
            {" · "}<RunningStatementFilter coverage={state.list.response.statement_filter} text={text} />
            {" · "}{text("read at", "读取于")} <time dateTime={state.list.response.read_at_utc}>{state.list.response.read_at_utc}</time>
          </p>
          <div className="running-profile-candidate-list">
            {state.list.response.candidates.map((candidate) => <CandidateRow candidate={candidate} key={`${candidate.connection_id}:${candidate.query_id}:${candidate.discovery_generation}:${candidate.source_refs.join("|")}`} onSelect={() => controller.select(candidate)} selected={candidate === state.selected} text={text} />)}
            {state.list.response.candidates.length === 0 && <p className="running-profile-empty-list">{text("No running query was returned. That does not mean the earlier queries finished.", "没有返回运行中的查询；这并不代表之前的查询已完成。")}</p>}
          </div>
          <LimitationNotes codes={state.list.response.limitations} text={text} />
        </>}
      </section>

      {/* The panel opens only once a query is chosen; the list keeps the full width until then. */}
      {state.selected && <aside className="running-profile-context" aria-label={text("Selected running query", "已选运行中查询") }>
        <h3>{text("Selected query", "已选查询")}<button aria-label={text("Close details", "关闭详情")} className="running-profile-close" onClick={() => controller.deselect()} type="button"><svg aria-hidden="true" fill="none" height="16" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="16"><path d="M6 6l12 12M18 6L6 18" /></svg></button></h3>
        <>
          <p className="running-profile-selected"><code>{state.selected.query_id}</code> · {text("status", "状态")} <code>{state.selected.status}</code></p>
          <section className="running-profile-actions" aria-label={text("Read actions", "读取操作") }>
            <button className="button secondary running-profile-action" disabled={!binding || snapshotLoading} onClick={() => { void controller.readSnapshot(); }} type="button">
              {snapshotLoading ? text("Reading Profile snapshot…", "读取 Profile 快照中…") : text("Read Profile snapshot", "读取 Profile 快照")}
            </button>
            <button className="button secondary running-profile-action" disabled={!binding || officialLoading} onClick={() => { void controller.readOfficial(); }} type="button">
              {officialLoading ? text("Reading official analysis…", "读取官方解读中…") : text("Read official analysis", "读取官方解读")}
            </button>
          </section>
          <SnapshotState onOpenDoctor={onOpenDoctor} state={state.snapshot} text={text} />
          <OfficialState state={state.official} text={text} />
        </>
      </aside>}
    </div>

    <RetainedSuccess onOpenDoctor={onOpenDoctor} retained={state.retained} text={text} />
  </section>;
}

/** The backend's own count of this tool's running statements it hid; nothing is counted here. */
function RunningStatementFilter({ coverage, text }: { coverage: QueryListStatementFilterCoverage; text: (english: string, chinese: string) => string }): JSX.Element {
  const message = coverage.state === "doctor_only_applied"
    ? text(`hid ${coverage.hidden_doctor_row_count} of this tool's own statements`, `已隐藏本工具语句 ${coverage.hidden_doctor_row_count} 条`)
    : coverage.state === "not_applied"
    ? text("this tool's statements shown", "含本工具语句")
    : coverage.state === "applied"
    ? text(`hid ${coverage.hidden_doctor_row_count} of this tool's own statements`, `已隐藏本工具语句 ${coverage.hidden_doctor_row_count} 条`)
    : text(`this tool's statements cannot be told apart (${coverage.reason})`, `无法区分本工具语句（${coverage.reason}）`);
  return <span data-running-statement-filter={coverage.state}>{message}</span>;
}

function EmptyCandidateState({ text }: { text: (english: string, chinese: string) => string }): JSX.Element {
  return <div className="running-profile-empty">
    <strong>{text("Nothing read yet", "尚未读取")}</strong>
    <p>{text("Click Read running queries to list what is running on the connected database.", "点击“读取运行中查询”，列出已连接数据库上正在运行的查询。")}</p>
  </div>;
}

function CandidateRow({ candidate, onSelect, selected, text }: { candidate: ConnectedQueryRef; onSelect(): void; selected: boolean; text: (english: string, chinese: string) => string }): JSX.Element {
  return <button aria-pressed={selected} className="running-profile-candidate" onClick={onSelect} type="button">
    <span><code>{candidate.query_id}</code></span>
    <span>{text("status", "状态")}: <code>{candidate.status}</code></span>
    <span>{text("source", "来源")}: <code>{candidate.discovery_source ?? "—"}</code></span>
  </button>;
}

function SnapshotState({ onOpenDoctor, state, text }: { onOpenDoctor?: (snapshot: RunningProfileSnapshotResponse) => void; state: ReturnType<ReturnType<typeof createRunningProfileEntryController>["snapshot"]>["snapshot"]; text: (english: string, chinese: string) => string }): JSX.Element | null {
  if (state.status === "idle") return null;
  if (state.status === "loading") return <section aria-busy="true" className="running-profile-state"><h4>{text("Profile snapshot", "Profile 快照")}</h4><p>{text("Reading one snapshot…", "正在读取一次快照…")}</p></section>;
  if (state.status === "failed") return <RequestFailure error={state.error} text={text} />;
  const response = state.response;
  return <section className="running-profile-state running-profile-state-success">
    <h4>{text("Profile snapshot", "Profile 快照")}</h4>
    <p>{text("Query state", "查询状态")} <code>{response.list_observed_state}</code> · {text("Profile", "Profile")} <code>{response.profile_lifecycle}</code> · {text("read at", "读取于")} <time dateTime={response.read_at_utc}>{response.read_at_utc}</time></p>
    <button className="button secondary" disabled={!onOpenDoctor} onClick={() => onOpenDoctor?.(response)} type="button">{text("Open diagnosis", "打开诊断")}</button>
  </section>;
}

function OfficialState({ state, text }: { state: ReturnType<ReturnType<typeof createRunningProfileEntryController>["snapshot"]>["official"]; text: (english: string, chinese: string) => string }): JSX.Element | null {
  if (state.status === "closed" || state.status === "unread") return null;
  if (state.status === "loading") return <section aria-busy="true" className="running-profile-state"><h4>{text("Official analysis", "官方解读")}</h4><p>{text("Reading official analysis…", "正在读取官方解读…")}</p></section>;
  return <section className="running-profile-official"><OfficialProfileReadDetails state={state as OfficialDisplayState} /></section>;
}

function RetainedSuccess({ onOpenDoctor, retained, text }: { onOpenDoctor?: (snapshot: RunningProfileSnapshotResponse) => void; retained: ReturnType<ReturnType<typeof createRunningProfileEntryController>["snapshot"]>["retained"]; text: (english: string, chinese: string) => string }): JSX.Element | null {
  if (!retained) return null;
  return <section className="running-profile-retained" aria-label={text("Earlier result", "之前的结果") }>
    <div><h3>{text("Earlier result", "之前的结果")} · <code>{retained.query.query_id}</code></h3></div>
    <p>{text("Kept for reading after a newer list read; it cannot start new reads.", "读取新列表后保留以供查看，不能再发起新的读取。")}</p>
    {retained.snapshot && <SnapshotState onOpenDoctor={onOpenDoctor} state={{ status: "success", response: retained.snapshot }} text={text} />}
    {retained.official && <OfficialProfileReadDetails state={{ status: "success", binding: retained.official.binding, response: retained.official.response }} />}
  </section>;
}

function RequestFailure({ error, text }: { error: ApiFailure; text: (english: string, chinese: string) => string }): JSX.Element {
  return <section className="running-profile-state running-profile-state-failed" role="alert"><strong>{text("Request failed", "请求失败")}</strong><p><code>{error.code}</code> · <code>{error.message_id}</code></p></section>;
}
