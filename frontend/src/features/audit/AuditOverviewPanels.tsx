// Design board 01 (查询 · 审计历史): the KPI strip, the in-flight sample
// card, the repeated-SQL table and the selected-family aside. Every number is
// a server fact rendered through the shared display policy; nothing here
// aggregates, ranks or converts units.
import type { JSX } from "react";
import type {
  AuditConcurrencyPresentation,
  AuditCoverage,
  AuditFamilyPresentation,
  AuditFamilySort,
  AuditFingerprintFamilyResponse,
  AuditMetricFact,
  AuditMetricId,
  AuditOccurrencePresentation,
  AuditOccurrenceResponse,
  AuditOverviewFamilyTopPresentation,
  ProductSqlFingerprintUnavailableReason,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { compactCountText, countText } from "../../lib/duration-display";
import { compactQueryId } from "../../lib/query-id";
import { ExpandableSqlLine, SqlText, SqlToggleButton } from "../shell/SqlText";
import { auditSegmentAt } from "./AuditOverviewCharts";
import { auditClockText, metricRawText, metricText, metricTone } from "./audit-display";
import "./audit-overview-panels.css";

const SAMPLE_POINTS = 30;

export function AuditKpiStrip({ coverage, concurrency, onSelectTime, disabled = false, text }: {
  coverage: AuditCoverage;
  concurrency: AuditConcurrencyPresentation | null;
  onSelectTime?(atMilliseconds: string): void;
  disabled?: boolean;
  text: LocalizedText;
}): JSX.Element {
  const input = (id: AuditMetricId) => coverage.metric_inputs.find((item) => item.metric_id === id) ?? null;
  const queryTime = input("query_time");
  const cpu = input("cpu_cost");
  const scanBytes = input("scan_bytes");
  const missingScan = scanBytes && scanBytes.sql_null_count !== "0" ? scanBytes.sql_null_count : null;
  const errors = coverage.error_count;
  return <section aria-label={text("Audit full-window summary", "审计全窗口摘要")} className="audit-kpi-row">
    <div className="audit-kpi-grid">
      <AuditKpi label={text("Query count", "查询次数")} note={text("deduplicated retained events", "已去重保留事件")} value={countText(coverage.retained_event_count) ?? coverage.retained_event_count} />
      <AuditKpi label={text("Cumulative query time", "累计查询耗时")} metric={queryTime?.aggregate.metric ?? null} note={text("summed across concurrency, not elapsed", "并发累加，非经过时间")} />
      <AuditKpi label={text("Cumulative CPU", "累计 CPU")} metric={cpu?.aggregate.metric ?? null} note={text("audit cpuCostNs", "审计 cpuCostNs")} />
      <AuditKpi label={text("Cumulative scanned bytes", "累计扫描数据量")} metric={scanBytes?.aggregate.metric ?? null} note={missingScan ? text(`${missingScan} records lack this field`, `${missingScan} 条记录缺失该字段`) : text("audit scanBytes", "审计 scanBytes")} />
      <AuditKpi critical={errors !== "0"} label={text("Failed executions", "执行失败")} note={text("ERR terminal state", "ERR 终态")} value={countText(errors) ?? errors} />
    </div>
    <AuditInFlightCard concurrency={concurrency} disabled={disabled} onSelectTime={onSelectTime} text={text} />
  </section>;
}

function AuditKpi({ critical = false, label, metric, note, value }: { critical?: boolean; label: string; metric?: AuditMetricFact | null; note: string; value?: string }): JSX.Element {
  const shown = value ?? metricText(metric ?? null);
  const tone = value !== undefined ? "exact" : metricTone(metric ?? null);
  return <div className={`audit-kpi audit-kpi-${critical ? "critical" : tone}`}>
    <div className="audit-kpi-label">{label}</div>
    <div className="audit-kpi-value" title={metric ? metricRawText(metric) : undefined}>{shown}</div>
    <div className="audit-kpi-note">{note}</div>
  </div>;
}

/**
 * Historical in-flight count read at 30 evenly spaced instants of the
 * window. Each bar is the server's exact step value at that instant (a
 * lookup, not an aggregate); its height is the server's y projection.
 */
export function AuditInFlightCard({ concurrency, disabled, onSelectTime, text }: { concurrency: AuditConcurrencyPresentation | null; disabled: boolean; onSelectTime?(at: string): void; text: LocalizedText }): JSX.Element {
  const samples = concurrency ? inFlightSamples(concurrency) : [];
  const peak = samples.reduce<number>((best, sample, index) => sample.permyriad > (samples[best]?.permyriad ?? -1) ? index : best, -1);
  const ticks = concurrency?.axis.time_ticks ?? [];
  const range = ticks.length >= 2 ? `${clock(ticks[0].label)}–${clock(ticks[ticks.length - 1].label)}` : "—";
  return <div className="audit-inflight">
    <div className="audit-inflight-head"><span>{text("In-flight queries · 30 point samples", "在途查询 · 30 点取样")}</span><span className="audit-inflight-range">{range}</span></div>
    {concurrency?.series.state === "exact"
      ? <div className="audit-inflight-bars">{samples.map((sample, index) => <button
          aria-label={`${auditClockText(sample.at)} · ${text("in flight", "在途")} ${sample.count}`}
          className={index === peak ? "peak" : undefined}
          disabled={disabled || !onSelectTime}
          key={sample.at}
          onClick={() => onSelectTime?.(sample.at)}
          style={{ height: `${Math.max(2, sample.permyriad / 100)}%` }}
          title={`${auditClockText(sample.at)} · ${text("in flight", "在途")} ${sample.count}`}
          type="button"
        />)}</div>
      : <p className="audit-inflight-unavailable">{text("In-flight history unavailable for this audit table.", "该审计表无法重建在途历史。")}</p>}
    <div className="audit-inflight-note">{text("Hover for source values · cumulative time is not elapsed time", "悬停查看原值 · 累计耗时不等于实际经过时间")}</div>
  </div>;
}

/** Instants are the midpoints of 30 equal slices of the server window (integer ms). */
export function inFlightSamples(concurrency: AuditConcurrencyPresentation): { at: string; count: string; permyriad: number }[] {
  if (concurrency.series.state !== "exact") return [];
  const start = Number(concurrency.window.start_milliseconds);
  const end = Number(concurrency.window.end_milliseconds);
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || end <= start) return [];
  const points = concurrency.series.points;
  return Array.from({ length: SAMPLE_POINTS }, (_, index) => {
    const at = String(Math.floor(start + (end - start) * (2 * index + 1) / (2 * SAMPLE_POINTS)));
    const segment = auditSegmentAt(points, at);
    return { at, count: segment?.active_count ?? "0", permyriad: segment?.y_permyriad ?? 0 };
  });
}

