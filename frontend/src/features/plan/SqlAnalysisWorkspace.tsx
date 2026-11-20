import { useEffect, useState, type JSX } from "react";
import type { ExplainSnapshot } from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, InfoTip } from "../../design";
import { localizedEnum, lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
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
  /** Task 68: why the last "execute once" failed; shown with the execution, not under the plan. */
  executionError?: string | null;
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

const STAGE_LABELS: Record<SqlAnalysisWorkspaceStage, readonly [string, string]> = {
  draft: ["Editing", "编辑态"],
  confirm: ["Confirm execution", "二次确认"],
  pending: ["Executing", "执行中"],
  unknown: ["Result unknown", "结果未知"],
};

const TLS_MODE_LABELS: Record<string, readonly [string, string]> = {
  verify_full: ["TLS verify-full", "TLS verify-full"],
  plaintext: [
    "Plaintext · password and SQL are not encrypted in transit",
    "明文连接 · 口令与 SQL 不加密传输",
  ],
  docker_loopback_plaintext: [
    "Loopback Docker plaintext · password and SQL are not encrypted in transit",
    "回环 Docker 明文 · 口令与 SQL 不加密传输",
  ],
};

function transportLabel(tlsMode: string, text: LocalizedText): string {
  return lookupEnumLabel(TLS_MODE_LABELS, tlsMode, text);
}

export function planFailureLabel(reason: string, text: (en: string, zh: string) => string): string {
  if (reason === "connection_expired" || reason === "connection_session_expired") {
    return text("The connection expired. Reconnect before requesting a plan.", "连接已过期，请重新连接后再查看计划。");
  }
  if (reason === "execution_response_unknown") {
    return text(
      "The last execution result is unknown. Check the database before submitting again.",
      "上一次执行结果未知，请先在数据库侧核查，再决定是否重新提交。",
    );
  }
  if (reason === "remote_timeout") {
    return text("The plan request timed out. Check the connection and query scope.", "计划请求超时，请检查连接与查询范围。");
  }
  return text("Plan request failed. Check the SQL and the connection.", "计划请求失败，请检查 SQL 与连接。");
}

export function executionFailureLabel(reason: string, text: (en: string, zh: string) => string): string {
  if (reason === "connection_expired" || reason === "connection_session_expired") {
    return text("The connection expired. Reconnect, then execute again.", "连接已过期，请重新连接后再执行。");
  }
  if (reason === "profile_unavailable") {
    return text(
      "The query ran, but its Profile could not be fetched. It may not be ready yet; try once more later.",
      "查询已执行，但没有取到这次执行的 Profile，可能还未生成。可稍后再试一次。",
    );
  }
  if (reason === "remote_timeout") {
    return text("The execution timed out. Narrow the query and try again.", "执行超时，请缩小查询范围后再试。");
  }
  return text("The execution was refused. Check the SQL and the connection.", "执行被拒绝，请检查 SQL 与连接。");
}

export function SqlAnalysisWorkspace(props: SqlAnalysisWorkspaceProps): JSX.Element {
  const { text } = useLocale();
  const [graphOpen, setGraphOpen] = useState(false);
  const snapshot = props.planResultMode === props.planMode ? props.planSnapshot ?? null : null;
  const planError = props.planResultMode === props.planMode || props.planResultMode === null ? props.planError ?? null : null;
  useEffect(() => {
    setGraphOpen(false);
  }, [snapshot?.snapshot_id, props.stage]);

  const draft = props.stage === "draft";
  const confirm = props.stage === "confirm";
  const hasSql = props.sql.trim().length > 0;
  const isPlaintext = Boolean(props.connection && props.connection.tlsMode !== "verify_full");
  const modes: ReadonlyArray<{ id: SqlPlanMode; label: string }> = [
    { id: "costs", label: text("COSTS · plan graph", "COSTS · 计划图") },
    { id: "logical", label: text("LOGICAL · text", "LOGICAL · 文本") },
    { id: "verbose", label: text("VERBOSE · text", "VERBOSE · 文本") },
  ];

  return (
    <section
      aria-label={confirm ? text("Confirm execution", "确认执行") : text("SQL workspace", "SQL 工作区")}
      className={`sql-analysis-workspace is-${props.stage}`}
      data-sql-analysis-stage={props.stage}
    >
      <div className="workbench-split sql-analysis-grid">
        <Card
          actions={
            <>
              <span className={confirm ? "state-stamp state-stamp--warn" : "badge-stamp"}>
                {lookupEnumLabel(STAGE_LABELS, props.stage, text)}
              </span>
              <InfoTip label={text("Controlled SQL boundary", "受控 SQL 边界")} side="bottom">
                {text(
                  "Accepts one SELECT or WITH statement only. Multi-statements, DDL/DML/INSERT, EXPLAIN ANALYZE and automatic retries are never allowed. New plan checks are started here.",
                  "仅接受单条 SELECT / WITH 语句；禁止多语句、DDL/DML/INSERT、EXPLAIN ANALYZE 与自动重试。发起新的计划检查统一在此进行。",
                )}
              </InfoTip>
            </>
          }
          className="sql-analysis-editor-card"
          footer={
            <div className="sql-analysis-editor-foot">
              <span>
                {props.connection ? (
                  <>
                    {text("Session", "会话")} · {transportLabel(props.connection.tlsMode, text)} ·{" "}
                    <span className="mono">{props.connection.detectedVersion ?? text("version unknown", "版本未知")}</span>
                  </>
                ) : (
                  text("One SELECT or WITH statement", "一条 SELECT 或 WITH 语句")
                )}
              </span>
              {confirm && (
                <Button onClick={props.onBackToDraft} size="sm" variant="secondary">
                  {text("Back to edit", "返回修改")}
                </Button>
              )}
            </div>
          }
          title={confirm ? text("SQL to execute", "将执行的 SQL") : text("SQL statement (one SELECT / WITH)", "SQL 语句（单条 SELECT / WITH）")}
          variant="form"
        >
          <textarea
            aria-label={text("SQL statement", "SQL 语句")}
            className="textarea sql-analysis-sql"
            data-sql-analysis-input="sql"
            disabled={props.stage === "pending" || props.stage === "unknown"}
            onChange={(event) => {
              if (draft) props.onSqlChange(event.target.value);
            }}
            placeholder={text("Paste one SELECT / WITH statement", "粘贴一条 SELECT / WITH 语句")}
            readOnly={confirm}
            spellCheck={false}
            value={props.sql}
          />
        </Card>

        {confirm ? (
          <Card
            className="sql-analysis-control-card"
            footer={
              <div className="sql-analysis-control-foot">
                <Button onClick={props.onBackToDraft} size="sm" variant="secondary">
                  {text("Back to edit", "返回修改")}
                </Button>
                <Button data-sql-analysis-action="confirm-execution" onClick={props.onConfirmExecution} size="sm" variant="warning">
                  {text("Execute once", "执行一次")}
                </Button>
              </div>
            }
            subtitle={text(
              "This runs the SQL on the connected database, discards the result rows and fetches the Profile of this exact execution.",
              "将在已连接数据库上执行左侧 SQL，丢弃结果行，并拉取这一次执行的 Profile。",
            )}
            title={text("Execute once and capture its Profile", "执行一次并采集 Profile")}
            variant="detail"
          >
            <div className="sql-analysis-control-stack">
              <div className="notice-banner notice-banner--warn">
                <div>
                  <strong>{text("This adds load to the database", "此操作会增加数据库负载")}</strong>
                  <p>{text("One SELECT / WITH only · 10 s timeout · runs once and is never retried automatically.", "仅一条 SELECT / WITH · 超时 10 秒 · 只执行一次，不会自动重试。")}</p>
                </div>
              </div>
              {isPlaintext && (
                <div className="notice-banner notice-banner--warn" role="note">
                  <div>
                    <strong>{text("Plaintext connection: password and SQL are not encrypted in transit", "明文连接：口令与 SQL 不加密传输")}</strong>
                    <p>{text("The SQL text will be sent without TLS encryption on this connection.", "当前连接未启用 TLS 加密，提交的 SQL 原文将以明文传输。")}</p>
                  </div>
                </div>
              )}
              {props.connection && (
                <dl className="kv-grid">
                  <dt>{text("Transport", "传输方式")}</dt>
                  <dd>{transportLabel(props.connection.tlsMode, text)}</dd>
                  <dt>{text("Engine version", "引擎版本")}</dt>
                  <dd className="mono">{props.connection.detectedVersion ?? text("version unknown", "版本未知")}</dd>
                </dl>
              )}
            </div>
          </Card>
        ) : (
          <Card
            className="sql-analysis-control-card"
            footer={
              <div className="sql-analysis-control-foot">
                {draft ? (
                  <>
                    <span className="muted">{text("Enable profile = true · pipeline_profile_level = 1 · 10 s timeout", "enable_profile=true · pipeline_profile_level=1 · 超时 10 秒")}</span>
                    <Button
                      className="sql-analysis-confirm"
                      data-sql-analysis-action="request-confirmation"
                      disabled={!hasSql}
                      onClick={props.onRequestConfirmation}
                      size="sm"
                      variant="warning"
                    >
                      {text("Review and execute…", "检查并执行…")}
                    </Button>
                  </>
                ) : props.stage === "pending" ? (
                  <Button data-sql-analysis-action="stop-waiting" onClick={props.onStopWaiting} size="sm" variant="secondary">
                    {text("Stop waiting", "停止等待")}
                  </Button>
                ) : null}
              </div>
            }
            subtitle={text(
              "Check the readonly plan first; execute only when you need a runtime Profile.",
              "先看只读计划；需要运行时 Profile 时再执行一次。",
            )}
            title={text("Plan check & controlled execution", "受控计划检查与单次执行")}
            variant="detail"
          >
            <div className="sql-analysis-control-stack">
              <section className="sql-analysis-section">
                <div className="sql-analysis-section-head">
                  <h2 className="sql-analysis-step-title">{text("1 · View plan (does not execute)", "1 · 查看计划（不执行）")}</h2>
                </div>
                <div className="sql-analysis-plan-row">
                  <div aria-label={text("Plan mode", "计划模式")} className="seg-tabs seg-tabs--sm sql-analysis-plan-modes" role="group">
                    {modes.map(({ id, label }) => (
                      <button
                        aria-pressed={props.planMode === id}
                        className="seg-tab"
                        data-plan-mode={id}
                        disabled={!draft}
                        key={id}
                        onClick={() => props.onPlanModeChange(id)}
                        type="button"
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                  <Button
                    className="sql-analysis-plan-action"
                    data-sql-analysis-action="view-plan"
                    disabled={!draft || !hasSql || props.planRequesting}
                    onClick={() => props.onViewPlan(props.planMode)}
                    size="sm"
                    variant="primary"
                  >
                    {props.planRequesting ? text("Requesting plan…", "正在请求计划…") : text("View plan", "查看计划")}
                  </Button>
                </div>
                {planError ? (
                  <div className="notice-banner notice-banner--danger plan-check-error" data-sql-analysis-plan-result="error" role="status">
                    <span>{planFailureLabel(planError, text)} <code className="mono">{planError}</code></span>
                  </div>
                ) : null}
                {snapshot ? <PlanSnapshotSummary onOpenGraph={() => setGraphOpen(true)} snapshot={snapshot} /> : null}
              </section>

              {draft ? (
                <section className="sql-analysis-section">
                  <h2 className="sql-analysis-step-title">{text("2 · Execute once and diagnose", "2 · 执行一次并诊断")}</h2>
                  <div className="notice-banner notice-banner--warn">
                    <div>
                      <strong>{text("Runs the SQL once with profiling, then opens its Profile in Doctor.", "带 Profile 执行一次，然后在诊断页打开这次的 Profile。")}</strong>
                      <p>
                        {text("10 s timeout · single SELECT / WITH · never retried.", "超时 10 秒 · 仅单条 SELECT / WITH · 不会自动重试。")}
                        {isPlaintext ? ` · ${text("Password and SQL are not encrypted in transit.", "口令与 SQL 不加密传输。")}` : ""}
                      </p>
                    </div>
                  </div>
                  {props.executionError ? (
                    <div className="notice-banner notice-banner--danger plan-check-error" data-sql-analysis-execution-result="error" role="status">
                      <span>{executionFailureLabel(props.executionError, text)} <code className="mono">{props.executionError}</code></span>
                    </div>
                  ) : null}
                </section>
              ) : (
                <section aria-live="polite" className="sql-analysis-section sql-analysis-locked">
                  <div className="notice-banner notice-banner--warn">
                    <div>
                      <strong>
                        {props.stage === "pending"
                          ? text("Executing · waiting for the Profile", "执行中 · 等待 Profile")
                          : text("Execution result unknown", "执行结果未知")}
                      </strong>
                      <p>
                        {props.stage === "pending"
                          ? text(
                              "Stopping only stops this page from waiting; it does not cancel the database query.",
                              "停止等待只停止页面等待，不会取消数据库查询。",
                            )
                          : text(
                              "No confirmed result returned. Check the query in the database before submitting again.",
                              "没有收到确认结果。重新提交前请先在数据库侧核查这次查询。",
                            )}
                      </p>
                    </div>
                  </div>
                </section>
              )}
            </div>
          </Card>
        )}
      </div>
      {graphOpen && snapshot ? <ExplainGraphPanel onClose={() => setGraphOpen(false)} snapshot={snapshot} /> : null}
    </section>
  );
}

export function PlanSnapshotSummary({ onOpenGraph, snapshot }: { onOpenGraph(): void; snapshot: ExplainSnapshot }): JSX.Element {
  const { text } = useLocale();
  const hasGraph = snapshot.graph_view.state !== "unavailable";
  const fragments = new Set(snapshot.graph_view.nodes.map((node) => node.fragment_id).filter((fragment) => fragment !== null)).size;

  return (
    <div className="plan-snapshot-wrap" data-sql-analysis-plan-result="success">
      <dl className="kv-grid plan-snapshot">
        <dt>{text("Plan graph", "计划图")}</dt>
        <dd>{hasGraph ? localizedEnum(snapshot.graph_view.state, text) : text("Text only", "仅文本")}</dd>
        <dt>{text("Structure", "结构")}</dt>
        <dd className="mono">
          {hasGraph ? (
            `${fragments} Fragment · ${snapshot.graph_view.nodes.length} ${text("nodes", "个节点")}`
          ) : (
            <span className="state-stamp" data-honest-state="unavailable">
              <span aria-hidden="true" className="state-stamp-sym">—</span>
              <span>{text("Unavailable", "不可用")}</span>
            </span>
          )}
        </dd>
      </dl>
      <div className="plan-snapshot-actions">
        <Button onClick={onOpenGraph} size="sm" variant="secondary">
          {hasGraph ? text("Open plan graph", "打开计划图") : text("Open plan text", "打开计划文本")}
        </Button>
      </div>
    </div>
  );
}
