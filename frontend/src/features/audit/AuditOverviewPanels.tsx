// Design board 03 (查询 · 审计历史): the KPI strip, the in-flight sample
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
  AuditMetricId,
  AuditOccurrencePresentation,
  AuditOverviewFamilyTopPresentation,
  ProductSqlFingerprintUnavailableReason,
} from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Button, InfoTip, KpiCard, StateStamp } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { countText } from "../../lib/duration-display";
import { ExpandableSqlLine } from "../shell/SqlText";
import { AuditMetricValue } from "./AuditMetricValue";
import { auditSegmentAt } from "./AuditOverviewCharts";
import { auditClockText, metricRawText } from "./audit-display";
import "./audit-overview-panels.css";

const SAMPLE_POINTS = 30;

export function AuditKpiStrip({
  coverage,
  concurrency,
  onSelectTime,
  disabled = false,
  text,
}: {
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

  return (
    <section aria-label={text("Audit full-window summary", "审计全窗口摘要")} className="kpi-grid kpi-grid--6 audit-kpi-strip">
      <KpiCard
        info={text("deduplicated retained events", "已去重保留事件")}
        label={text("Query count", "查询次数")}
        value={countText(coverage.retained_event_count) ?? coverage.retained_event_count}
      />
      <KpiCard
        foot={text("summed across concurrency, not elapsed", "并发累加，非墙钟经过时间")}
        label={text("Cumulative query time", "累计查询耗时")}
        value={
          <span title={metricRawText(queryTime?.aggregate.metric ?? null)}>
            <AuditMetricValue text={text} value={queryTime?.aggregate.metric ?? null} />
          </span>
        }
      />
      <KpiCard
        info={text("audit cpuCostNs", "审计 cpuCostNs")}
        label={text("Cumulative CPU", "累计 CPU")}
        value={
          <span title={metricRawText(cpu?.aggregate.metric ?? null)}>
            <AuditMetricValue text={text} value={cpu?.aggregate.metric ?? null} />
          </span>
        }
      />
      <KpiCard
        foot={
          missingScan ? (
            <span className="kpi-foot--warn">
              {text(`${missingScan} records lack this field`, `${missingScan} 条记录缺失该字段`)}
            </span>
          ) : undefined
        }
        info={text("audit scanBytes; missing rows are never counted as 0", "审计 scanBytes；缺失行不按 0 计")}
        label={text("Cumulative scanned bytes", "累计扫描数据量")}
        value={
          <span title={metricRawText(scanBytes?.aggregate.metric ?? null)}>
            <AuditMetricValue text={text} value={scanBytes?.aggregate.metric ?? null} />
          </span>
        }
      />
      <KpiCard
        info={text("ERR terminal state", "ERR 终态")}
        label={text("Failed executions", "执行失败")}
        tone={errors !== "0" ? "critical" : "default"}
        value={countText(errors) ?? errors}
      />
      <AuditInFlightCard concurrency={concurrency} disabled={disabled} onSelectTime={onSelectTime} text={text} />
    </section>
  );
}

/**
 * Historical in-flight count read at 30 evenly spaced instants of the
 * window. Each bar is the server's exact step value at that instant (a
 * lookup, not an aggregate); its height is the server's y projection.
 */
export function AuditInFlightCard({
  concurrency,
  disabled,
  onSelectTime,
  text,
}: {
  concurrency: AuditConcurrencyPresentation | null;
  disabled: boolean;
  onSelectTime?(at: string): void;
  text: LocalizedText;
}): JSX.Element {
  const samples = concurrency ? inFlightSamples(concurrency) : [];
  const peak = samples.reduce<number>(
    (best, sample, index) => (sample.permyriad > (samples[best]?.permyriad ?? -1) ? index : best),
    -1,
  );
  const ticks = concurrency?.axis.time_ticks ?? [];
  const range = ticks.length >= 2 ? `${clock(ticks[0].label)}–${clock(ticks[ticks.length - 1].label)}` : "—";

  return (
    <article className="card card--kpi audit-inflight">
      <div className="kpi-head">
        <span className="kpi-label">{text("In-flight · 30 samples", "在途查询 · 30 点取样")}</span>
        <InfoTip label={text("About in-flight samples", "关于在途取样")} side="top">
          {text("Click a bar to read retained queries covering that instant. Cumulative time is not elapsed time.", "点击柱条可查看该时刻的历史在途查询。累计耗时不等于实际经过时间。")}
        </InfoTip>
      </div>
      {concurrency?.series.state === "exact" ? (
        <div className="audit-inflight-bars">
          {samples.map((sample, index) => (
            <button
              aria-label={`${auditClockText(sample.at)} · ${text("in flight", "在途")} ${sample.count}`}
              className={index === peak ? "peak" : undefined}
              disabled={disabled || !onSelectTime}
              key={sample.at}
              onClick={() => onSelectTime?.(sample.at)}
              style={{ height: `${Math.max(4, sample.permyriad / 100)}%` }}
              title={`${auditClockText(sample.at)} · ${text("in flight", "在途")} ${sample.count}`}
              type="button"
            />
          ))}
        </div>
      ) : (
        <p className="audit-inflight-unavailable">
          {text("In-flight history unavailable for this audit table.", "该审计表无法重建在途历史。")}
        </p>
      )}
      <div className="kpi-foot mono">{range}</div>
    </article>
  );
}

/** Instants are the midpoints of 30 equal slices of the server window (integer ms). */
export function inFlightSamples(concurrency: AuditConcurrencyPresentation): { at: string; count: string; permyriad: number }[] {
  if (concurrency.series.state !== "exact") return [];
  const start = Number(concurrency.window.start_milliseconds);
  const end = Number(concurrency.window.end_milliseconds);
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || end <= start) return [];
  const points = concurrency.series.points;
  return Array.from({ length: SAMPLE_POINTS }, (_, index) => {
    const at = String(Math.floor(start + ((end - start) * (2 * index + 1)) / (2 * SAMPLE_POINTS)));
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

export function AuditFamilyList({
  families,
  familyTop,
  onSelect,
  selectedKey,
  sort,
  disabled = false,
  text,
}: {
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
  const arrow = (columnSort: AuditFamilySort) => (sort === columnSort ? "\u00a0↓" : "");
  const headers = [
    text("Cumulative query time", "累计查询耗时") + arrow("cumulative_query_time_desc"),
    text("Maximum query time", "最大查询耗时") + arrow("max_query_time_desc"),
    text("Cumulative scanned bytes", "累计扫描数据量") + arrow("cumulative_scan_bytes_desc"),
  ];
  return (
    <div className="audit-family-list-wrap">
      <table aria-label={text("Audited query families", "审计查询族")} className="data-table audit-family-list">
        <thead>
          <tr>
            <th scope="col">{text("SQL summary", "SQL 摘要")}</th>
            <th className="num" scope="col">{text("Executions", "执行次数")}{arrow("execution_count_desc")}</th>
            {headers.map((header) => (
              <th className="num" key={header} scope="col">{header}</th>
            ))}
            <th className="num last" scope="col">{text("Failed", "失败")}{arrow("error_count_desc")}</th>
          </tr>
        </thead>
        <tbody>
          {families.map((family, index) => {
            const key = family.membership.state === "exact_native_digest" ? family.membership.family_key : `ungrouped:${index}`;
            const selected = key === selectedKey;
            const share = familyShare(family, familyTop);
            const exactMembership = family.membership.state === "exact_native_digest" ? family.membership : null;
            const exact = exactMembership !== null;
            return (
              <tr
                className={selected ? "is-selected" : undefined}
                key={key}
                onClick={(event) => {
                  if (!disabled && exact) {
                    event.currentTarget.querySelector<HTMLButtonElement>(".row-pick-btn")?.focus();
                    onSelect(family);
                  }
                }}
              >
                <td className="audit-family-summary-cell">
                  <div className="audit-family-id">
                    <button
                      aria-haspopup={exact ? "dialog" : undefined}
                      aria-label={exactMembership ? `${text("Select query family", "选择查询族")} ${exactMembership.native_digest}` : undefined}
                      className="row-pick-btn mono"
                      disabled={disabled || !exact}
                      onClick={(event) => {
                        event.stopPropagation();
                        onSelect(family);
                      }}
                      type="button"
                    >
                      {exactMembership ? exactMembership.native_digest.slice(0, 8) : text("Not grouped", "未归组")}
                    </button>
                    {exact && <BadgeStamp kind="official_digest" />}
                    <span aria-hidden="true" className="share-track audit-family-share">
                      <span className="share-fill" style={{ width: share === null ? 0 : `${share / 100}%` }} />
                    </span>
                  </div>
                  <FamilyStatement family={family} id={`audit-family-sql-${key}`} text={text} />
                </td>
                <td className="num">{family.execution_count ?? <StateStamp state="missing" />}</td>
                {familyColumns.map((column) => {
                  const metric =
                    family.aggregates.find((item) => item.metric.metric_id === column.id && item.aggregation === column.aggregation)?.metric ?? null;
                  return (
                    <td className="num" key={`${column.id}-${column.aggregation}`} title={metricRawText(metric)}>
                      <AuditMetricValue text={text} value={metric} />
                    </td>
                  );
                })}
                <td className={`num last${family.error_count && family.error_count !== "0" ? " failed" : ""}`}>
                  {family.error_count ?? <StateStamp state="missing" />}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

const unavailableReasonLabels: Record<ProductSqlFingerprintUnavailableReason, readonly [string, string]> = {
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
export function AuditFingerprintFamilyList({
  response,
  error,
  loading,
  text,
  onShowExecutions,
}: {
  response: AuditFingerprintFamilyResponse | null;
  error: string | null;
  loading: boolean;
  text: LocalizedText;
  /** Switches the board to the window's individual executions. */
  onShowExecutions?(): void;
}): JSX.Element {
  const coverage = response?.coverage;
  const unofficial = response?.authority === "product_unofficial";
  const excluded = coverage?.unavailable
    .map((item) => `${lookupEnumLabel(unavailableReasonLabels, item.reason, text)} ${item.count}`)
    .join(" · ");

  return (
    <section aria-labelledby="audit-fingerprint-title" className="audit-fingerprint-families">
      <div className="audit-fingerprint-head">
        <h2 className="card-title" id="audit-fingerprint-title">
          {text("Executions without a native digest · grouped by template", "无原生 digest 的执行 · 按模板归组")}
        </h2>
        {unofficial && (
          <span
            className="badge-stamp badge-stamp--unofficial"
            title={`${response.fingerprint_version} · ${text("made by this tool from the audited statement; not a StarRocks digest", "由本工具根据审计语句生成，不是 StarRocks digest")}`}
          >
            {text("Unofficial fingerprint", "非官方指纹")}
          </span>
        )}
      </div>
      {error ? (
        <div className="notice-banner notice-banner--danger" role="alert">{error}</div>
      ) : loading && !response ? (
        <div className="state-box" role="status">{text("Grouping statements…", "正在按模板归组…")}</div>
      ) : (
        coverage && response && (
          <>
            <p className="audit-fingerprint-coverage">
              {coverage.source.state === "partial"
                ? text(
                    `Template grouping read the newest ${coverage.read_event_count} of ${coverage.eligible_event_count} candidate executions`,
                    `模板归组读取了 ${coverage.eligible_event_count} 次候选中最近的 ${coverage.read_event_count} 次执行`,
                  )
                : text(
                    `Template grouping read ${coverage.read_event_count} candidate executions`,
                    `本次模板归组读取了 ${coverage.read_event_count} 次候选执行`,
                  )}
              {" · "}
              {text(
                `${coverage.fingerprinted_event_count} grouped into ${coverage.family_count} templates`,
                `${coverage.fingerprinted_event_count} 次归入 ${coverage.family_count} 个模板`,
              )}
              {coverage.partial_event_count !== "0" && (
                <>
                  {" · "}
                  <span
                    className="state-stamp state-stamp--warn"
                    title={text(
                      "The statement is longer than the read prefix; it is neither grouped nor ranked",
                      "语句超出读取前缀，不参与归组与排序",
                    )}
                  >
                    {text(`truncated ${coverage.partial_event_count}`, `截断 ${coverage.partial_event_count}`)}
                  </span>
                </>
              )}
              {excluded && (
                <>
                  {" · "}
                  <span>{text("not grouped", "未归组")}: {excluded}</span>
                </>
              )}
            </p>
            {response.rows.length === 0 ? (
              <div className="state-box">
                <span>{text("No execution in this window can be grouped by template.", "当前窗口没有可按模板归组的执行。")}</span>
                {onShowExecutions && (
                  <Button onClick={onShowExecutions} size="sm" variant="secondary">
                    {text("See individual executions", "查看单次执行")}
                  </Button>
                )}
              </div>
            ) : (
              <div className="audit-family-list-wrap">
                <table aria-label={text("Unofficial fingerprint groups", "非官方指纹分组")} className="data-table audit-family-list">
                  <thead>
                    <tr>
                      <th scope="col">{text("Template (literals as ?)", "模板（字面量为 ?）")}</th>
                      <th className="num" scope="col">{text("Executions", "执行次数")}</th>
                      {familyColumns.map((column) => (
                        <th className="num" key={column.sort} scope="col">{familyColumnLabel(column.sort, text)}</th>
                      ))}
                      <th className="num last" scope="col">{text("Failed", "失败")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {response.rows.map((family) => (
                      <tr key={family.family_key}>
                        <td className="audit-family-summary-cell">
                          <ExpandableSqlLine
                            copyLabel={text("Copy template", "复制模板")}
                            id={`audit-fingerprint-template-${family.family_key}`}
                            label={text("Unofficial fingerprint template", "非官方指纹模板")}
                            lineClassName="audit-family-sql"
                            note={`${family.context.catalog ?? "—"} / ${family.context.database ?? "—"}`}
                            value={family.template}
                          />
                        </td>
                        <td className="num">{family.execution_count}</td>
                        {familyColumns.map((column) => {
                          const metric =
                            family.aggregates.find((item) => item.metric.metric_id === column.id && item.aggregation === column.aggregation)?.metric ?? null;
                          return (
                            <td className="num" key={column.sort} title={metricRawText(metric)}>
                              <AuditMetricValue text={text} value={metric} />
                            </td>
                          );
                        })}
                        <td className={`num last${family.error_count !== "0" ? " failed" : ""}`}>{family.error_count}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        )
      )}
    </section>
  );
}

function familyColumnLabel(sort: AuditFamilySort, text: LocalizedText): string {
  return sort === "cumulative_query_time_desc"
    ? text("Cumulative query time", "累计查询耗时")
    : sort === "max_query_time_desc"
      ? text("Maximum query time", "最大查询耗时")
      : text("Cumulative scanned bytes", "累计扫描数据量");
}

/** The plotted share comes only from the server's Top projection of the same family. */
function familyShare(family: AuditFamilyPresentation, familyTop: AuditOverviewFamilyTopPresentation | null): number | null {
  if (!familyTop || family.membership.state !== "exact_native_digest") return null;
  const key = family.membership.family_key;
  const row = familyTop.rows.find(
    (item) => item.family.membership.state === "exact_native_digest" && item.family.membership.family_key === key,
  );
  return row?.plot.state === "exact" ? row.plot.permyriad : null;
}

function FamilyStatement({ family, id, text }: { family: AuditFamilyPresentation; id: string; text: LocalizedText }): JSX.Element {
  const sample = family.statement_sample;
  if (sample.state === "text" && sample.value.trim() !== "") {
    return (
      <ExpandableSqlLine
        copyLabel={sample.truncated ? text("Copy returned prefix", "复制已返回片段") : text("Copy SQL", "复制 SQL")}
        id={id}
        label={text("SQL of the most recent member, as audited", "最近一次成员的审计原文")}
        lineClassName="audit-family-sql"
        note={text("Most recent member, as audited", "最近一次成员的审计原文")}
        truncated={sample.truncated}
        value={sample.value}
      />
    );
  }
  if (sample.state === "text") {
    return <div className="audit-family-sql muted">{text("audited stmt is empty", "审计 stmt 为空字符串")}</div>;
  }
  const context =
    family.membership.state === "exact_native_digest"
      ? `${family.membership.context.catalog ?? "—"} / ${family.membership.context.database ?? "—"}`
      : "—";
  const note =
    sample.state === "sql_null"
      ? text("audited stmt is NULL", "审计 stmt 为空")
      : sample.reason === "source_column_absent"
        ? text("no stmt column in the audit table", "审计表无 stmt 列")
        : context;
  return <div className="audit-family-sql muted">{note}</div>;
}

export type FamilyMemberState = "fetched" | "available" | "unavailable";
/** none: no baseline; selected: this member is it; comparable: it is in this list; elsewhere: set from another list. */
export type BaselineState = "none" | "selected" | "comparable" | "elsewhere";

export function memberKey(row: AuditOccurrencePresentation, index: number): string {
  return `${row.query.query_id}:${row.query.source_refs[0] ?? `member:${index}`}`;
}