function clock(label: string): string {
  return /^\d\d:\d\d/.test(label) ? label.slice(0, 5) : label;
}

const familyColumns: { id: AuditMetricId; aggregation: "sum" | "max"; sort: AuditFamilySort }[] = [
  { id: "query_time", aggregation: "sum", sort: "cumulative_query_time_desc" },
  { id: "query_time", aggregation: "max", sort: "max_query_time_desc" },
  { id: "scan_bytes", aggregation: "sum", sort: "cumulative_scan_bytes_desc" },
];

export function AuditFamilyList({ families, familyTop, onSelect, selectedKey, sort, disabled = false, text }: {
  families: AuditFamilyPresentation[];
  /** While a read is in flight a selection would be ignored; say so instead. */
  disabled?: boolean;
  familyTop: AuditOverviewFamilyTopPresentation | null;
  onSelect(family: AuditFamilyPresentation): void;
  selectedKey: string | null;
  sort: AuditFamilySort;
  text: LocalizedText;
}): JSX.Element {
  // A no-break space keeps the arrow with its word when a long header wraps.
  const arrow = (columnSort: AuditFamilySort) => sort === columnSort ? " ↓" : "";
  const headers = [text("Cumulative query time", "累计查询耗时") + arrow("cumulative_query_time_desc"), text("Maximum query time", "最大查询耗时") + arrow("max_query_time_desc"), text("Cumulative scanned bytes", "累计扫描数据量") + arrow("cumulative_scan_bytes_desc")];
  return <div className="audit-family-list-wrap"><table aria-label={text("Audited query families", "审计查询族")} className="audit-family-list">
    <thead><tr>
      <th scope="col">{text("SQL summary", "SQL 摘要")}</th>
      <th className="num" scope="col">{text("Executions", "执行次数")}{arrow("execution_count_desc")}</th>
      {headers.map((header) => <th className="num" key={header} scope="col">{header}</th>)}
      <th className="num last" scope="col">{text("Failed", "失败")}{arrow("error_count_desc")}</th>
    </tr></thead>
    <tbody>{families.map((family, index) => {
      const key = family.membership.state === "exact_native_digest" ? family.membership.family_key : `ungrouped:${index}`;
      const selected = key === selectedKey;
      const share = familyShare(family, familyTop);
      return <tr aria-selected={selected} className={selected ? "selected" : undefined} key={key} onClick={() => { if (!disabled) onSelect(family); }}>
        <td className="audit-family-summary-cell">
          <div className="audit-family-id">
            <button aria-label={`${text("Select query family", "选择查询族")} ${family.membership.state === "exact_native_digest" ? family.membership.native_digest : ""}`} aria-pressed={selected} disabled={disabled || family.membership.state !== "exact_native_digest"} onClick={(event) => { event.stopPropagation(); onSelect(family); }} type="button">{family.membership.state === "exact_native_digest" ? family.membership.native_digest.slice(0, 8) : text("Not grouped", "未归组")}</button>
            <span aria-hidden="true" className="audit-family-share"><span style={{ width: share === null ? 0 : `${share / 100}%` }} /></span>
          </div>
          <FamilyStatement family={family} id={`audit-family-sql-${key}`} text={text} />
        </td>
        <td className="num">{family.execution_count ?? "—"}</td>
        {familyColumns.map((column) => {
          const metric = family.aggregates.find((item) => item.metric.metric_id === column.id && item.aggregation === column.aggregation)?.metric ?? null;
          return <td className="num" key={`${column.id}-${column.aggregation}`} title={metricRawText(metric)}>{metricTone(metric) === "missing"
            ? <span className="audit-missing-tag" title={text("The audit did not provide this field; it is not counted as 0", "审计未提供该字段，不按 0 计")}>{text("Missing", "缺失")}</span>
            : metricText(metric)}</td>;
        })}
        <td className={`num last${family.error_count && family.error_count !== "0" ? " failed" : ""}`}>{family.error_count ?? "—"}</td>
      </tr>;
    })}</tbody>
  </table></div>;
}

