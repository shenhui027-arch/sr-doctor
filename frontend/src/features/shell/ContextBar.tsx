import type { JSX, ReactNode } from "react";
import type {
  AnalysisSnapshotReport,
  Comparison,
  ConnectedProfileComparisonResponse,
  InputOrigin,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, EvidenceMeter, SegTabs, type EvidenceLevel } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { hasVersionPack, versionPackLabel } from "../../lib/version-pack";
import { evidenceGapItems } from "../findings/FindingsPanel";
import "./shell.css";

export type ContextBarMode = "diagnosis" | "compare";

function toEvidenceLevel(state: string): EvidenceLevel {
  if (state === "complete") return "complete";
  if (state === "partial") return "partial";
  if (state === "sparse") return "sparse";
  return "missing";
}

function reportIdentity(report: AnalysisSnapshotReport | undefined, fallbackQueryId?: string | null): { full: string; short: string } {
  const queryId = fallbackQueryId ?? report?.query_summary?.query_id ?? null;
  if (queryId) return { full: queryId, short: shortQueryId(queryId) };
  if (report) return { full: report.analysis_key, short: `${report.analysis_key.slice(0, 12)}…` };
  return { full: "—", short: "—" };
}

/**
 * Persistent analysis context bar shared across Doctor's Diagnosis and Compare modes
 * (boards/00-shell.html, 04-doctor.html, 05-compare.html, spec §8 & §9 item 7):
 * The left-hand "Diagnosis | Compare" SegTabs stays anchored at the exact same position in both modes;
 * the middle shows the current Profile or comparison identity; the right side shows mode-specific actions.
 */
export function ContextBar(props: {
  mode?: ContextBarMode;
  onModeChange?(mode: ContextBarMode): void;
  report?: AnalysisSnapshotReport | null;
  importing?: boolean;
  inputOrigin?: InputOrigin | null;
  observedAtUtc?: string | null;
  comparison?: Comparison | null;
  connectedComparison?: ConnectedProfileComparisonResponse | null;
  compareBaselineQueryId?: string | null;
  onReturnToQueries?(): void;
  actions?: ReactNode;
}): JSX.Element {
  const {
    mode = "diagnosis",
    onModeChange,
    report = null,
    importing = false,
    inputOrigin = null,
    observedAtUtc = null,
    comparison = null,
    connectedComparison = null,
    compareBaselineQueryId = null,
    onReturnToQueries,
    actions,
  } = props;
  const { text } = useLocale();

  const canReturnDiagnosis = mode === "diagnosis" && Boolean(report) && !importing && Boolean(inputOrigin && inputOrigin !== "offline_import");
  const canReturnCompare = mode === "compare" && Boolean(connectedComparison || compareBaselineQueryId);
  const returnTo = (canReturnDiagnosis || canReturnCompare) ? onReturnToQueries : undefined;

  return (
    <section aria-label={text("Current analysis context", "当前分析上下文栏")} className="context-bar">
      <div className="context-anchor">
        {onModeChange ? (
          <SegTabs
            aria-label={text("Doctor view", "诊断与对比视图切换")}
            items={[
              { value: "diagnosis", label: text("Diagnosis", "诊断") },
              { value: "compare", label: text("Compare", "对比") },
            ]}
            onValueChange={onModeChange}
            size="sm"
            value={mode}
          />
        ) : null}
        {returnTo ? (
          <>
            {onModeChange ? <span aria-hidden="true" className="context-sep" /> : null}
            <Button
              aria-label={text("Back to query list", "返回查询列表")}
              onClick={returnTo}
              size="sm"
              variant="ghost"
            >
              ‹ {text("Query list", "查询列表")}
            </Button>
          </>
        ) : null}
      </div>

      <div className="context-identity">
        {mode === "diagnosis"
          ? renderDiagnosisIdentity(report, importing, inputOrigin, observedAtUtc, text)
          : renderCompareIdentity(comparison, connectedComparison, compareBaselineQueryId, text)}
      </div>

      {actions !== undefined && actions !== null ? <div className="context-actions">{actions}</div> : null}
    </section>
  );
}

