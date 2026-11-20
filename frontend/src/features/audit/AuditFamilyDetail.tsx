import type { JSX } from "react";
import type {
  AuditFamilyDetailResponse,
  AuditFamilyRepresentative,
  AuditOccurrencePresentation,
} from "../../../../contracts/generated/doctor-contracts";
import { Button } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { LimitationNotes } from "../shell/LimitationNotes";
import { SqlToggleButton } from "../shell/SqlText";
import { AuditMetricValue } from "./AuditMetricValue";
import { metricRawText } from "./audit-display";

export type AuditFamilyRepresentativeRole = "recent" | "slowest" | "profile_known";

export function AuditFamilyDetail({
  detail,
  parentWindow,
  onBrowseMembers,
  onFetchProfile,
  onReturn,
  onSelectRepresentative,
  onViewSql,
  profileLoading,
  shownSqlQueryId = null,
  sqlLoading,
  text,
}: {
  detail: AuditFamilyDetailResponse;
  parentWindow: string;
  onBrowseMembers(): void;
  onFetchProfile(occurrence: AuditOccurrencePresentation): void;
  onReturn(): void;
  onSelectRepresentative(occurrence: AuditOccurrencePresentation): void;
  onViewSql(occurrence: AuditOccurrencePresentation): void;
  profileLoading: boolean;
  /** Query ID whose SQL the detail panel shows now, so the representative button reads show or hide. */
  shownSqlQueryId?: string | null;
  sqlLoading: boolean;
  text: LocalizedText;
}): JSX.Element {
  const slowest = selectedRepresentative(detail.representatives.slowest);
  const profileKnown = selectedRepresentative(detail.representatives.profile_known);
  const family =
    detail.family.state === "exact_native_digest"
      ? `${detail.family.context.catalog ?? "—"} / ${detail.family.context.database ?? "—"} / ${detail.family.native_digest}`
      : `— (${localizedEnum(detail.family.reason, text)})`;

  return (
    <section aria-labelledby="audit-family-detail-title" className="audit-family-detail">
      <header className="audit-family-detail-header">
        <button className="btn btn-secondary btn--sm" onClick={() => onReturn()} type="button">
          {text("Return to parent window", "返回父窗口")}
        </button>
        <div>
          <h3 id="audit-family-detail-title">{text("Query family detail · exact family", "查询族详情 · 精确族")}</h3>
          <p className="mono">{family}</p>
        </div>
        <p className="audit-family-detail-window mono">
          {text("Parent fixed UTC window", "父窗口固定 UTC")} · {parentWindow}
        </p>
      </header>

      <div className="audit-family-detail-layout">
        <section aria-labelledby="audit-family-distribution-title" className="audit-family-distribution">
          <div>
            <h4 id="audit-family-distribution-title">{text("Duration distribution · server buckets", "耗时分布 · 服务端桶")}</h4>
            <p className="muted" title={detail.distribution.algorithm}>
              {text("Exact family members, bucketed by the server", "精确同类成员，由服务端分桶")}
            </p>
          </div>
          <p className="audit-family-detail-counts">
            {text("Executions", "执行次数")} {detail.distribution.population_count} · {text("with query time", "有查询耗时")} {detail.distribution.eligible_count}
          </p>
          <dl className="audit-family-buckets">
            {detail.distribution.buckets.map((bucket) => (
              <div key={bucket.bucket_id}>
                <dt title={`${bucket.lower_ms_inclusive} ms ≤ queryTime < ${bucket.upper_ms_exclusive} ms`}>
                  {bucket.display_label_seconds}
                </dt>
                <dd>
                  <meter
                    aria-label={`${bucket.display_label_seconds} ${bucket.count} / ${detail.distribution.eligible_count}`}
                    max={detail.distribution.eligible_count}
                    min="0"
                    value={bucket.count}
                  >
                    {bucket.count}
                  </meter>
                  <span className="mono">{bucket.count}</span>
                </dd>
              </div>
            ))}
          </dl>
          <dl aria-label={text("Executions left out of the distribution", "未计入分布的执行")} className="audit-family-exclusions">
            <div><dt title="SQL NULL">{text("No query time", "无查询耗时")}</dt><dd>{detail.distribution.exclusions.sql_null_count}</dd></div>
            <div><dt>{text("Negative time", "耗时为负")}</dt><dd>{detail.distribution.exclusions.negative_count}</dd></div>
            <div><dt>{text("Out of range", "超出范围")}</dt><dd>{detail.distribution.exclusions.overflow_count}</dd></div>
          </dl>
          <p className="audit-family-detail-note muted">
            {text(
              "Bucket order, labels, and counts are server facts; the client does not rebuild this distribution.",
              "桶顺序、标签和计数均为服务端事实；客户端不重建分布。",
            )}
          </p>
        </section>

        <section aria-labelledby="audit-family-representatives-title" className="audit-family-representatives">
          <h4 id="audit-family-representatives-title">{text("Returned representative executions", "返回的代表执行")}</h4>
          <RepresentativeCard
            label={text("Most recent", "最近执行")}
            onSelect={onSelectRepresentative}
            representative={detail.representatives.recent}
            role="recent"
            text={text}
          />
          <RepresentativeCard
            label={text("Slowest", "最慢执行")}
            onSelect={onSelectRepresentative}
            representative={detail.representatives.slowest}
            role="slowest"
            text={text}
          />
          <RepresentativeCard
            label={text("Profile evidence", "Profile 证据")}
            onSelect={onSelectRepresentative}
            representative={detail.representatives.profile_known}
            role="profile_known"
            text={text}
          />
          <section aria-label={text("Explicit representative actions", "显式代表操作")} className="audit-family-detail-actions">
            <h4>{text("Next steps", "下一步")}</h4>
            <p className="muted">
              {text(
                "Detail reading does not fetch SQL or Profile. These actions run only after a click.",
                "读取详情不会拉取 SQL 或 Profile；以下操作只在点击后执行。",
              )}
            </p>
            <div className="audit-family-detail-btn-row">
              <SqlToggleButton
                className="btn btn-secondary btn--sm"
                controls="audit-selected-sql"
                disabled={!slowest}
                expanded={Boolean(slowest) && shownSqlQueryId === slowest?.query.query_id}
                hideLabel={text("Hide slowest representative SQL", "收起最慢代表 SQL")}
                loading={sqlLoading}
                onToggle={() => {
                  if (slowest) onViewSql(slowest);
                }}
                showLabel={text("View SQL for slowest representative", "查看最慢代表 SQL")}
              />
              <Button
                disabled={!profileKnown || profileKnown.query.profile_fetch.state !== "available" || profileLoading}
                onClick={() => {
                  if (profileKnown) onFetchProfile(profileKnown);
                }}
                size="sm"
                title={profileKnown && profileKnown.query.profile_fetch.state !== "available" ? profileKnown.query.profile_fetch.reason : undefined}
                variant="primary"
              >
                {profileLoading ? text("Fetching…", "拉取中…") : text("Fetch Profile for evidence representative", "拉取证据代表 Profile")}
              </Button>
            </div>
            <Button onClick={onBrowseMembers} size="sm" variant="secondary">
              {text("Browse member rankings", "浏览成员排行")}
            </Button>
          </section>
        </section>
      </div>

      <section aria-label={text("Family detail evidence and limitations", "查询族详情证据与限制")} className="audit-family-detail-evidence">
        <p>
          <strong>{text("Profile evidence coverage", "Profile 证据覆盖")}</strong> · {text("Profile list", "Profile 列表")}{" "}
          {localizedEnum(detail.profile_evidence_coverage.profile_list_check, text)} ·{" "}
          {detail.profile_evidence_coverage.profile_list_returned}/{detail.profile_evidence_coverage.profile_list_limit} ·{" "}
          {text("candidates", "候选")} {detail.profile_evidence_coverage.candidate_count}/{detail.profile_evidence_coverage.candidate_limit} ·{" "}
          {text("cache", "缓存")} {detail.profile_evidence_coverage.successful_cache_retained}/{detail.profile_evidence_coverage.successful_cache_capacity}
        </p>
        <p className="muted">{text("A listed Profile may already have expired when you fetch it.", "列出的 Profile 在拉取时可能已过期。")}</p>
        <p className="muted">{text("Returning reloads the parent time window.", "返回时会重新加载上一级时间窗口。")}</p>
        <LimitationNotes codes={detail.limitations} text={text} />
      </section>
    </section>
  );
}

