import { useEffect, useState, type JSX } from "react";
import type { ExplainSnapshot } from "../../../../contracts/generated/doctor-contracts";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale } from "../../i18n/locale";
import { ExplainGraphPanel } from "./ExplainGraphPanel";
import "./plan.css";
import "./SqlAnalysisWorkspace.css";

/** One explicit execution path: standard Profile capture (Runtime Top). */
export type SqlAnalysisWorkspaceStage = "draft" | "confirm" | "pending" | "unknown";
export type SqlPlanMode = "logical" | "verbose" | "costs";

export type SqlAnalysisConnectionContext = Readonly<{
  connectionId: string;
  tlsMode: string;
  detectedVersion: string | null;
}>;

export type SqlAnalysisWorkspaceProps = {
  stage: SqlAnalysisWorkspaceStage;
  planMode: SqlPlanMode;
  sql: string;
  connection?: SqlAnalysisConnectionContext;
  planSnapshot?: ExplainSnapshot | null;
  planError?: string | null;
  planResultMode?: SqlPlanMode | null;
  planRequesting?: boolean;
  onSqlChange(sql: string): void;
  onPlanModeChange(mode: SqlPlanMode): void;
  onViewPlan(mode: SqlPlanMode): void;
  onRequestConfirmation(): void;
  onConfirmExecution(): void;
  onBackToDraft(): void;
  onStopWaiting(): void;
};

export function planFailureLabel(reason: string, text: (en: string, zh: string) => string): string {
  if (reason === "connection_expired" || reason === "connection_session_expired") return text("The connection expired. Reconnect before requesting a plan.", "连接已过期，请重新连接后再查看计划。");
  if (reason === "execution_response_unknown") return text("The last execution result is unknown. Check the database before submitting again.", "上一次执行结果未知，请先在数据库侧核查，再决定是否重新提交。");
  if (reason === "remote_timeout") return text("The plan request timed out. Check the connection and query scope.", "计划请求超时，请检查连接与查询范围。");
  return text("Plan request failed. Check the SQL and the connection.", "计划请求失败，请检查 SQL 与连接。");
}

