// Design board I2: load records StarRocks still holds, newest first. The
// server reads, filters and sums; this lays them out and sends filters back.
import type { JSX } from "react";
import type { LoadKindFilter, LoadOutcomeFilter, LoadRecord, LoadRecordsResponse } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { compact, count, feTime, hourRangeText, loadTone, ms, shareText } from "./load-text";

/** `hour` is one `YYYY-MM-DD HH` creation hour, set from the overview. */
export type RecordFilters = { kind: LoadKindFilter | null; outcome: LoadOutcomeFilter | null; table: string; include_system: boolean; hour: string | null };

export function LoadRecordsPanel({ records, filters, onFilters, onOpenLog, text }: {
  records: LoadRecordsResponse;
  filters: RecordFilters;
  onFilters(filters: RecordFilters): void;
  onOpenLog(record: LoadRecord): void;
  text: LocalizedText;
}): JSX.Element {
  if (records.support === "unsupported_version") {
    return <p className="history-banner">{text("Load records need StarRocks 3.3 or later (information_schema.loads with runtime details).", "导入记录需要 StarRocks 3.3 及以上（information_schema.loads 的 RUNTIME_DETAILS 等列）。")}</p>;
  }
  const summary = records.summary;
  const zone = records.fe_time_zone ? ` · ${records.fe_time_zone}` : "";
  const filteredShare = summary.filtered_rows !== "0" ? shareText(summary.filtered_share_permyriad) : null;
  const unknownRows = summary.filtered_rows_unknown !== "0" || summary.loaded_rows_unknown !== "0";
  return <div className="history-page load-page">
    <p className="history-banner" role="status">
      {text("StarRocks keeps only the last 3 days and at most about 1000 records per manager in FE memory; once Stream Load records pass 1000 it drops every finished one at once, so the count can fall to a handful.", "StarRocks 只在 FE 内存中保留最近 3 天、每类最多约 1000 条导入记录；Stream Load 记录超过 1000 条时会一次清掉所有已结束的任务，可见条数可能骤降到个位数。")}
      {records.earliest_create_time && <> {text("The oldest one still visible: ", "当前能看到的最早一条是 ")}<strong>{feTime(records.earliest_create_time)}</strong>{zone}{records.at_retention_limit ? text(" (at the limit; older ones are gone).", "（已到上限，更早的已不可查）。") : "。"}</>}
      {" "}<span className="history-muted">{text("The History tab keeps 30 days of hourly totals and saved error logs.", "“历史”标签保留 30 天的小时汇总与保存的错误日志。")}</span>
    </p>
    <div className="notify-kpis">
      <div className="history-card"><span className="history-muted">{text("All held loads (ignore the filters below)", "全部保留记录（不随下方筛选）")}</span><strong>{count(summary.loads)}</strong>{records.hidden_system !== "0" && <span className="history-muted">{text(`${records.hidden_system} system loads hidden`, `另有 ${records.hidden_system} 次系统任务已隐藏`)}</span>}</div>
      <div className={`history-card ${summary.failed !== "0" ? "load-card-bad" : ""}`}><span className="history-muted">{text("Failed (CANCELLED)", "失败（CANCELLED）")}</span><strong className={summary.failed !== "0" ? "bad" : ""}>{count(summary.failed)}</strong></div>
      <div className="history-card"><span className="history-muted">{text("Filtered / loaded rows", "被过滤行 / 导入行")}</span><strong>{compact(summary.filtered_rows)}</strong><span className="history-muted">/ {compact(summary.loaded_rows)}{filteredShare ? ` · ${filteredShare}` : ""}</span>{unknownRows && <span className="history-muted">{text(`${summary.filtered_rows_unknown} filtered / ${summary.loaded_rows_unknown} loaded counts unknown, left out`, `过滤行 ${summary.filtered_rows_unknown} 条、导入行 ${summary.loaded_rows_unknown} 条未知，未计入`)}</span>}</div>
      <div className="history-card"><span className="history-muted">{text("Begin txn + plan, P95", "取事务 + 规划耗时 P95")}</span><strong>{summary.begin_plan_p95_ms !== null ? ms(summary.begin_plan_p95_ms) : "—"}</strong>{summary.begin_plan_p95_ms === null && <span className="history-muted">{text("fewer than 20 samples", "样本不足 20 条")}</span>}</div>
    </div>
    <div className="history-row">
      <section aria-labelledby="load-records" className="history-card wide">
        <div className="history-card-head">
          <h2 id="load-records" tabIndex={-1}>{text("Load records", "导入记录")} <span className="history-muted">{text(`${records.matched} matched`, `匹配 ${records.matched} 条`)}</span></h2>
          <label className="notify-filter">{text("Type", "类型")}
            <select onChange={(event) => onFilters({ ...filters, kind: (event.target.value || null) as LoadKindFilter | null })} value={filters.kind ?? ""}>
              <option value="">{text("All", "全部")}</option><option value="stream_load">STREAM_LOAD</option><option value="routine_load">ROUTINE_LOAD</option><option value="broker">BROKER</option><option value="insert">INSERT</option>
            </select>
          </label>
          <label className="notify-filter">{text("Result", "结果")}
            <select onChange={(event) => onFilters({ ...filters, outcome: (event.target.value || null) as LoadOutcomeFilter | null })} value={filters.outcome ?? ""}>
              <option value="">{text("All", "全部")}</option><option value="failed">{text("Failed", "失败")}</option><option value="filtered">{text("Has filtered rows", "有过滤行")}</option><option value="running">{text("In progress", "进行中")}</option>
            </select>
          </label>
          <label className="notify-filter">{text("Table", "库表")}
            <input onChange={(event) => onFilters({ ...filters, table: event.target.value })} placeholder="ods.orders" value={filters.table} />
          </label>
          <label className="notify-check"><input checked={filters.include_system} onChange={(event) => onFilters({ ...filters, include_system: event.target.checked })} type="checkbox" /> {text("Show system loads (statistics, audit log)", "显示系统任务（统计信息收集、审计日志）")}</label>
          {filters.table.trim() && <span className="load-filter-chip">{text("Table contains", "库表包含")} <span className="mono">{filters.table.trim()}</span><button aria-label={text("Clear the table filter", "清除库表筛选")} onClick={() => onFilters({ ...filters, table: "" })} type="button">×</button></span>}
          {filters.hour && <span className="load-filter-chip">{text("Created", "创建于")} {hourRangeText(filters.hour)}{records.fe_time_zone ? ` · ${records.fe_time_zone}` : ""}<button aria-label={text("Clear the hour filter", "清除小时筛选")} onClick={() => onFilters({ ...filters, hour: null })} type="button">×</button></span>}
        </div>
        {records.records.length === 0 ? <p className="notify-empty">{text("No load record matches.", "没有匹配的导入记录。")}</p> : <div className="history-table-wrap">
          <table className="history-table load-table">
            <thead><tr>
              <th>{text(`Created${zone}`, `开始${zone}`)}</th><th>{text("Label · table", "Label · 库表")}</th><th>{text("Type · source", "类型 · 来源")}</th><th>{text("Result", "结果")}</th>
              <th className="num">{text("Loaded / filtered", "导入 / 过滤")}</th><th className="num">{text("Begin · plan · receive", "取事务 · 规划 · 接收")}</th><th className="num">{text("Total", "总耗时")}</th><th className="num">{text("Actions", "操作")}</th>
            </tr></thead>
            <tbody>
              {records.records.map((record) => {
                const share = record.filtered_rows !== "0" ? shareText(record.filtered_share_permyriad) : null;
                return <tr key={record.job_id}>
                  <td className="mono">{feTime(record.create_time)}</td>
                  <td><span className="mono load-label" title={record.label}>{record.label}</span><span className="history-muted load-sub">{record.db}.{record.table}{record.system ? text(" · system", " · 系统任务") : ""}</span></td>
                  <td>{record.kind}<span className="history-muted load-sub">{[record.user, record.client_ip].filter(Boolean).join(" · ") || "—"}</span></td>
                  <td><span className={`notify-chip ${loadTone(record.state_class)}`}>{record.state}</span>{record.error_message && <span className="load-error-text">{record.error_message}</span>}</td>
                  <td className="num">{count(record.loaded_rows)}<span className={`load-sub ${share ? "warn" : "history-muted"}`}>{record.filtered_rows === null ? "—" : text(`filtered ${count(record.filtered_rows)}${share ? ` (${share})` : ""}`, `过滤 ${count(record.filtered_rows)}${share ? `（${share}）` : ""}`)}</span></td>
                  <td className="num mono">{[record.begin_txn_ms, record.plan_ms, record.receive_data_ms].map(ms).join(" · ")}</td>
                  <td className="num mono">{record.total_seconds !== null ? `${record.total_seconds} s` : record.state_class === "running" ? text("running", "进行中") : "—"}</td>
                  <td className="num">
                    {record.has_error_log && <button aria-haspopup="dialog" className="button text" onClick={() => onOpenLog(record)} type="button">{text("Error log", "错误日志")}</button>}
                    {record.profile_id && <span className="planned" title={record.profile_id}>Profile · Planned</span>}
                  </td>
                </tr>;
              })}
            </tbody>
          </table>
        </div>}
        <ul className="notify-notes">
          <li>{text("Begin-transaction, plan and receive-data times come from RUNTIME_DETAILS (milliseconds). StarRocks does not record write and commit/publish times; the total is finish minus create, whole seconds only.", "取事务、规划、接收数据三段来自 RUNTIME_DETAILS（毫秒）；写数据、提交发布两段 StarRocks 不记录，总耗时由开始与完成时间相减，只精确到秒。")}</li>
          <li>{text("Byte counts are not shown: StarRocks counts them differently from the LoadBytes a Stream Load returns. Load Profile diagnosis is planned (L3).", "字节数口径与 Stream Load 返回的 LoadBytes 不同，这里不显示；导入 Profile 诊断为 Planned（L3）。")}</li>
          <li>{text("When a retry reuses a label and fails, StarRocks keeps only the retry's CANCELLED record; the original success is no longer visible.", "用同一 label 重试失败时，StarRocks 只保留重试那条 CANCELLED 记录，最初的成功记录会看不到。")}</li>
        </ul>
      </section>
      <section aria-labelledby="load-tables" className="history-card load-side">
        <h2 id="load-tables">{text("By table", "按表汇总")} <span className="history-muted">{text("all held records, ignores the filters", "全部保留记录，不随筛选")}</span></h2>
        {records.tables.length === 0 ? <p className="history-muted">—</p> : <table className="history-table">
          <thead><tr><th>{text("Table", "表")}</th><th className="num">{text("Loads", "次数")}</th><th className="num">{text("Failed", "失败")}</th><th className="num">{text("Filtered rows", "过滤行")}</th></tr></thead>
          <tbody>
            {records.tables.map((row) => <tr key={row.table}>
              <td><button className="load-link" onClick={() => onFilters({ ...filters, table: row.table })} type="button">{row.table}</button></td>
              <td className="num">{count(row.loads)}</td><td className={`num ${row.failed !== "0" ? "bad" : ""}`}>{count(row.failed)}</td><td className="num">{count(row.filtered_rows)}{row.filtered_unknown !== "0" && <span className="history-muted load-sub" title={text("records with an unknown filtered count", "过滤行未知的记录数")}>{text(`+${row.filtered_unknown} unknown`, `另 ${row.filtered_unknown} 条未知`)}</span>}</td>
            </tr>)}
          </tbody>
        </table>}
        <p className="history-muted">{text("Click a table to filter by it. Failure and filter-rate trends across days are on the History tab.", "点表名把它填进筛选。跨天的失败率与过滤率趋势见“历史”标签。")}</p>
        <span className="planned">{text("Load alerts: Alerts › Loads", "导入告警见“告警 › 导入”")}</span>
      </section>
    </div>
  </div>;
}
