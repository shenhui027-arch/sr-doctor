import { useEffect, useRef, useState, type JSX } from "react";
import type {
  ConnectedQueryRef,
  ConnectionSessionView,
  QueryListStatementFilterCoverage,
  RunningProfileSnapshotResponse,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, Icon } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
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
  const binding =
    state.list.status === "success"
      ? admitRunningOfficialBinding({ session, list: state.list.response, selected: state.selected })
      : null;
  const snapshotLoading = state.snapshot.status === "loading";
  const officialLoading = state.official.status === "loading";

  return (
    <section aria-labelledby="running-profile-entry-title" className="running-profile-entry">
      <div className={`workbench-split running-profile-grid${state.selected ? "" : " no-selection"}`}>
        <Card
          actions={
            <>
              <span className="status-inline">
                <span className={ready ? "status-dot status-dot--ok" : "status-dot status-dot--hollow"} />
                {ready ? text("Connected", "已连接") : text("Not connected", "未连接")}
              </span>
              <label className="check-inline running-profile-statement-filter">
                <input
                  checked={hideOwnStatements}
                  disabled={!ready || state.list.status === "loading"}
                  onChange={(event) => setHideOwnStatements(event.target.checked)}
                  type="checkbox"
                />
                <span>{text("Hide this tool's queries", "隐藏本工具查询")}</span>
              </label>
              <Button
                disabled={!ready || state.list.status === "loading"}
                onClick={() => {
                  void controller.readCandidates(hideOwnStatements ? "exclude_doctor_and_non_query" : "all");
                }}
                size="sm"
                variant="primary"
              >
                {state.list.status === "loading" ? text("Reading…", "读取中…") : text("Read running queries", "读取运行中查询")}
              </Button>
            </>
          }
          aria-busy={state.list.status === "loading"}
          className="running-profile-candidates"
          footer={
            state.list.status === "success" ? (
              <div className="running-profile-receipt">
                <span>
                  {text("Returned", "返回")} <code className="mono">{state.list.response.returned}</code> / {text("limit", "上限")}{" "}
                  <code className="mono">{state.list.response.limit}</code>
                  {state.list.response.rejected > 0 ? (
                    <> · {text("rejected", "未采用")} <code className="mono">{state.list.response.rejected}</code></>
                  ) : null}
                  {state.list.response.at_limit || state.list.response.local_truncated ? (
                    <> · {text("list truncated", "列表已截断")}</>
                  ) : null}
                  {" · "}
                  <RunningStatementFilter coverage={state.list.response.statement_filter} text={text} />
                </span>
                <span className="mono">
                  {text("read at", "读取于")} <time dateTime={state.list.response.read_at_utc}>{state.list.response.read_at_utc}</time>
                </span>
              </div>
            ) : undefined
          }
          subtitle={text(
            "One SHOW PROFILELIST read per click; the list never refreshes on its own. Nothing here executes SQL.",
            "每次点击读取一次 SHOW PROFILELIST，列表不会自动刷新。这里不会执行任何 SQL。",
          )}
          title={<span id="running-profile-entry-title">{text("Running queries", "运行中的查询")}</span>}
          variant="table"
        >
          {state.list.status === "idle" && <EmptyCandidateState text={text} />}
          {state.list.status === "failed" && <RequestFailure error={state.list.error} text={text} />}
          {state.list.status === "success" && (
            <div className="running-profile-body">
              {state.list.response.candidates.length === 0 ? (
                <div className="state-box">
                  {text(
                    "No running query was returned. That does not mean the earlier queries finished.",
                    "没有返回运行中的查询；这并不代表之前的查询已完成。",
                  )}
                </div>
              ) : (
                <ul className="item-list running-profile-candidate-list">
                  {state.list.response.candidates.map((candidate) => (
                    <li key={`${candidate.connection_id}:${candidate.query_id}:${candidate.discovery_generation}:${candidate.source_refs.join("|")}`}>
                      <CandidateRow
                        candidate={candidate}
                        onSelect={() => controller.select(candidate)}
                        selected={candidate === state.selected}
                        text={text}
                      />
                    </li>
                  ))}
                </ul>
              )}
              <div className="running-profile-limitations">
                <LimitationNotes codes={state.list.response.limitations} text={text} />
              </div>
            </div>
          )}
        </Card>

        {/* The panel opens only once a query is chosen; the list keeps the full width until then. */}
        {state.selected && (
          <Card
            actions={
              <button
                aria-label={text("Close details", "关闭详情")}
                className="icon-btn running-profile-close"
                onClick={() => controller.deselect()}
                type="button"
              >
                <Icon name="close" />
              </button>
            }
            aria-label={text("Selected running query", "已选运行中查询")}
            className="running-profile-context anim-drawer-in"
            footer={
              <div aria-label={text("Read actions", "读取操作")} className="running-profile-actions">
                <Button
                  disabled={!binding || snapshotLoading}
                  onClick={() => {
                    void controller.readSnapshot();
                  }}
                  size="sm"
                  variant="primary"
                >
                  {snapshotLoading ? text("Reading Profile snapshot…", "读取 Profile 快照中…") : text("Read Profile snapshot", "读取 Profile 快照")}
                </Button>
                <Button
                  disabled={!binding || officialLoading}
                  onClick={() => {
                    void controller.readOfficial();
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {officialLoading ? text("Reading official analysis…", "读取官方解读中…") : text("Read official analysis", "读取官方解读")}
                </Button>
              </div>
            }
            role="complementary"
            subtitle={<span className="mono">{state.selected.query_id}</span>}
            title={text("Selected query", "已选查询")}
            variant="detail"
          >
            <div className="running-profile-detail-stack">
              <div className="notice-banner notice-banner--warn">
                {text(
                  "One-shot provisional snapshot: counters are still changing while the query runs, and it may finish or expire at any moment.",
                  "运行中一次性临时快照：计数仍在变化，查询可能随时结束或过期，不代表最终完成态 Profile。",
                )}
              </div>
              <dl className="kv-grid">
                <dt>{text("Query ID", "查询 ID")}</dt>
                <dd className="mono">{state.selected.query_id}</dd>
                <dt>{text("Status", "状态")}</dt>
                <dd>
                  <span className="status-inline">
                    <span className="status-dot status-dot--warn" />
                    {localizedEnum(state.selected.status, text)}
                  </span>
                </dd>
                <dt>{text("Source", "来源")}</dt>
                <dd className="mono">{state.selected.discovery_source ?? "—"}</dd>
              </dl>
              <SnapshotState onOpenDoctor={onOpenDoctor} state={state.snapshot} text={text} />
              <OfficialState state={state.official} text={text} />
            </div>
          </Card>
        )}
      </div>

      <RetainedSuccess onOpenDoctor={onOpenDoctor} retained={state.retained} text={text} />
    </section>
  );
}

/** The backend's own count of this tool's running statements it hid; nothing is counted here. */
function RunningStatementFilter({
  coverage,
  text,
}: {
  coverage: QueryListStatementFilterCoverage;
  text: (english: string, chinese: string) => string;
}): JSX.Element {
  const message =
    coverage.state === "doctor_only_applied" || coverage.state === "applied"
      ? text(`hid ${coverage.hidden_doctor_row_count} of this tool's own statements`, `已隐藏本工具语句 ${coverage.hidden_doctor_row_count} 条`)
      : coverage.state === "not_applied"
        ? text("this tool's statements shown", "含本工具语句")
        : text(`this tool's statements cannot be told apart (${coverage.reason})`, `无法区分本工具语句（${coverage.reason}）`);
  return <span data-running-statement-filter={coverage.state}>{message}</span>;
}

function EmptyCandidateState({ text }: { text: (english: string, chinese: string) => string }): JSX.Element {
  return (
    <div className="state-box running-profile-empty">
      <strong>{text("Nothing read yet", "尚未读取")}</strong>
      <p>{text("Click Read running queries to list what is running on the connected database.", "点击“读取运行中查询”，列出已连接数据库上正在运行的查询。")}</p>
    </div>
  );
}

function CandidateRow({
  candidate,
  onSelect,
  selected,
  text,
}: {
  candidate: ConnectedQueryRef;
  onSelect(): void;
  selected: boolean;
  text: (english: string, chinese: string) => string;
}): JSX.Element {
  return (
    <button aria-pressed={selected} className="item-btn running-profile-candidate" onClick={onSelect} type="button">
      <div className="item-row-top">
        <code className="mono">{candidate.query_id}</code>
        <span className="status-inline">
          <span className="status-dot status-dot--warn" />
          {localizedEnum(candidate.status, text)}
        </span>
      </div>
      <div className="item-row-meta">
        <span>{text("Source", "来源")}: <code className="mono">{candidate.discovery_source ?? "—"}</code></span>
        {candidate.database && <span>{text("Database", "数据库")}: <code className="mono">{candidate.database}</code></span>}
      </div>
    </button>
  );
}

function SnapshotState({
  onOpenDoctor,
  state,
  text,
}: {
  onOpenDoctor?: (snapshot: RunningProfileSnapshotResponse) => void;
  state: ReturnType<ReturnType<typeof createRunningProfileEntryController>["snapshot"]>["snapshot"];
  text: (english: string, chinese: string) => string;
}): JSX.Element | null {
  if (state.status === "idle") return null;
  if (state.status === "loading") {
    return (
      <section aria-busy="true" className="running-profile-state state-box">
        <strong>{text("Profile snapshot", "Profile 快照")}</strong>
        <span>{text("Reading one snapshot…", "正在读取一次快照…")}</span>
      </section>
    );
  }
  if (state.status === "failed") return <RequestFailure error={state.error} text={text} />;
  const response = state.response;
  return (
    <section className="running-profile-state running-profile-state-success">
      <span className="field-label">{text("Profile snapshot", "Profile 快照")}</span>
      <p className="mono">
        {text("Query state", "查询状态")} <code>{response.list_observed_state}</code> · {text("Profile", "Profile")}{" "}
        <code>{response.profile_lifecycle}</code> · {text("read at", "读取于")}{" "}
        <time dateTime={response.read_at_utc}>{response.read_at_utc}</time>
      </p>
      <Button disabled={!onOpenDoctor} onClick={() => onOpenDoctor?.(response)} size="sm" variant="secondary">
        {text("Open diagnosis", "打开诊断")}
      </Button>
    </section>
  );
}

function OfficialState({
  state,
  text,
}: {
  state: ReturnType<ReturnType<typeof createRunningProfileEntryController>["snapshot"]>["official"];
  text: (english: string, chinese: string) => string;
}): JSX.Element | null {
  if (state.status === "closed" || state.status === "unread") return null;
  if (state.status === "loading") {
    return (
      <section aria-busy="true" className="running-profile-state state-box">
        <strong>{text("Official analysis", "官方解读")}</strong>
        <span>{text("Reading official analysis…", "正在读取官方解读…")}</span>
      </section>
    );
  }
  return (
    <section className="running-profile-official">
      <OfficialProfileReadDetails state={state as OfficialDisplayState} />
    </section>
  );
}

function RetainedSuccess({
  onOpenDoctor,
  retained,
  text,
}: {
  onOpenDoctor?: (snapshot: RunningProfileSnapshotResponse) => void;
  retained: ReturnType<ReturnType<typeof createRunningProfileEntryController>["snapshot"]>["retained"];
  text: (english: string, chinese: string) => string;
}): JSX.Element | null {
  if (!retained) return null;
  return (
    <Card
      aria-label={text("Earlier result", "之前的结果")}
      className="running-profile-retained"
      subtitle={text("Kept for reading after a newer list read; it cannot start new reads.", "读取新列表后保留以供查看，不能再发起新的读取。")}
      title={
        <span>
          {text("Earlier result", "之前的结果")} · <code className="mono">{retained.query.query_id}</code>
        </span>
      }
      variant="detail"
    >
      <div className="running-profile-detail-stack">
        {retained.snapshot && <SnapshotState onOpenDoctor={onOpenDoctor} state={{ status: "success", response: retained.snapshot }} text={text} />}
        {retained.official && (
          <OfficialProfileReadDetails state={{ status: "success", binding: retained.official.binding, response: retained.official.response }} />
        )}
      </div>
    </Card>
  );
}

function RequestFailure({ error, text }: { error: ApiFailure; text: (english: string, chinese: string) => string }): JSX.Element {
  return (
    <section className="notice-banner notice-banner--danger running-profile-state-failed" role="alert">
      <div>
        <strong>{text("Request failed", "请求失败")}</strong>{" "}
        <span className="mono">
          <code>{error.code}</code> · <code>{error.message_id}</code>
        </span>
      </div>
    </section>
  );
}
