// Design board 07-loads (decided item 2): Routine Load merged into Overview.
// Shows Routine Load KPIs + side-by-side job list card (col-7) and selected
// job's partition offset details card (col-5). Read-only: no pause, resume or mutate.
import { useId, useState } from "react";
import type { JSX } from "react";
import type {
  LoadOverviewResponse,
  RoutineLoadJob,
  RoutineLoadJobsResponse,
  RoutineLoadStateClass,
} from "../../../../contracts/generated/doctor-contracts";
import { AnimatedFigure, Button, Card, InfoTip, KpiCard, PageGrid, StateStamp } from "../../design";
import type { LocalizedText } from "../../i18n/locale";
import {
  ago,
  bytes,
  compact,
  count,
  feTime,
  ms,
  routineDotClass,
  routineStateLabel,
} from "./load-text";

const BIG_LAG = 10_000n;

function isBig(lag: string | null): boolean {
  return lag !== null && /^\d+$/.test(lag) && BigInt(lag) > BIG_LAG;
}

function noteOf(job: RoutineLoadJob, text: LocalizedText): string {
  if (job.state_class === "paused" || job.state_class === "cancelled" || job.state_class === "stopped") {
    return job.reason ?? job.other_message ?? "—";
  }
  if (job.max_lag_partition !== null && isBig(job.max_lag)) {
    return text(
      `Lag concentrated in partition ${job.max_lag_partition}`,
      `积压集中在分区 ${job.max_lag_partition}`,
    );
  }
  return job.other_message ?? "—";
}

const STATE_FILTER_OPTIONS: RoutineLoadStateClass[] = [
  "running",
  "scheduling",
  "paused",
  "stopped",
  "cancelled",
];

