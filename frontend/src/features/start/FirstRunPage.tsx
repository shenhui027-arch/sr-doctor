import type { Dispatch, JSX } from "react";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { ConnectionForm } from "../connection/ConnectionForm";
import type { ConnectionAction, ConnectionState } from "../connection/connection-state";
import { ImportPanel } from "../import/ImportPanel";
import "./first-run.css";

/** Not connected yet: import a Profile offline, or connect read-only to pick one from history. */
export function FirstRunPage({ connection, dispatch, maxInputBytes = null, onReport, onSessionEnded, onSessionExpired }: {
  connection: ConnectionState;
  dispatch: Dispatch<ConnectionAction>;
  maxInputBytes?: number | null;
  onReport(report: AnalysisSnapshotReport): void;
  onSessionEnded?(): void;
  onSessionExpired?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const steps: Array<[string, string]> = [
    [text("Pick a query", "选择查询"), text("Audit history, Profile list, running queries or the SQL workspace", "审计历史、Profile 列表、运行中或 SQL 工作区")],
    [text("Diagnose evidence", "诊断证据"), text("Findings, execution structure, operator metrics and source lines", "发现、执行结构、算子指标与原文溯源")],
    [text("Investigation steps", "排查建议"), text("Evidence-linked steps; nothing is changed automatically", "关联证据的排查步骤，不自动修改")],
    [text("Before / after", "前后对比"), text("Verify with measured changes; incomparable items are marked", "以实测变化验证收益，标出不可比项")],
  ];
  return <section aria-labelledby="first-run-title" className="first-run">
    <header>
      <h1 id="first-run-title">{text("Start diagnosing from a Profile", "从一个 Profile 开始诊断")}</h1>
      <p>{text("Import a Profile file to analyze it offline, or connect StarRocks to pick slow queries from the audit history and fetch their Profiles. Read-only by default.", "离线导入 Profile 文件即可分析；连接 StarRocks 后可以从审计历史中挑选慢查询并拉取其 Profile。默认只读。")}</p>
    </header>
    <div className="first-run-cards">
      <div className="first-run-card">
        <ImportPanel idPrefix="start-profile" maxInputBytes={maxInputBytes} onReport={onReport} />
      </div>
      <section aria-labelledby="first-run-connect-title" className="first-run-card first-run-connect">
        <div className="first-run-card-heading">
          <h2 id="first-run-connect-title">{text("Connect StarRocks", "连接 StarRocks")}</h2>
          <span className="chip accent">{text("Read-only connection", "只读连接")}</span>
        </div>
        <ConnectionForm dispatch={dispatch} idPrefix="start" onSessionEnded={onSessionEnded} onSessionExpired={onSessionExpired} showCapabilities state={connection} submitLabel={text("Connect and probe", "连接并探测能力")} />
      </section>
    </div>
    <ol aria-label={text("Diagnosis flow", "诊断流程")} className="first-run-steps">
      {steps.map(([title, detail], index) => <li key={title}><span>{String(index + 1).padStart(2, "0")}</span><strong>{title}</strong><small>{detail}</small></li>)}
    </ol>
  </section>;
}