const unavailableReasonLabels: Record<ProductSqlFingerprintUnavailableReason, [string, string]> = {
  statement_sql_null: ["stmt is NULL", "stmt 为空"],
  statement_not_read: ["statement not read", "未读到语句"],
  conflicting_source_rows: ["conflicting audit rows", "审计记录冲突"],
  empty_statement: ["empty statement", "空语句"],
  lexical_error: ["unparseable text", "无法词法解析"],
  unbalanced_parentheses: ["unbalanced parentheses", "括号不匹配"],
  multiple_statements: ["multiple statements", "多条语句"],
  unsupported_statement: ["not SELECT/INSERT/UPDATE/DELETE", "非 SELECT/INSERT/UPDATE/DELETE"],
  massive_compound_predicate: ["16+ AND/OR terms", "AND/OR 条件过多"],
};

/**
 * Product-fingerprint groups of executions without a native digest. A
 * separate list from exact families, labelled from the server's authority
 * fact; the template, counters and metrics are rendered exactly as received.
 */
export function AuditFingerprintFamilyList({ response, error, loading, text }: {
  response: AuditFingerprintFamilyResponse | null;
  error: string | null;
  loading: boolean;
  text: LocalizedText;
}): JSX.Element {
  const coverage = response?.coverage;
  const unofficial = response?.authority === "product_unofficial";
  const excluded = coverage?.unavailable.map((item) => `${text(...unavailableReasonLabels[item.reason])} ${item.count}`).join(" · ");
  return <section aria-labelledby="audit-fingerprint-title" className="audit-fingerprint-families">
    <div className="audit-fingerprint-head">
      <h2 id="audit-fingerprint-title">{text("Executions without a native digest · grouped by template", "无原生 digest 的执行 · 按模板归组")}</h2>
      {unofficial && <span className="audit-fingerprint-badge" title={`${response.fingerprint_version} · ${text("made by this tool from the audited statement; not a StarRocks digest", "由本工具根据审计语句生成，不是 StarRocks digest")}`}>{text("Unofficial fingerprint", "非官方指纹")}</span>}
    </div>
    {error ? <p className="audit-error" role="alert">{error}</p>
      : loading && !response ? <p className="audit-empty" role="status">{text("Grouping statements…", "正在按模板归组…")}</p>
      : coverage && response && <>
        <p className="audit-fingerprint-coverage">
          {coverage.source.state === "partial"
            ? text(`Newest ${coverage.read_event_count} of ${coverage.eligible_event_count} executions read`, `读取了 ${coverage.eligible_event_count} 次中最近的 ${coverage.read_event_count} 次执行`)
            : text(`All ${coverage.read_event_count} executions read`, `已读取全部 ${coverage.read_event_count} 次执行`)}
          {" · "}{text(`${coverage.fingerprinted_event_count} grouped into ${coverage.family_count} templates`, `${coverage.fingerprinted_event_count} 次归入 ${coverage.family_count} 个模板`)}
          {coverage.partial_event_count !== "0" && <>{" · "}<span title={text("The statement is longer than the read prefix; it is neither grouped nor ranked", "语句超出读取前缀，不参与归组与排序")}>{text(`truncated ${coverage.partial_event_count}`, `截断 ${coverage.partial_event_count}`)}</span></>}
          {excluded && <>{" · "}<span>{text("not grouped", "未归组")}: {excluded}</span></>}
        </p>
        {response.rows.length === 0
          ? <p className="audit-empty">{text("No statement could be grouped.", "没有可归组的语句。")}</p>
          : <div className="audit-family-list-wrap"><table aria-label={text("Unofficial fingerprint groups", "非官方指纹分组")} className="audit-family-list">
            <thead><tr>
              <th scope="col">{text("Template (literals as ?)", "模板（字面量为 ?）")}</th>
              <th className="num" scope="col">{text("Executions", "执行次数")}</th>
              {familyColumns.map((column) => <th className="num" key={column.sort} scope="col">{familyColumnLabel(column.sort, text)}</th>)}
              <th className="num last" scope="col">{text("Failed", "失败")}</th>
            </tr></thead>
            <tbody>{response.rows.map((family) => <tr key={family.family_key}>
              <td className="audit-family-summary-cell">
                <ExpandableSqlLine copyLabel={text("Copy template", "复制模板")} id={`audit-fingerprint-template-${family.family_key}`} label={text("Unofficial fingerprint template", "非官方指纹模板")} lineClassName="audit-family-sql" note={`${family.context.catalog ?? "—"} / ${family.context.database ?? "—"}`} value={family.template} />
              </td>
              <td className="num">{family.execution_count}</td>
              {familyColumns.map((column) => {
                const metric = family.aggregates.find((item) => item.metric.metric_id === column.id && item.aggregation === column.aggregation)?.metric ?? null;
                return <td className="num" key={column.sort} title={metricRawText(metric)}>{metricTone(metric) === "missing"
                  ? <span className="audit-missing-tag" title={text("No execution in this group provided the field; it is not counted as 0", "该组没有执行提供此字段，不按 0 计")}>{text("Missing", "缺失")}</span>
                  : metricText(metric)}</td>;
              })}
              <td className={`num last${family.error_count !== "0" ? " failed" : ""}`}>{family.error_count}</td>
            </tr>)}</tbody>
          </table></div>}
      </>}
  </section>;
}

