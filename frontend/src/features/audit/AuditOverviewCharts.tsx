// Audit overview helpers kept from the v0.5 charts: the bounded-source note,
// the server step lookup used by the in-flight samples, and the digest guidance.
import type { JSX } from "react";
import type { AuditBoundedSourceCoverage, AuditConcurrencyPoint } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { auditClockText } from "./audit-display";
import "./audit-overview-charts.css";

export function AuditBoundedSourceNote({ source, text }: { source: AuditBoundedSourceCoverage; text: LocalizedText }): JSX.Element {
  return (
    <div className="notice-banner notice-banner--warn audit-chart-boundary" data-source-coverage={source.state}>
      <div>
        <strong>{text("Partial source coverage", "来源覆盖不完整")}</strong> · {text("Fixed lookback", "固定回溯")} {source.lookback_seconds} s ·{" "}
        {text("Earlier long queries may be missing.", "更早开始的长查询可能遗漏。")}
        <div className="mono">
          {text("Source start-time range / reader zone", "来源开始时间范围 / 读取时区")} [{auditClockText(String(source.source_start_seconds * 1000))},{" "}
          {auditClockText(String(source.source_end_seconds * 1000))})
        </div>
      </div>
    </div>
  );
}

// Decimal-text lookup on the already ordered server segments. No timestamp
// conversion, query interval parsing, accumulation, or rank calculation.
export function auditSegmentAt(points: readonly AuditConcurrencyPoint[], at: string): AuditConcurrencyPoint | null {
  let result: AuditConcurrencyPoint | null = null;
  for (const point of points) {
    if (point.at_milliseconds.length > at.length || (point.at_milliseconds.length === at.length && point.at_milliseconds > at)) break;
    result = point;
  }
  return result;
}

export function AuditDigestGuidance({ text }: { text: LocalizedText }): JSX.Element {
  return (
    <article aria-label={text("SQL digest unavailable", "官方 SQL digest 不可用")} className="notice-banner audit-digest-guidance" role="note">
      <div>
        <strong>{text("Grouping by official digest is unavailable for this period", "当前时段无法按官方 digest 归组")}</strong>
        <p>{text("Queries below are grouped by unofficial fingerprint instead.", "下方改按非官方指纹归组。")}</p>
        {/* Review: the cluster setting is background, not the first thing to do. */}
        <details className="disclosure">
          <summary>{text("Why there is no official digest", "为什么没有官方 digest")}</summary>
          <div className="disclosure-body">
            <p>
              {text(
                "None of these audit records carries a SQL digest, which usually means the FE config enable_sql_digest is off. A DBA can turn it on with:",
                "这些审计记录都没有官方 SQL digest，通常是 FE 配置 enable_sql_digest 未开启。可由 DBA 执行：",
              )}
            </p>
            <pre className="sql-well"><code>ADMIN SET FRONTEND CONFIG (&quot;enable_sql_digest&quot; = &quot;true&quot;);</code></pre>
            <p className="muted">
              {text(
                "Only queries recorded after that get an official digest. This tool never changes cluster settings.",
                "开启后新记录的查询才有官方 digest。本工具不会修改集群配置。",
              )}
            </p>
          </div>
        </details>
      </div>
    </article>
  );
}
