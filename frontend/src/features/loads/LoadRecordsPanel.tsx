// Design board 07-loads: live load records table card (col-12) with inline
// `.filter-chip-input` controls, `BadgeStamp kind="planned"` on Profile,
// `StateStamp state="missing"` on unknown counts, and `.card-ft` receipt.
import { useId } from "react";
import type { JSX } from "react";
import type {
  LoadKindFilter,
  LoadOutcomeFilter,
  LoadRecord,
  LoadRecordsResponse,
} from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Button, Card, InfoTip, PageGrid, StateStamp } from "../../design";
import type { LocalizedText } from "../../i18n/locale";
import {
  count,
  feTime,
  hourRangeText,
  loadDotClass,
  loadKindLabel,
  loadStateLabel,
  ms,
  shareText,
} from "./load-text";

/** `hour` is one `YYYY-MM-DD HH` creation hour, set from the overview. */
export type RecordFilters = {
  kind: LoadKindFilter | null;
  outcome: LoadOutcomeFilter | null;
  table: string;
  include_system: boolean;
  hour: string | null;
};

export function LoadRecordsPanel({
  records,
  filters,
  onFilters,
  onOpenLog,
  hideSharedFilters = false,
  text,
}: {
  records: LoadRecordsResponse;
  filters: RecordFilters;
  onFilters(filters: RecordFilters): void;
  onOpenLog(record: LoadRecord): void;
  /** True when mounted inside LoadWorkspace (type & system toggle live in `.workspace-bar`). */
  hideSharedFilters?: boolean;
  text: LocalizedText;
}): JSX.Element {
  const kindId = useId();
  const outId = useId();
  const tblId = useId();

  if (records.support === "unsupported_version") {
    return (
      <PageGrid>
        <Card
          span={{ xl: 12, lg: 12, md: 8, sm: 4 }}
          title={text("Live load records", "实时导入记录")}
          variant="detail"
        >
          <div className="state-box">
            <strong>
              {text(
                "Load records need StarRocks 3.3 or later.",
                "导入记录需要 StarRocks 3.3 及以上。",
              )}
            </strong>
          </div>
        </Card>
      </PageGrid>
    );
  }

  const summary = records.summary;
  const zone = records.fe_time_zone ? ` · ${records.fe_time_zone}` : "";
  const filteredShare = summary.filtered_rows !== "0" ? shareText(summary.filtered_share_permyriad) : null;
  const unknownRows = summary.filtered_rows_unknown !== "0" || summary.loaded_rows_unknown !== "0";

  return (
    <PageGrid>
      <Card
        actions={
          <>
            {filters.table.trim() ? (
              <span className="load-filter-chip">
                {text("Table contains", "库表包含")}{" "}
                <span className="mono">{filters.table.trim()}</span>
                <button
                  aria-label={text("Clear the table filter", "清除库表筛选")}
                  onClick={() => onFilters({ ...filters, table: "" })}
                  type="button"
                >
                  ×
                </button>
              </span>
            ) : null}
            {filters.hour ? (
              <span className="load-filter-chip">
                {text("Created", "创建于")} {hourRangeText(filters.hour)}
                {records.fe_time_zone ? ` · ${records.fe_time_zone}` : ""}
                <button
                  aria-label={text("Clear the hour filter", "清除小时筛选")}
                  onClick={() => onFilters({ ...filters, hour: null })}
                  type="button"
                >
                  ×
                </button>
              </span>
            ) : null}
            {!hideSharedFilters ? (
              <div className="filter-chip-input">
                <label htmlFor={kindId}>{text("Type", "类型")}</label>
                <select
                  id={kindId}
                  onChange={(event) =>
                    onFilters({
                      ...filters,
                      kind: (event.target.value || null) as LoadKindFilter | null,
                    })
                  }
                  value={filters.kind ?? ""}
                >
                  <option value="">{text("All", "全部")}</option>
                  <option value="stream_load">Stream Load</option>
                  <option value="routine_load">Routine Load</option>
                  <option value="broker">Broker Load</option>
                  <option value="insert">INSERT</option>
                </select>
              </div>
            ) : null}
            <div className="filter-chip-input">
              <label htmlFor={outId}>{text("Outcome", "结果")}</label>
              <select
                id={outId}
                onChange={(event) =>
                  onFilters({
                    ...filters,
                    outcome: (event.target.value || null) as LoadOutcomeFilter | null,
                  })
                }
                value={filters.outcome ?? ""}
              >
                <option value="">{text("All", "全部")}</option>
                <option value="failed">{text("Failed / cancelled", "失败 / 取消")}</option>
                <option value="filtered">{text("Has filtered rows", "含过滤行")}</option>
                <option value="running">{text("In progress", "进行中")}</option>
              </select>
            </div>
            <div className="filter-chip-input">
              <label htmlFor={tblId}>{text("Table", "库.表")}</label>
              <input
                id={tblId}
                onChange={(event) => onFilters({ ...filters, table: event.target.value })}
                placeholder={text("e.g. ods.orders", "如 ods.orders")}
                size={12}
                type="text"
                value={filters.table}
              />
            </div>
            {!hideSharedFilters ? (
              <label className="check-inline">
                <input
                  checked={filters.include_system}
                  onChange={(event) =>
                    onFilters({ ...filters, include_system: event.target.checked })
                  }
                  type="checkbox"
                />
                <span>{text("Include system tasks", "含系统任务")}</span>
              </label>
            ) : null}
          </>
        }
        footer={
          <>
            <span>
              {text(`${count(records.matched)} matched`, `匹配 ${count(records.matched)} 条`)}
              {" · "}
              {summary.begin_plan_p95_ms !== null
                ? text(
                    `Begin+plan P95 ${ms(summary.begin_plan_p95_ms)}`,
                    `取事务+规划 P95 ${ms(summary.begin_plan_p95_ms)}`,
                  )
                : text("Begin+plan P95 < 20 samples", "取事务+规划 P95 样本不足 20 条")}
              {filteredShare ? ` · ${text(`Filtered share ${filteredShare}`, `过滤率 ${filteredShare}`)}` : ""}
              {records.earliest_create_time ? (
                <>
                  {" · "}
                  {text("Oldest", "最早")} <strong>{feTime(records.earliest_create_time)}</strong>
                  {zone}
                  {records.at_retention_limit
                    ? text(" (at retention limit)", "（已到上限，更早的已不可查）")
                    : ""}
                </>
              ) : null}
              {records.hidden_system !== "0"
                ? ` · ${text(`${records.hidden_system} system loads hidden`, `另有 ${records.hidden_system} 次系统任务已隐藏`)}`
                : ""}
            </span>
            {unknownRows ? (
              <span className="load-warn-text">
                {text(
                  `${summary.filtered_rows_unknown} filtered / ${summary.loaded_rows_unknown} loaded counts unknown, left out`,
                  `过滤行 ${summary.filtered_rows_unknown} 条、导入行 ${summary.loaded_rows_unknown} 条未知，未计入`,
                )}
              </span>
            ) : null}
          </>
        }
        id="load-records"
        span={{ xl: 12, lg: 12, md: 8, sm: 4 }}
        title={text("Live load records", "实时导入记录")}
        titleExtra={
          <InfoTip label={text("About record retention and timing", "导入记录保留期与耗时口径")}>
            {text(
              "FE keeps the last 3 days and ~1,000 records per manager in memory (Stream Load clears finished records at once past 1,000). Millisecond phases apply to Stream Load; total time is finish minus create in whole seconds. When a retry reuses a label and fails, StarRocks keeps only the retry's cancelled record.",
              "FE 内存默认保留近 3 天、每类约 1,000 条记录（Stream Load 超过 1,000 条会一次清空已结束记录）。毫秒级取事务/规划/接收仅适用 Stream Load；总耗时 = 完成 − 创建（秒级）。同一 Label 重试失败时仅保留重试那条记录。",
            )}
          </InfoTip>
        }
        variant="table"
      >
        {records.records.length === 0 ? (
          <div className="state-box">
            <strong>{text("No load record matches.", "没有匹配的导入记录。")}</strong>
          </div>
        ) : (
          <table aria-label={text("Live load records", "实时导入记录表")} className="data-table">
            <thead>
              <tr>
                <th scope="col">{text("Label / Job ID", "Label / Job ID")}</th>
                <th scope="col">{text("Type / source", "类型 / 来源")}</th>
                <th scope="col">{text("Target table", "目标表")}</th>
                <th scope="col">{text("State", "状态")}</th>
                <th className="num" scope="col">{text("Rows written", "写入行")}</th>
                <th className="num" scope="col">{text("Filtered rows", "过滤行")}</th>
                <th className="num" scope="col">{text("Duration & phases", "耗时与毫秒阶段")}</th>
                <th scope="col">{text("Diagnosis & error log", "诊断与错误日志")}</th>
              </tr>
            </thead>
            <tbody>
              {records.records.map((record) => {
                const share =
                  record.filtered_rows !== null && record.filtered_rows !== "0"
                    ? shareText(record.filtered_share_permyriad)
                    : null;
                const hasPhases =
                  record.begin_txn_ms !== null ||
                  record.plan_ms !== null ||
                  record.receive_data_ms !== null;
                const execPublish =
                  record.execute_seconds !== null && record.publish_seconds !== null
                    ? ` (${record.execute_seconds} + ${record.publish_seconds})`
                    : "";
                return (
                  <tr key={record.job_id}>
                    <td>
                      <strong className="mono load-label" title={record.label}>
                        {record.label}
                      </strong>
                      <div className="card-subtitle mono">
                        #{record.job_id} · {feTime(record.create_time)}
                        {record.system ? text(" · system", " · 系统任务") : ""}
                      </div>
                    </td>
                    <td>
                      <span>{loadKindLabel(record.kind, text)}</span>
                      <div className="card-subtitle mono">
                        {[record.user, record.client_ip].filter(Boolean).join(" · ") || "—"}
                      </div>
                    </td>
                    <td>
                      <button
                        className="row-pick-btn mono"
                        onClick={() =>
                          onFilters({ ...filters, table: `${record.db}.${record.table}` })
                        }
                        title={text("Filter by this table", "按此表筛选")}
                        type="button"
                      >
                        {record.db}.{record.table}
                      </button>
                    </td>
                    <td>
                      <span className="status-inline">
                        <span aria-hidden="true" className={loadDotClass(record.state_class)} />
                        <span>{loadStateLabel(record.state_class, text)}</span>
                      </span>
                      {record.error_message ? (
                        <span className="load-error-text">{record.error_message}</span>
                      ) : null}
                    </td>
                    <td className="num">
                      {record.loaded_rows === null ? (
                        <StateStamp state="missing" />
                      ) : (
                        count(record.loaded_rows)
                      )}
                    </td>
                    <td className="num">
                      {record.filtered_rows === null ? (
                        <StateStamp state="missing" />
                      ) : (
                        <>
                          <span>{count(record.filtered_rows)}</span>
                          {share ? (
                            <span className="load-sub load-warn-text">
                              {text(`filtered (${share})`, `过滤 ${count(record.filtered_rows)}（${share}）`)}
                            </span>
                          ) : null}
                        </>
                      )}
                    </td>
                    <td className="num mono">
                      <span>
                        {record.total_seconds !== null
                          ? `${record.total_seconds} s${execPublish}`
                          : record.state_class === "running"
                            ? text("In progress", "进行中")
                            : "—"}
                      </span>
                      <span className="load-sub card-subtitle mono">
                        {hasPhases
                          ? [record.begin_txn_ms, record.plan_ms, record.receive_data_ms]
                              .map(ms)
                              .join(" · ")
                          : "— · — · —"}
                      </span>
                    </td>
                    <td>
                      <div className="load-record-actions">
                        {record.has_error_log ? (
                          <Button
                            aria-haspopup="dialog"
                            onClick={() => onOpenLog(record)}
                            size="sm"
                            variant="secondary"
                          >
                            {text("Error log", "错误日志")}
                          </Button>
                        ) : null}
                        {record.profile_id ? (
                          <span className="load-profile-planned" title={record.profile_id}>
                            <span className="mono">Profile</span> <BadgeStamp kind="planned" />
                          </span>
                        ) : null}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </Card>
    </PageGrid>
  );
}