function familyColumnLabel(sort: AuditFamilySort, text: LocalizedText): string {
  return sort === "cumulative_query_time_desc" ? text("Cumulative query time", "累计查询耗时")
    : sort === "max_query_time_desc" ? text("Maximum query time", "最大查询耗时")
    : text("Cumulative scanned bytes", "累计扫描数据量");
}

/** The plotted share comes only from the server's Top projection of the same family. */
function familyShare(family: AuditFamilyPresentation, familyTop: AuditOverviewFamilyTopPresentation | null): number | null {
  if (!familyTop || family.membership.state !== "exact_native_digest") return null;
  const key = family.membership.family_key;
  const row = familyTop.rows.find((item) => item.family.membership.state === "exact_native_digest" && item.family.membership.family_key === key);
  return row?.plot.state === "exact" ? row.plot.permyriad : null;
}

function FamilyStatement({ family, id, text }: { family: AuditFamilyPresentation; id: string; text: LocalizedText }): JSX.Element {
  const sample = family.statement_sample;
  if (sample.state === "text" && sample.value.trim() !== "") return <ExpandableSqlLine copyLabel={sample.truncated ? text("Copy returned prefix", "复制已返回片段") : text("Copy SQL", "复制 SQL")} id={id} label={text("SQL of the most recent member, as audited", "最近一次成员的审计原文")} lineClassName="audit-family-sql" note={text("Most recent member, as audited", "最近一次成员的审计原文")} truncated={sample.truncated} value={sample.value} />;
  if (sample.state === "text") return <div className="audit-family-sql muted">{text("audited stmt is empty", "审计 stmt 为空字符串")}</div>;
  const context = family.membership.state === "exact_native_digest" ? `${family.membership.context.catalog ?? "—"} / ${family.membership.context.database ?? "—"}` : "—";
  const note = sample.state === "sql_null" ? text("audited stmt is NULL", "审计 stmt 为空") : sample.reason === "source_column_absent" ? text("no stmt column in the audit table", "审计表无 stmt 列") : context;
  return <div className="audit-family-sql muted">{note}</div>;
}