function renderDiagnosisIdentity(
  report: AnalysisSnapshotReport | null,
  importing: boolean,
  inputOrigin: InputOrigin | null,
  observedAtUtc: string | null,
  text: LocalizedText,
): ReactNode {
  if (!report || importing) {
    return (
      <span className="card-subtitle">
        {importing
          ? text("Import an offline Profile to diagnose", "导入离线 Profile 以开始诊断")
          : text("Pick a query or import a Profile to diagnose", "选择查询或导入 Profile 以开始诊断")}
      </span>
    );
  }
  const queryId = report.query_summary?.query_id ?? null;
  const version = report.version.effective_version ?? report.version.reported_version?.value ?? text("unknown", "未知");
  const evidence = report.completeness.summary.state;
  const gaps = evidenceGapItems(report, text);
  const clock = observedAtUtc && /T(\d\d:\d\d:\d\d)/.exec(observedAtUtc)?.[1];
  const packed = hasVersionPack(report.version);
  const gapSuffix = gaps.length ? ` · ${text(`${gaps.length} gap(s)`, `${gaps.length} 处缺口`)}` : "";
  const evidenceShort = `${text("Evidence", "证据")} ${localizedEnum(evidence, text)}${gapSuffix}`;
  const reasons = report.completeness.summary.reasons.map((reason) => localizedEnum(reason, text)).join(" · ");

  return (
    <>
      <span className="context-qid" title={queryId ?? report.analysis_key}>
        {text("Query", "查询")} {queryId ? shortQueryId(queryId) : `${report.analysis_key.slice(0, 12)}…`}
      </span>
      <span className="status-inline" title={localizedEnum(report.version.compatibility, text)}>
        <span
          aria-hidden="true"
          className={packed && report.version.compatibility === "compatible" ? "status-dot status-dot--ok" : "status-dot status-dot--warn"}
        />
        StarRocks {version} · {versionPackLabel(report.version, text)}
      </span>
      <span className="context-evidence">
        <EvidenceMeter detail={reasons || undefined} label={evidenceShort} level={toEvidenceLevel(evidence)} />
        <span>{evidenceShort}</span>
      </span>
      <span className="hide-lt-1440">
        {originLabel(inputOrigin ?? undefined, text)}{clock ? ` · ${clock} UTC` : ""}
      </span>
    </>
  );
}

function renderCompareIdentity(
  comparison: Comparison | null,
  connectedComparison: ConnectedProfileComparisonResponse | null,
  compareBaselineQueryId: string | null,
  text: LocalizedText,
): ReactNode {
  if (comparison) {
    const baseReport = comparison.baseline.outcome?.report;
    const candReport = comparison.candidate.outcome?.report;
    const baseId = reportIdentity(baseReport, connectedComparison?.baseline.query.query_id ?? null);
    const candId = reportIdentity(candReport, connectedComparison?.candidate.query.query_id ?? null);
    const dotClass = comparison.state === "comparable"
      ? "status-dot status-dot--ok"
      : comparison.state === "partially_comparable"
        ? "status-dot status-dot--warn"
        : "status-dot status-dot--critical";
    const baseVer = baseReport?.version.effective_version ?? baseReport?.version.reported_version?.value ?? null;
    const candVer = candReport?.version.effective_version ?? candReport?.version.reported_version?.value ?? null;
    const sameVersion = Boolean(baseVer && candVer && baseVer === candVer);

    return (
      <>
        <span
          className="context-qid"
          title={`${text("Baseline", "基线")} ${baseId.full} → ${text("Candidate", "候选")} ${candId.full}`}
        >
          {text("Baseline", "基线")} {baseId.short} → {text("Candidate", "候选")} {candId.short}
        </span>
        <span className="status-inline">
          <span aria-hidden="true" className={dotClass} />
          <span>{localizedEnum(comparison.state, text)}</span>
        </span>
        {baseVer && candVer ? (
          <span className="status-inline hide-lt-1440">
            <span aria-hidden="true" className={sameVersion ? "status-dot status-dot--ok" : "status-dot status-dot--warn"} />
            <span>StarRocks {baseVer} {sameVersion ? "=" : "≠"} {candVer}</span>
          </span>
        ) : null}
      </>
    );
  }

  if (compareBaselineQueryId) {
    return (
      <>
        <span className="context-qid" title={compareBaselineQueryId}>
          {text("Baseline", "基线")} {shortQueryId(compareBaselineQueryId)}
        </span>
        <span className="card-subtitle">{text("Select a candidate Profile to compare", "请选择候选 Profile 进行对比")}</span>
      </>
    );
  }

  return <span className="card-subtitle">{text("Compare before / after Profiles", "导入或选择前后两份 Profile 进行对比")}</span>;
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