export function RoutineLoadPanel({
  jobs,
  overview = null,
  initialJob,
  nowMs,
  onOpenLog,
  onOpenAlerts,
  text,
}: {
  jobs: RoutineLoadJobsResponse;
  /** Optional overview payload when rendered inside Overview (type = Routine Load). */
  overview?: LoadOverviewResponse | null;
  /** The job opened from an alert or drill-down, selected first. */
  initialJob?: string | null;
  /** Browser time, only to say how long ago a message was consumed. */
  nowMs: number;
  onOpenLog(job: RoutineLoadJob): void;
  onOpenAlerts?(): void;
  text: LocalizedText;
}): JSX.Element {
  const [picked, setPicked] = useState<string | null>(initialJob ?? null);
  const [stateFilter, setStateFilter] = useState<string>("all");
  const filterId = useId();

  if (jobs.support === "unsupported_version") {
    return (
      <PageGrid>
        <Card
          span={{ xl: 12, lg: 12, md: 8, sm: 4 }}
          title={text("Routine Load jobs", "Routine Load 作业")}
          variant="detail"
        >
          <div className="state-box">
            <strong>
              {text(
                "Routine Load jobs need StarRocks 3.2 or later.",
                "Routine Load 作业需要 StarRocks 3.2 及以上。",
              )}
            </strong>
          </div>
        </Card>
      </PageGrid>
    );
  }

  const visible = jobs.jobs.filter((job) => stateFilter === "all" || job.state_class === stateFilter);
  const selected = visible.find((job) => job.job_id === picked) ?? visible[0] ?? null;
  const summary = jobs.summary;
  const worst = summary.lags[0] ?? null;
  const unknownErrorRows = summary.error_rows_unknown !== "0";
  const allErrorRowsUnknown = unknownErrorRows && summary.error_rows === "0";
  const detail = overview?.routine_detail ?? null;
  const tasksSupported = overview ? overview.support !== "unsupported_version" : false;

  return (
    <div className="load-view-stack">
      <section aria-label={text("Routine Load summary", "Routine Load 汇总")} className="kpi-grid kpi-grid--5">
        <KpiCard
          foot={
            summary.scheduling !== "0"
              ? <span className="kpi-foot--warn">{text(`${summary.scheduling} waiting to schedule`, `另有 ${summary.scheduling} 个等待调度`)}</span>
              : undefined
          }
          label={text("Running", "运行中")}
          value={
            <>
              <AnimatedFigure value={count(summary.running)} />
              <span className="kpi-unit">{text("jobs", "个")}</span>
            </>
          }
        />

        <KpiCard
          foot={
            summary.scheduling !== "0"
              ? text(`Waiting to schedule ${summary.scheduling}`, `等待调度 ${summary.scheduling}`)
              : undefined
          }
          label={text("Paused / ended", "已暂停 / 已结束")}
          tone={summary.paused !== "0" ? "critical" : "default"}
          value={
            <>
              <AnimatedFigure value={`${summary.paused} / ${summary.ended}`} />
            </>
          }
        />

        <KpiCard
          foot={
            jobs.support === "without_lag"
              ? text("StarRocks 3.2 has no partition lag", "StarRocks 3.2 没有分区积压位点")
              : worst
                ? `${worst.db}.${worst.table} · p${worst.partition} · ${routineStateLabel(worst.state_class, text)}`
                : undefined
          }
          label={text("Largest partition lag", "最大分区积压")}
          tone={worst?.state_class === "paused" ? "critical" : "default"}
          value={
            jobs.support === "without_lag" ? (
              <StateStamp detail={text("Unavailable on StarRocks 3.2", "3.2 版本不可用")} state="missing" />
            ) : worst ? (
              <>
                <AnimatedFigure value={count(worst.lag)} />
                <span className="kpi-unit">{text("msgs", "条")}</span>
              </>
            ) : (
              "—"
            )
          }
        />

        <KpiCard
          foot={
            unknownErrorRows ? (
              <span className="kpi-foot--warn">
                {text(
                  `${summary.error_rows_unknown} jobs unreported, left out`,
                  `${summary.error_rows_unknown} 个作业未报告，未计入`,
                )}
              </span>
            ) : undefined
          }
          info={text(
            "Lifetime error rows summed across visible jobs; jobs without reported counters stay excluded and are never counted as 0.",
            "可见作业创建以来的错误行累计；未报告计数的作业不计入，绝不当作 0。",
          )}
          label={text("Lifetime error rows", "累计错误行")}
          value={
            allErrorRowsUnknown ? (
              <StateStamp state="missing" />
            ) : (
              <AnimatedFigure value={count(summary.error_rows)} />
            )
          }
        />

        <KpiCard
          foot={
            detail && detail.average_task_ms !== null
              ? `${text("Avg task", "平均任务执行")} ${ms(detail.average_task_ms)}`
              : detail && tasksSupported && overview?.earliest_create_time
                ? `${text("Oldest held", "最早保留")} ${feTime(overview.earliest_create_time)}`
                : undefined
          }
          label={detail ? text("Recent tasks", "近期提交任务") : text("Visible jobs", "可见作业总数")}
          value={
            detail ? (
              tasksSupported ? (
                <>
                  <AnimatedFigure value={count(detail.held_tasks)} />
                  <span className="kpi-unit">{text("tasks", "批")}</span>
                </>
              ) : (
                <StateStamp detail={text("Task records need StarRocks 3.3+", "任务记录需要 StarRocks 3.3 及以上")} state="missing" />
              )
            ) : (
              <>
                <AnimatedFigure value={count(summary.jobs)} />
                <span className="kpi-unit">{text("jobs", "个")}</span>
              </>
            )
          }
        />
      </section>

      <PageGrid>
        <Card
          actions={
            <div className="filter-chip-input">
              <label htmlFor={filterId}>{text("State", "状态")}</label>
              <select
                id={filterId}
                onChange={(event) => setStateFilter(event.target.value)}
                value={stateFilter}
              >
                <option value="all">{text("All", "全部")}</option>
                {STATE_FILTER_OPTIONS.map((item) => (
                  <option key={item} value={item}>
                    {routineStateLabel(item, text)}
                  </option>
                ))}
              </select>
            </div>
          }
          footer={
            <>
              <span>
                {selected && selected.reason
                  ? text(
                      `Selected ${selected.name}: ${selected.reason}`,
                      `已选 ${selected.name}：${selected.reason}`,
                    )
                  : text(
                      "SR Doctor is read-only: it does not pause, resume or modify jobs.",
                      "SR Doctor 只读，不提供暂停、恢复或修改作业；需要时请在 StarRocks 执行 RESUME ROUTINE LOAD。",
                    )}
              </span>
              {onOpenAlerts ? (
                <Button onClick={onOpenAlerts} size="sm" variant="ghost">
                  {text("Load alerts →", "导入告警 →")}
                </Button>
              ) : (
                <span className="card-subtitle">
                  {text("Alerts: Alerts › Loads", "告警见“告警 › 导入”")}
                </span>
              )}
            </>
          }
          span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
          title={text("Routine Load jobs", "Routine Load 作业与状态")}
          titleExtra={
            <>
              <span className="badge-stamp">{text("Read-only", "只读会话")}</span>
              <InfoTip label={text("About Routine Load retention and lag", "Routine Load 保留期与积压说明")}>
                {text(
                  "StarRocks retains stopped or cancelled jobs for 3 days. Partition lag equals latest offset minus consumed offset (3.3+). Lifetime throughput is total rows divided by cumulative task execution time, not instantaneous rate.",
                  "已停止或已取消的作业 StarRocks 默认保留 3 天。未消费条数 = 最新位点 − 已消费位点（3.3 及以上支持）。导入速率为作业生命周期累计导入行 ÷ 累计执行耗时，并非瞬时速率。",
                )}
              </InfoTip>
            </>
          }
          variant="table"
        >
          {visible.length === 0 ? (
            <div className="state-box">
              <strong>
                {jobs.jobs.length === 0
                  ? text("No Routine Load job is visible to this account.", "当前账号看不到任何 Routine Load 作业。")
                  : text("No job in this state.", "该状态下没有作业。")}
              </strong>
            </div>
          ) : (
            <table aria-label={text("Routine Load jobs", "Routine Load 作业列表")} className="data-table">
              <thead>
                <tr>
                  <th scope="col">{text("Job / table", "作业名 / 库.表")}</th>
                  <th scope="col">{text("State", "状态")}</th>
                  <th className="num" scope="col">{text("Tasks", "任务数")}</th>
                  <th className="num" scope="col">{text("Max unconsumed", "最大分区积压")}</th>
                  <th className="num" scope="col">{text("Rows / s", "导入速率")}</th>
                  <th className="num" scope="col">{text("Error / total", "错误行 / 总行")}</th>
                  <th scope="col">{text("Notes / log", "说明与日志")}</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((job) => {
                  const isSelected = job.job_id === selected?.job_id;
                  return (
                    <tr className={isSelected ? "is-selected" : undefined} key={job.job_id}>
                      <td>
                        <button
                          aria-pressed={isSelected}
                          className="row-pick-btn load-row-pick"
                          onClick={() => setPicked(job.job_id)}
                          type="button"
                        >
                          <strong className="mono">{job.name}</strong>
                          <span className="card-subtitle mono">
                            {job.db}.{job.table} · {job.source_type}
                            {job.topic ? ` ${job.topic}` : ""}
                          </span>
                        </button>
                      </td>
                      <td>
                        <span className="status-inline">
                          <span aria-hidden="true" className={routineDotClass(job.state_class)} />
                          <span>{routineStateLabel(job.state_class, text)}</span>
                        </span>
                      </td>
                      <td className="num">
                        {count(job.current_tasks)} / {count(job.desired_tasks)}
                      </td>
                      <td className={`num${isBig(job.max_lag) ? " load-num-critical" : ""}`}>
                        {jobs.support === "without_lag" ? (
                          <StateStamp state="missing" />
                        ) : job.max_lag !== null ? (
                          <>
                            <strong>{count(job.max_lag)}</strong>
                            {job.max_lag_partition !== null ? ` (p${job.max_lag_partition})` : ""}
                          </>
                        ) : (
                          "—"
                        )}
                      </td>
                      <td className="num">
                        {job.state_class === "running" && job.statistics.load_rows_rate !== null
                          ? compact(job.statistics.load_rows_rate)
                          : "—"}
                      </td>
                      <td className="num">
                        {compact(job.statistics.error_rows)} / {compact(job.statistics.total_rows)}
                      </td>
                      <td>
                        <div className="load-job-note-cell">
                          <span className="load-note">{noteOf(job, text)}</span>
                          {job.has_error_log ? (
                            <Button
                              aria-haspopup="dialog"
                              onClick={() => onOpenLog(job)}
                              size="sm"
                              variant="secondary"
                            >
                              {text("Error log", "错误日志")}
                            </Button>
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

        <Card
          actions={
            selected ? (
              <span className="status-inline">
                <span aria-hidden="true" className={routineDotClass(selected.state_class)} />
                <span>{routineStateLabel(selected.state_class, text)}</span>
              </span>
            ) : undefined
          }
          footer={
            selected ? (
              <>
                <Button
                  aria-haspopup="dialog"
                  disabled={!selected.has_error_log}
                  onClick={() => onOpenLog(selected)}
                  size="sm"
                  variant="secondary"
                >
                  {selected.has_error_log
                    ? text("View error log", "查看错误日志")
                    : jobs.support === "without_lag"
                      ? text("Error logs need 3.3+", "错误日志需要 3.3 及以上")
                      : text("No error log", "没有错误日志")}
                </Button>
                <span className="card-subtitle">
                  {text("SR Doctor is read-only: no pause or resume", "SR Doctor 只读，不提供暂停、恢复或修改作业")}
                </span>
              </>
            ) : undefined
          }
          id="routine-detail"
          span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
          title={
            selected
              ? `${selected.name} · ${text("Partition offsets", "分区位点明细")}`
              : text("Partition offsets", "分区位点明细")
          }
          variant="detail"
        >
          {!selected ? (
            <div className="state-box">
              <span>{text("Select a Routine Load job to inspect its partition offsets.", "选择左侧作业查看分区位点与统计明细。")}</span>
            </div>
          ) : (
            <div className="detail-stack">
              {selected.reason ? (
                <div className="notice-banner notice-banner--warn">
                  <span>
                    <strong>
                      {text("State change reason", "状态变化原因")}
                      {selected.pause_time ? ` · ${feTime(selected.pause_time)}` : ""}：
                    </strong>
                    <span className="mono">{selected.reason}</span>
                  </span>
                </div>
              ) : null}

              <dl className="kv-grid">
                <dt>{text("Target & source", "目标表与数据源")}</dt>
                <dd className="mono">
                  {selected.db}.{selected.table} · {selected.source_type}
                  {selected.topic ? ` · ${selected.topic}` : ""} ·{" "}
                  {text(`${selected.partitions.length} partitions`, `${selected.partitions.length} 个分区`)}
                </dd>
                <dt>{text("Committed / aborted", "已提交 / 已中止任务")}</dt>
                <dd className="mono">
                  {count(selected.statistics.committed_tasks)} / {count(selected.statistics.aborted_tasks)}
                </dd>
                <dt>{text("Received / rate", "累计接收 / 速率")}</dt>
                <dd className="mono">
                  {bytes(selected.statistics.received_bytes)} · {bytes(selected.statistics.received_bytes_rate)}/s
                </dd>
                <dt>{text("Loaded / error / unselected", "导入 / 错误 / 未选中行")}</dt>
                <dd className="mono">
                  {count(selected.statistics.loaded_rows)} / {count(selected.statistics.error_rows)} /{" "}
                  {count(selected.statistics.unselected_rows)}
                </dd>
                <dt>{text("Task time / tolerance", "累计执行耗时 / 容错")}</dt>
                <dd className="mono">
                  {ms(selected.statistics.task_execute_time_ms)} ·{" "}
                  {selected.max_error_number !== null ? `max_error_number=${selected.max_error_number}` : "—"}
                </dd>
              </dl>

              <h3 className="detail-sec-title">{text("Partition progress", "分区位点与未消费积压")}</h3>
              {selected.partitions.length === 0 ? (
                <div className="state-box">
                  <span>{text("No partition progress reported.", "StarRocks 没有报告分区位点进度。")}</span>
                </div>
              ) : (
                <table aria-label={text("Partition offsets", "分区位点明细表")} className="data-table">
                  <thead>
                    <tr>
                      <th scope="col">{text("Partition", "分区")}</th>
                      <th className="num" scope="col">{text("Consumed", "已消费位点")}</th>
                      <th className="num" scope="col">{text("Latest", "最新位点")}</th>
                      <th className="num" scope="col">{text("Unconsumed", "未消费条数")}</th>
                      <th className="num" scope="col">{text("Last message", "最新消费时间")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selected.partitions.map((partition) => (
                      <tr key={partition.partition}>
                        <td className="mono">p{partition.partition}</td>
                        <td className="num">
                          {partition.consumed && /^\d+$/.test(partition.consumed)
                            ? count(partition.consumed)
                            : partition.consumed ?? "—"}
                        </td>
                        <td className="num">{count(partition.latest)}</td>
                        <td className={`num${isBig(partition.lag) ? " load-num-critical" : ""}`}>
                          {partition.lag === null ? (
                            <StateStamp state="missing" />
                          ) : (
                            <strong>{count(partition.lag)}</strong>
                          )}
                        </td>
                        <td className="num">{ago(partition.consumed_message_ms, nowMs, text)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          )}
        </Card>
      </PageGrid>
    </div>
  );
}