export type FamilyMemberState = "fetched" | "available" | "unavailable";
/** none: no baseline; selected: this member is it; comparable: it is in this list; elsewhere: set from another list. */
export type BaselineState = "none" | "selected" | "comparable" | "elsewhere";

export function AuditFamilyAside({ family, members, memberSql = null, memberSqlError = null, memberSqlLoading = false, onViewMemberSql, loading, error, selectedMemberKey, fetchedQueryIds, profileLoading, canDiagnose, canPlanCheck, baseline = "none", onClose, onSelectMember, onDiagnose, onSetBaseline, onCompareWithBaseline, onPlanCheck, onShowMembers, text }: {
  family: AuditFamilyPresentation;
  members: AuditOccurrenceResponse | null;
  /** Full SQL of the selected member, once read; null while hidden. */
  memberSql?: string | null;
  memberSqlError?: string | null;
  memberSqlLoading?: boolean;
  onViewMemberSql?(): void;
  loading: boolean;
  error: string | null;
  selectedMemberKey: string | null;
  fetchedQueryIds: ReadonlySet<string>;
  profileLoading: boolean;
  canDiagnose: boolean;
  canPlanCheck: boolean;
  baseline?: BaselineState;
  onClose(): void;
  onSelectMember(key: string): void;
  onDiagnose(): void;
  onSetBaseline?(): void;
  onCompareWithBaseline?(): void;
  onPlanCheck?(): void;
  onShowMembers(): void;
  text: LocalizedText;
}): JSX.Element {
  const exact = family.membership.state === "exact_native_digest" ? family.membership : null;
  const sample = family.statement_sample;
  const rows = members?.rows.slice(0, 4) ?? [];
  // The count is the entry to the full member table and the family distribution.
  const shownOf = text(`top ${rows.length} of ${family.execution_count ?? "—"}`, `${family.execution_count ?? "—"} 次中前 ${rows.length}`);
  return <aside aria-label={text("Selected query family", "已选查询族")} className="audit-family-aside">
    <div className="audit-family-aside-head">
      <div className="audit-family-aside-title">
        <span>{text("Selected query family · digest", "已选查询族 · digest")} <code>{exact?.native_digest.slice(0, 12) ?? "—"}</code></span>
        <button aria-label={text("Close details", "关闭详情")} onClick={onClose} type="button"><svg aria-hidden="true" fill="none" height="16" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="16"><path d="M6 6l12 12M18 6L6 18" /></svg></button>
      </div>
      {sample.state === "text" && sample.value.trim() !== ""
        ? <SqlText copyLabel={sample.truncated ? text("Copy returned prefix", "复制已返回片段") : text("Copy SQL", "复制 SQL")} label={text("SQL of the most recent member, as audited", "最近一次成员的审计原文")} note={sample.truncated ? text("Most recent member · sample truncated (prefix only); a member's full SQL is below", "最近一次成员 · 样本已截断（只返回前缀），完整 SQL 可在下方成员中查看") : text("Most recent member, as audited", "最近一次成员的审计原文")} value={sample.value} />
        : <p className="audit-family-aside-empty">{sample.state === "text" ? text("Audited stmt is empty.", "审计 stmt 为空字符串。") : sample.state === "sql_null" ? text("Audited stmt is NULL.", "审计 stmt 为空。") : text("No SQL sample for this family.", "该查询族没有 SQL 示例。")}</p>}
      <div className="audit-family-aside-context"><span>{text("Database", "数据库")} {exact?.context.database ?? "—"}</span><span>Catalog {exact?.context.catalog ?? "—"}</span></div>
    </div>
    <div className="audit-family-members-head">
      <h2>{text("Member executions · by duration", "成员执行 · 按耗时")}</h2>
      {members
        ? <button aria-label={`${shownOf} · ${text("All members and distribution", "全部成员与耗时分布")}`} className="audit-family-members-more" onClick={onShowMembers} title={text("All members and distribution", "全部成员与耗时分布")} type="button">{shownOf}</button>
        : <span>{loading ? text("Loading…", "加载中…") : ""}</span>}
    </div>
    {error && <p className="audit-family-aside-error" role="alert">{error}</p>}
    <ul className="audit-family-members">{rows.map((row, index) => {
      const key = memberKey(row, index);
      const fetch = row.query.profile_fetch;
      const state: FamilyMemberState = fetchedQueryIds.has(row.query.query_id) || (fetch.state === "available" && fetch.reason === "profile_fetched") ? "fetched" : fetch.state;
      return <li className={key === selectedMemberKey ? "selected" : undefined} key={key}>
        <button aria-pressed={key === selectedMemberKey} onClick={() => onSelectMember(key)} title={row.query.query_id} type="button">
          <span className="id">{compactQueryId(row.query.query_id)}</span>
          <span className="time" title={metricRawText(memberMetric(row, "query_time"))}>{metricText(memberMetric(row, "query_time"))}</span>
          <span className="meta">{memberClock(row)} · {text("scanned", "扫描")} {memberRows(row, text)}</span>
          <span className={`state state-${state}`}>{state === "fetched" ? text("Profile fetched", "Profile 已拉取") : state === "available" ? text("Can try fetching", "可尝试拉取") : text("Not fetchable yet", "暂不可拉取")}</span>
        </button>
      </li>;
    })}</ul>
    <div className="audit-family-aside-foot">
      <p>{text("Reads existing Profiles only; SQL is never rerun. A Profile may have expired.", "只读取既有 Profile，不会重新执行 SQL。Profile 缓存可能已过期。")}</p>
      <div className="audit-family-aside-actions">
        <button className="button accent primary" disabled={!canDiagnose || profileLoading} onClick={onDiagnose} type="button">{profileLoading ? text("Fetching Profile…", "正在拉取 Profile…") : text("Fetch Profile and diagnose", "拉取 Profile 并诊断")}<svg aria-hidden="true" fill="none" height="14" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M5 12h14M13 6l6 6-6 6" /></svg></button>
        {baseline === "comparable" && onCompareWithBaseline
          ? <button className="button secondary" disabled={!canDiagnose || profileLoading} onClick={onCompareWithBaseline} title={text("Compare the baseline with this execution; both are in this list", "将基线与这次执行对比；两者都在本列表中")} type="button">{text("Compare with baseline", "与基线对比")}</button>
          : baseline === "selected"
            ? <button className="button secondary" disabled type="button">{text("Baseline set", "已设为基线")}</button>
            : <button className="button secondary" disabled={!canDiagnose || profileLoading || !onSetBaseline} onClick={onSetBaseline} title={baseline === "elsewhere" ? text("The current baseline is not in this list; this execution would replace it", "当前基线不在此列表中；设为基线会替换它") : undefined} type="button">{text("Set as baseline", "设为基线")}</button>}
        <button className="button secondary" disabled={!canPlanCheck || !onPlanCheck} onClick={onPlanCheck} type="button">{text("Plan check", "计划检查")}</button>
        {onViewMemberSql && <SqlToggleButton controls="audit-family-member-sql" disabled={!selectedMemberKey} expanded={memberSql !== null} hideLabel={text("Hide member SQL", "收起成员 SQL")} loading={memberSqlLoading} onToggle={onViewMemberSql} showLabel={text("View member full SQL", "查看成员完整 SQL")} />}
      </div>
      {memberSqlError && <p className="audit-family-aside-error" role="alert">{memberSqlError}</p>}
      {memberSql !== null && <SqlText copyLabel={text("Copy SQL", "复制 SQL")} id="audit-family-member-sql" label={text("Selected member SQL", "所选成员 SQL")} value={memberSql} />}
    </div>
  </aside>;
}

export function memberKey(row: AuditOccurrencePresentation, index: number): string {
  return `${row.query.query_id}:${row.query.source_refs[0] ?? `member:${index}`}`;
}

function memberMetric(row: AuditOccurrencePresentation, id: AuditMetricId): AuditMetricFact | null {
  return row.metrics.find((item) => item.metric_id === id) ?? null;
}

/** The audited local clock text as returned (hh:mm:ss), never a converted zone. */
function memberClock(row: AuditOccurrencePresentation): string {
  const local = row.query.observed_at_local ?? row.query.observed_at_utc ?? "—";
  const match = /(\d\d:\d\d:\d\d)/.exec(local);
  return match ? match[1] : local;
}

function memberRows(row: AuditOccurrencePresentation, text: LocalizedText): string {
  const metric = memberMetric(row, "scan_rows");
  if (!metric || metric.state !== "exact" || !metric.canonical_number) return `— ${text("missing", "缺失")}`;
  return `${compactCountText(metric.canonical_number.value) ?? metric.canonical_number.value} ${text("rows", "行")}`;
}
