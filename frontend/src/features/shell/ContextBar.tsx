import type { JSX, ReactNode } from "react";
import type { AnalysisSnapshotReport, InputOrigin } from "../../../../contracts/generated/doctor-contracts";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { hasVersionPack, versionPackLabel } from "../../lib/version-pack";
import { evidenceGapItems } from "../findings/FindingsPanel";
import "./shell.css";

/** The current Profile under the top bar: identity, provenance and evidence state, all read from the report DTO. */
export function ContextBar({ actions, compact = false, inputOrigin, observedAtUtc = null, onOpenDoctor, onReturnToQueries, report }: {
  actions?: ReactNode;
  /** Recommendations (design board 03): the Profile identity, version and evidence state in one line. */
  compact?: boolean;
  onOpenDoctor?(): void;
  inputOrigin?: InputOrigin | null;
  /** The fetched query's observed UTC time, when the Profile came from the database. */
  observedAtUtc?: string | null;
  onReturnToQueries?(): void;
  report: AnalysisSnapshotReport;
}): JSX.Element {
  const { text } = useLocale();
  const queryId = report.query_summary?.query_id ?? null;
  const version = report.version.effective_version ?? report.version.reported_version?.value ?? text("unknown", "未知");
  const evidence = report.completeness.summary.state;
  const gaps = evidenceGapItems(report, text);
  const clock = observedAtUtc && /T(\d\d:\d\d:\d\d)/.exec(observedAtUtc)?.[1];
  const packed = hasVersionPack(report.version);
  // Only a Profile that came from the query list has a list to return to.
  const returnTo = inputOrigin && inputOrigin !== "offline_import" ? onReturnToQueries : undefined;
  if (compact) return <div aria-label={text("Current Profile", "当前 Profile")} className="context-bar compact" role="region">
    <div className="context-bar-inner">
      <span className="context-label">{text("Current Profile", "当前 Profile")}</span>
      {onOpenDoctor ? <button className="context-profile-link" onClick={onOpenDoctor} title={queryId ?? report.analysis_key} type="button">{queryId ? shortQueryId(queryId) : `${report.analysis_key.slice(0, 12)}…`}</button> : <span className="context-id">{queryId ? shortQueryId(queryId) : report.analysis_key.slice(0, 12)}</span>}
      <span className={`chip ${packed && report.version.compatibility === "compatible" ? "ok" : "warn"}`} title={localizedEnum(report.version.compatibility, text)}>StarRocks {version} · {versionPackLabel(report.version, text)}</span>
      <span className={`chip ${evidence === "complete" ? "ok" : evidence === "unavailable" ? "dashed" : "warn"}`} title={report.completeness.summary.reasons.map((reason) => localizedEnum(reason, text)).join(" · ") || undefined}>{`${text("Evidence", "证据")} ${localizedEnum(evidence, text)}`}</span>
      <span className="context-boundary">{text("Evidence-based · no SQL execution · nothing auto-applied · not a health score", "基于证据的建议 · 不执行 SQL · 不自动应用 · 非健康评分")}</span>
    </div>
  </div>;
  return <div aria-label={text("Current Profile", "当前 Profile")} className="context-bar" role="region">
    <div className="context-bar-inner">
      {returnTo ? <button aria-label={text("Back to query list", "返回查询列表")} className="context-back" onClick={returnTo} type="button"><svg aria-hidden="true" fill="none" height="14" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M15 6l-6 6 6 6" /></svg>{text("Query list", "查询列表")}</button> : null}
      {returnTo ? <span aria-hidden="true" className="context-divider" /> : null}
      <h1 className="context-title">{text("Query", "查询")} <span className="context-id" title={queryId ?? report.analysis_key}>{queryId ? shortQueryId(queryId) : `${report.analysis_key.slice(0, 12)}…`}</span></h1>
      <div className="context-chips">
        <span className="chip">{originLabel(inputOrigin ?? undefined, text)}{clock ? ` · ${clock} UTC` : ""}</span>
        <span className={`chip ${packed && report.version.compatibility === "compatible" ? "ok" : "warn"}`} title={localizedEnum(report.version.compatibility, text)}>StarRocks {version} · {versionPackLabel(report.version, text)}</span>
        <span className={`chip ${evidence === "complete" && gaps.length === 0 ? "ok" : evidence === "unavailable" ? "dashed" : "warn"}`} title={report.completeness.summary.reasons.map((reason) => localizedEnum(reason, text)).join(" · ") || undefined}>{text("Evidence", "证据")} {localizedEnum(evidence, text)}{gaps.length ? ` · ${text(`${gaps.length} gap(s)`, `${gaps.length} 处缺口`)}` : ""}</span>
      </div>
      {actions ? <div className="context-actions">{actions}</div> : null}
    </div>
  </div>;
}

/** The first two dash groups of a StarRocks query id (the full id stays in the title). */
export function shortQueryId(queryId: string): string {
  const parts = queryId.split("-");
  return parts.length >= 3 ? parts.slice(0, 3).join("-") : queryId;
}

export function originLabel(origin: InputOrigin | undefined, text: LocalizedText): string {
  if (origin === "offline_import") return text("Imported Profile", "导入的 Profile");
  if (origin === "connected_sql_runtime") return text("Executed from the SQL workspace", "SQL 工作区执行");
  if (origin === "candidate_profile_user_external_rerun") return text("User-side rerun", "用户侧重跑");
  if (origin === undefined) return text("Profile", "Profile");
  return text("Fetched from the database", "从数据库拉取");
}