export function SqlAnalysisWorkspace(props: SqlAnalysisWorkspaceProps): JSX.Element {
  const { text } = useLocale();
  const [graphOpen, setGraphOpen] = useState(false);
  const snapshot = props.planResultMode === props.planMode ? props.planSnapshot ?? null : null;
  const planError = props.planResultMode === props.planMode || props.planResultMode === null ? props.planError ?? null : null;
  useEffect(() => { setGraphOpen(false); }, [snapshot?.snapshot_id, props.stage]);
  const draft = props.stage === "draft";
  const hasSql = props.sql.trim().length > 0;
  const modes: ReadonlyArray<{ id: SqlPlanMode; label: string }> = [
    { id: "costs", label: text("COSTS · plan graph", "COSTS · 计划图") },
    { id: "logical", label: text("LOGICAL · text", "LOGICAL · 文本") },
    { id: "verbose", label: text("VERBOSE · text", "VERBOSE · 文本") },
  ];

  if (props.stage === "confirm") return <section className="sql-analysis-confirmation" data-sql-analysis-stage="confirm" aria-label={text("Confirm execution", "确认执行")}>
    <header><div><h1>{text("Execute once and capture its Profile", "执行一次并采集 Profile")}</h1><p>{text("This runs the SQL below on the connected database, discards the result rows and fetches the Profile of this exact execution.", "将在已连接数据库上执行下方 SQL，丢弃结果行，并拉取这一次执行的 Profile。")}</p></div><button className="button secondary" onClick={props.onBackToDraft} type="button">{text("Back to edit", "返回修改")}</button></header>
    <div className="sql-analysis-confirm-grid">
      <section className="sql-analysis-snapshot panel"><header className="sql-analysis-panel-heading"><h2>{text("SQL to execute", "将执行的 SQL")}</h2></header><textarea aria-label={text("SQL statement", "SQL 语句")} className="sql-analysis-sql" data-sql-analysis-input="sql" readOnly spellCheck={false} value={props.sql} /></section>
      <aside className="sql-analysis-confirm-side"><section className="sql-analysis-confirm-action panel">
        <h2>{text("This adds load to the database", "此操作会增加数据库负载")}</h2>
        <p>{text("One SELECT / WITH only · 10 s timeout · runs once and is never retried automatically.", "仅一条 SELECT / WITH · 超时 10 秒 · 只执行一次，不会自动重试。")}</p>
        {props.connection ? <p>{text("Session", "会话")} · TLS <code>{props.connection.tlsMode}</code> · {props.connection.detectedVersion ?? text("version unknown", "版本未知")}</p> : null}
        <button className="button warning" data-sql-analysis-action="confirm-execution" onClick={props.onConfirmExecution} type="button">{text("Execute once", "执行一次")}</button>
      </section></aside>
    </div>
  </section>;

  return <section className={`sql-analysis-workspace is-${props.stage}`} data-sql-analysis-stage={props.stage}>
    <header className="sql-analysis-title"><div><h1>{text("SQL workspace", "SQL 工作区")}</h1><p>{text("Check the readonly plan first; execute only when you need a runtime Profile.", "先看只读计划；需要运行时 Profile 时再执行一次。")}</p></div></header>
    <div className="sql-analysis-grid">
      <section className="sql-analysis-snapshot panel"><header className="sql-analysis-panel-heading"><h2>{text("SQL", "SQL")}</h2><span>{text("One SELECT or WITH statement", "一条 SELECT 或 WITH 语句")}</span></header><textarea aria-label={text("SQL statement", "SQL 语句")} className="sql-analysis-sql" data-sql-analysis-input="sql" disabled={!draft} onChange={(event) => props.onSqlChange(event.target.value)} placeholder={text("Paste one SELECT / WITH statement", "粘贴一条 SELECT / WITH 语句")} spellCheck={false} value={props.sql} /></section>
      <div className="sql-analysis-side">
        <section className="sql-analysis-plan panel">
          <h2>{text("1 · View plan (does not execute)", "1 · 查看计划（不执行）")}</h2>
          <div className="sql-analysis-plan-modes" aria-label={text("Plan mode", "计划模式")} role="group">{modes.map(({ id, label }) => <button aria-pressed={props.planMode === id} className={props.planMode === id ? "active" : ""} data-plan-mode={id} disabled={!draft} key={id} onClick={() => props.onPlanModeChange(id)} type="button">{label}</button>)}</div>
          <button className="button accent sql-analysis-plan-action" data-sql-analysis-action="view-plan" disabled={!draft || !hasSql || props.planRequesting} onClick={() => props.onViewPlan(props.planMode)} type="button">{props.planRequesting ? text("Requesting plan…", "正在请求计划…") : text("View plan", "查看计划")}</button>
          {planError ? <p className="plan-check-error" data-sql-analysis-plan-result="error" role="status">{planFailureLabel(planError, text)} <code>{planError}</code></p> : null}
          {snapshot ? <PlanSnapshotSummary onOpenGraph={() => setGraphOpen(true)} snapshot={snapshot} /> : null}
        </section>
        {draft ? <section className="sql-analysis-execution panel">
          <h2>{text("2 · Execute once and diagnose", "2 · 执行一次并诊断")}</h2>
          <p>{text("Runs the SQL once with profiling, then opens its Profile in Doctor.", "带 Profile 执行一次，然后在诊断页打开这次的 Profile。")}</p>
          <button className="button warning sql-analysis-confirm" data-sql-analysis-action="request-confirmation" disabled={!hasSql} onClick={props.onRequestConfirmation} type="button">{text("Review and execute…", "检查并执行…")}</button>
        </section> : <section className="sql-analysis-locked panel" aria-live="polite">
          <h2>{props.stage === "pending" ? text("Executing · waiting for the Profile", "执行中 · 等待 Profile") : text("Execution result unknown", "执行结果未知")}</h2>
          <p>{props.stage === "pending" ? text("Stopping only stops this page from waiting; it does not cancel the database query.", "停止等待只停止页面等待，不会取消数据库查询。") : text("No confirmed result returned. Check the query in the database before submitting again.", "没有收到确认结果。重新提交前请先在数据库侧核查这次查询。")}</p>
          {props.stage === "pending" ? <button className="button secondary" data-sql-analysis-action="stop-waiting" onClick={props.onStopWaiting} type="button">{text("Stop waiting", "停止等待")}</button> : null}
        </section>}
      </div>
    </div>
    {graphOpen && snapshot ? <ExplainGraphPanel onClose={() => setGraphOpen(false)} snapshot={snapshot} /> : null}
  </section>;
}

export function PlanSnapshotSummary({ onOpenGraph, snapshot }: { onOpenGraph(): void; snapshot: ExplainSnapshot }): JSX.Element {
  const { text } = useLocale();
  const fragments = new Set(snapshot.graph_view.nodes.map((node) => node.fragment_id).filter((fragment) => fragment !== null)).size;
  const hasGraph = snapshot.graph_view.state !== "unavailable";
  return <div className="plan-snapshot-wrap" data-sql-analysis-plan-result="success">
    <dl className="plan-snapshot">
      <div><dt>{text("Plan graph", "计划图")}</dt><dd>{hasGraph ? localizedEnum(snapshot.graph_view.state, text) : text("Text only", "仅文本")}</dd></div>
      <div><dt>{text("Structure", "结构")}</dt><dd>{fragments} Fragment · {snapshot.graph_view.nodes.length} {text("nodes", "个节点")}</dd></div>
    </dl>
    <div className="plan-snapshot-actions"><button className="graph-tool" onClick={onOpenGraph} type="button">{hasGraph ? text("Open plan graph", "打开计划图") : text("Open plan text", "打开计划文本")}</button></div>
  </div>;
}