function RepresentativeCard({
  label,
  representative,
  role,
  onSelect,
  text,
}: {
  label: string;
  representative: AuditFamilyRepresentative;
  role: AuditFamilyRepresentativeRole;
  onSelect(occurrence: AuditOccurrencePresentation): void;
  text: LocalizedText;
}): JSX.Element {
  if (representative.state !== "selected") {
    return (
      <article className={`audit-family-representative audit-family-representative-${role}`}>
        <h5>{label}</h5>
        <p title={representative.reason}>
          {text("Unavailable", "不可用")} · {localizedEnum(representative.reason, text)}
        </p>
      </article>
    );
  }
  const occurrence = representative.occurrence;
  const queryTime = occurrence.metrics.find((metric) => metric.metric_id === "query_time") ?? null;
  const scanBytes = occurrence.metrics.find((metric) => metric.metric_id === "scan_bytes") ?? null;
  return (
    <article className={`audit-family-representative audit-family-representative-${role}`}>
      <h5>{label}</h5>
      <p className="audit-family-representative-id mono">{occurrence.query.query_id}</p>
      <p>
        {occurrence.query.observed_at_utc ?? occurrence.query.observed_at_local ?? "—"} ·{" "}
        <span title={metricRawText(queryTime)}><AuditMetricValue text={text} value={queryTime} /></span>
        {scanBytes && (
          <>
            {" · "}
            <span title={metricRawText(scanBytes)}><AuditMetricValue text={text} value={scanBytes} /></span>
          </>
        )}
      </p>
      {representative.profile_evidence && (
        <p title={representative.profile_evidence}>
          {text("Profile evidence", "Profile 证据")} · {localizedEnum(representative.profile_evidence, text)}
        </p>
      )}
      <Button
        aria-haspopup="dialog"
        aria-label={`${text("Select representative", "选择代表")} · ${label}`}
        onClick={() => onSelect(occurrence)}
        size="sm"
        variant="secondary"
      >
        {text("Select representative", "选择代表")}
      </Button>
    </article>
  );
}

function selectedRepresentative(representative: AuditFamilyRepresentative): AuditOccurrencePresentation | null {
  return representative.state === "selected" ? representative.occurrence : null;
}
