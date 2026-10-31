// Design board I1: Routine Load jobs as StarRocks reports them now. The
// server decodes every field; this only lays them out. Read-only: no
// pause, resume or change.
import { useState } from "react";
import type { JSX } from "react";
import type { RoutineLoadJob, RoutineLoadJobsResponse } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { ago, bytes, compact, count, feTime, ms, routineTone } from "./load-text";

const BIG_LAG = 10_000n;

function isBig(lag: string | null): boolean {
  return lag !== null && /^\d+$/.test(lag) && BigInt(lag) > BIG_LAG;
}

function noteOf(job: RoutineLoadJob, text: LocalizedText): string {
  if (job.state_class === "paused" || job.state_class === "cancelled" || job.state_class === "stopped") {
    return job.reason ?? job.other_message ?? "—";
  }
  if (job.max_lag_partition !== null && isBig(job.max_lag)) return text(`Lag concentrated in partition ${job.max_lag_partition}`, `积压集中在分区 ${job.max_lag_partition}`);
  return job.other_message ?? "—";
}

export function RoutineLoadPanel({ jobs, initialJob, nowMs, onOpenLog, text }: {
  jobs: RoutineLoadJobsResponse;
  /** The job opened from the overview, selected first. */
  initialJob?: string | null;
  /** Browser time, only to say how long ago a message was consumed. */
  nowMs: number;
  onOpenLog(job: RoutineLoadJob): void;
  text: LocalizedText;
}): JSX.Element {
  const [picked, setPicked] = useState<string | null>(initialJob ?? null);
  const [stateFilter, setStateFilter] = useState<string>("all");
  if (jobs.support === "unsupported_version") {
    return <p className="history-banner">{text("Routine Load jobs need StarRocks 3.2 or later (information_schema.routine_load_jobs).", "Routine Load 作业需要 StarRocks 3.2 及以上（information_schema.routine_load_jobs）。")}</p>;
  }
  const visible = jobs.jobs.filter((job) => stateFilter === "all" || job.state_class === stateFilter);
  const selected = visible.find((job) => job.job_id === picked) ?? visible[0] ?? null;
  const summary = jobs.summary;
  const worst = summary.lags[0] ?? null;
  return <div className="history-page load-page">
    <div className="notify-kpis">
      <div className="history-card"><span className="history-muted">{text("Running", "运行中")}</span><strong>{count(summary.running)}</strong>{summary.scheduling !== "0" && <span className="history-muted">{text(`${summary.scheduling} waiting to be scheduled`, `另有 ${summary.scheduling} 个等待调度`)}</span>}</div>
      <div className={`history-card ${summary.paused !== "0" ? "load-card-bad" : ""}`}><span className="history-muted">{text("Paused / ended", "已暂停 / 已结束")}</span><strong className={summary.paused !== "0" ? "bad" : ""}>{summary.paused} / {summary.ended}</strong></div>
      <div className="history-card"><span className="history-muted">{text("Largest partition lag (live jobs)", "最大分区积压（未结束的作业）")}</span><strong>{jobs.support === "without_lag" ? text("unavailable", "不可用") : worst ? count(worst.lag) : "—"}</strong>{worst && <span className="history-muted">{worst.db}.{worst.table} · p{worst.partition}</span>}</div>
      <div className="history-card"><span className="history-muted">{text("Error rows (job lifetime)", "累计错误行（作业生命周期）")}</span><strong>{summary.error_rows_unknown !== "0" && summary.error_rows === "0" ? "—" : count(summary.error_rows)}</strong>{summary.error_rows_unknown !== "0" && <span className="history-muted">{text(`${summary.error_rows_unknown} jobs report none; left out`, `${summary.error_rows_unknown} 个作业未报告，未计入`)}</span>}</div>
    </div>
    <div className="history-row">
      <section aria-labelledby="routine-jobs" className="history-card wide">
        <div className="history-card-head">
          <h2 id="routine-jobs">{text("Jobs", "作业")}</h2>
          <label className="notify-filter">{text("State", "状态")}
            <select onChange={(event) => setStateFilter(event.target.value)} value={stateFilter}>
              <option value="all">{text("All", "全部")}</option>
              <option value="running">RUNNING</option><option value="scheduling">NEED_SCHEDULE</option><option value="paused">PAUSED</option>
              <option value="stopped">STOPPED</option><option value="cancelled">CANCELLED</option>
            </select>
          </label>
        </div>
        {visible.length === 0 ? <p className="notify-empty">{jobs.jobs.length === 0 ? text("No Routine Load job is visible to this account.", "这个账号看不到任何 Routine Load 作业。") : text("No job in this state.", "这个状态下没有作业。")}</p> : <div className="history-table-wrap">
          <table className="history-table load-table">
            <thead><tr>
              <th>{text("Job", "作业")}</th><th>{text("State", "状态")}</th><th className="num">{text("Largest lag", "最大积压")}</th>
              <th className="num">{text("Rows / s", "导入速率")}</th><th className="num">{text("Error / total rows", "错误行 / 总行")}</th><th>{text("Notes", "说明")}</th>
            </tr></thead>
            <tbody>
              {visible.map((job) => <tr className={job.job_id === selected?.job_id ? "selected" : undefined} key={job.job_id}>
                <td><button aria-pressed={job.job_id === selected?.job_id} className="load-link" onClick={() => setPicked(job.job_id)} type="button"><strong>{job.name}</strong><span className="history-muted">{job.db}.{job.table} · {job.source_type}{job.topic ? ` ${job.topic}` : ""}</span></button></td>
                <td><span className={`notify-chip ${routineTone(job.state_class)}`}>{job.state}</span></td>
                <td className={`num ${isBig(job.max_lag) ? "bad" : ""}`}>{jobs.support === "without_lag" ? "—" : count(job.max_lag)}</td>
                <td className="num">{job.state_class === "running" ? compact(job.statistics.load_rows_rate) : "—"}</td>
                <td className="num">{compact(job.statistics.error_rows)} / {compact(job.statistics.total_rows)}</td>
                <td className="load-note">{noteOf(job, text)}</td>
              </tr>)}
            </tbody>
          </table>
        </div>}
        <p className="history-muted">{text("StarRocks keeps stopped or cancelled jobs for 3 days. Lag is the latest position minus the consumed one (OFFSET_LAG, 3.3+).", "已停止或取消的作业 StarRocks 只保留 3 天（label_keep_max_second）。积压 = 最新位置 − 已消费位置，来自 OFFSET_LAG（3.3 起）。")}</p>
      </section>
      {selected && <section aria-labelledby="routine-detail" className="history-card">
        <span className="history-muted">Routine Load · {selected.db}.{selected.table}</span>
        <h2 id="routine-detail" tabIndex={-1}>{selected.name} <span className={`notify-chip ${routineTone(selected.state_class)}`}>{selected.state}</span></h2>
        {selected.reason && <div className="load-reason-box"><strong>{text("Why the state changed", "状态变化原因")}{selected.pause_time ? ` · ${feTime(selected.pause_time)}` : ""}</strong><span className="mono">{selected.reason}</span></div>}
        <dl className="history-facts">
          <dt>{text("Source", "数据源")}</dt><dd>{selected.source_type}{selected.topic ? ` · ${selected.topic}` : ""} · {text(`${selected.partitions.length} partitions`, `${selected.partitions.length} 个分区`)}</dd>
          <dt>{text("Current tasks", "当前任务数")}</dt><dd>{count(selected.current_tasks)}</dd>
          <dt>{text("Committed / aborted tasks", "已提交 / 已中止任务")}</dt><dd>{count(selected.statistics.committed_tasks)} / {count(selected.statistics.aborted_tasks)}</dd>
          <dt>{text("Received / rate", "接收 / 速率")}</dt><dd>{bytes(selected.statistics.received_bytes)} · {bytes(selected.statistics.received_bytes_rate)}/s</dd>
          <dt>{text("Loaded / error / unselected", "导入 / 错误 / 未选中行")}</dt><dd>{count(selected.statistics.loaded_rows)} / {count(selected.statistics.error_rows)} / {count(selected.statistics.unselected_rows)}</dd>
          <dt>{text("Task time (lifetime)", "任务执行耗时（累计）")}</dt><dd>{ms(selected.statistics.task_execute_time_ms)}</dd>
          <dt>{text("Tolerance", "容错设置")}</dt><dd>{selected.max_error_number !== null ? `max_error_number=${selected.max_error_number}` : "—"}</dd>
        </dl>
        <h3 className="trend-subhead">{text("Partitions", "分区进度与积压")}</h3>
        {selected.partitions.length === 0 ? <p className="history-muted">{text("No partition progress reported.", "StarRocks 没有报告分区进度。")}</p> : <div className="history-table-wrap"><table className="history-table load-partitions">
          <thead><tr><th>{text("Partition", "分区")}</th><th className="num">{text("Consumed", "已消费")}</th><th className="num">{text("Latest", "最新位置")}</th><th className="num">{text("Lag", "积压")}</th><th className="num">{text("Newest consumed message", "消费到的消息时间")}</th></tr></thead>
          <tbody>
            {selected.partitions.map((partition) => <tr key={partition.partition}>
              <td className="mono">{partition.partition}</td>
              <td className="num">{partition.consumed && /^\d+$/.test(partition.consumed) ? count(partition.consumed) : partition.consumed ?? "—"}</td>
              <td className="num">{count(partition.latest)}</td>
              <td className={`num ${isBig(partition.lag) ? "bad" : ""}`}>{count(partition.lag)}</td>
              <td className="num">{ago(partition.consumed_message_ms, nowMs, text)}</td>
            </tr>)}
          </tbody>
        </table></div>}
        <div className="alert-actions">
          <button aria-haspopup="dialog" className="button secondary" disabled={!selected.has_error_log} onClick={() => onOpenLog(selected)} type="button">{selected.has_error_log ? text("View error log", "查看错误日志") : jobs.support === "without_lag" ? text("Error logs need 3.3+", "错误日志需要 3.3 及以上") : text("No error log", "没有错误日志")}</button>
          <span className="planned">{text("30-day lag: History tab · alerts: Alerts › Loads", "30 天积压见“历史”标签 · 告警见“告警 › 导入”")}</span>
        </div>
        <p className="history-muted">{text("SR Doctor is read-only: it does not pause, resume or change jobs. Run RESUME ROUTINE LOAD in StarRocks when needed.", "SR Doctor 只读，不提供暂停、恢复或修改作业；需要时在 StarRocks 里执行 RESUME ROUTINE LOAD。")}</p>
      </section>}
    </div>
  </div>;
}
