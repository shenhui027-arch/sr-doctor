// Design boards I6-I8 (task 56): the overview of one load type. Each shows
// only what StarRocks reports for that type; the server computes every
// count, share and bar length.
import type { JSX } from "react";
import type { BatchLoadDetail, LoadDurationDistribution, LoadFailureReason, LoadGroupTable, LoadOverviewResponse, LoadSlowRecord, RoutineLoadDetail, StreamLoadDetail } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { HourChart, KpiCards, LagChart, PhaseChart, bar, failedCard, filteredCard, lagCard, lagStateText, loadsCard, routineJobsCard, type OverviewActions } from "./LoadOverviewPanel";
import { ago, bytes, compact, count, durationBinText, failureReasonText, feTime, ms, routineTone, runningStateText, seconds, shareText } from "./load-text";

type ViewProps = OverviewActions & { overview: LoadOverviewResponse };

function topReasons(reasons: LoadFailureReason[], text: LocalizedText): string {
  return reasons.slice(0, 3).map((reason) => `${failureReasonText(reason.reason, text)} ${reason.loads}`).join(" · ");
}

export function StreamView({ overview, detail, ...actions }: ViewProps & { detail: StreamLoadDetail }): JSX.Element {
  const { text } = actions;
  const phases = overview.phases;
  return <>
    <KpiCards cards={[
      loadsCard(overview, text, text("Loads", "次数")),
      failedCard(overview, topReasons(detail.failure_reasons, text) || text("state = CANCELLED", "state = CANCELLED"), text),
      filteredCard(overview, text(`${compact(detail.unselected_rows)} rows unselected`, `另有未选中行 ${compact(detail.unselected_rows)}`), text, detail.unselected_unknown !== "0" ? text(`unselected rows: ${detail.unselected_unknown} unknown, left out`, `未选中行 ${detail.unselected_unknown} 条未知，未计入`) : null),
      { label: text("Data received", "接收数据"), value: bytes(detail.received_bytes), note: `${text("written", "写入")} ${count(overview.summary.loaded_rows)} ${text("rows", "行")}`, caveat: detail.received_bytes_unknown !== "0" ? text(`${detail.received_bytes_unknown} unknown, left out`, `${detail.received_bytes_unknown} 条未知，未计入`) : null },
      { label: text("Begin txn P95", "取事务 P95"), value: phases.begin_txn.p95_ms !== null ? ms(phases.begin_txn.p95_ms) : "—", note: phases.begin_txn.p95_ms !== null ? `P50 ${ms(phases.begin_txn.p50_ms)} · ${phases.begin_txn.samples} ${text("records", "条")}` : text("fewer than 20 samples", "样本不足 20 条") },
      { label: text("Plan P95", "规划 P95"), value: phases.plan.p95_ms !== null ? ms(phases.plan.p95_ms) : "—", note: phases.plan.p95_ms !== null ? `P50 ${ms(phases.plan.p50_ms)} · ${phases.plan.samples} ${text("records", "条")}` : text("fewer than 20 samples", "样本不足 20 条") },
    ]} label={text("Stream Load summary", "Stream Load 汇总")} />
    <div className="history-row">
      <HourChart mode="outcome" onOpenHour={actions.onOpenHour} overview={overview} text={text} />
      <FailureReasons reasons={detail.failure_reasons} text={text} stream />
    </div>
    <div className="history-row">
      <PhaseChart overview={overview} text={text} />
      <section aria-labelledby="load-stages" className="history-card wide">
        <h2 id="load-stages">{text("Commit and publish", "提交与发布")} <span className="history-muted">{text("second precision", "秒级")}</span></h2>
        <dl className="load-facts">
          <div><dt>{text("Total time P95", "总耗时 P95")}</dt><dd>{seconds(detail.total.p95_seconds)}</dd><dd className="history-muted">{detail.total.p95_seconds !== null ? `P50 ${seconds(detail.total.p50_seconds)} · ${text("longest", "最长")} ${seconds(detail.total.max_seconds)}` : text("fewer than 20 finished", "已结束的不足 20 条")}</dd></div>
          <div><dt>{text("Publish wait P95", "发布等待 P95")}</dt><dd>{seconds(detail.publish_p95_seconds)}</dd><dd className="history-muted">{text("commit → finish", "提交 → 完成")}</dd></div>
          <div className={detail.near_timeout_count !== "0" ? "bad" : undefined}><dt>{text("Near the timeout", "接近超时")}</dt><dd>{count(detail.near_timeout_count)}</dd><dd className="history-muted">{text("used 80% or more of the timeout", "用了超时设置的 80% 以上")}</dd></div>
        </dl>
        {detail.near_timeout.length > 0 && <SlowList records={detail.near_timeout} text={text} segments={false} />}
        <p className="history-muted">{text("Total = finish − creation; publish wait = finish − commit. Times have second precision, so under a second is 0. The timeout comes from PROPERTIES.timeout.", "总耗时 = 完成 − 创建；发布等待 = 完成 − 提交。时间点只到秒，不足 1 秒记为 0。超时设置来自 PROPERTIES.timeout。")}</p>
      </section>
    </div>
    <div className="history-row">
      <GroupTables onOpenTable={actions.onOpenTable} stream tables={detail.tables} text={text} />
      <section aria-labelledby="load-sources" className="history-card load-side">
        <h2 id="load-sources">{text("Sources", "来源")} <span className="history-muted">{text("by client IP and user", "按客户端 IP 与用户")}</span></h2>
        {detail.sources.length === 0 ? <p className="notify-empty">{text("No source reported.", "没有来源信息。")}</p>
          : <div className="history-table-wrap"><table className="history-table load-table">
            <thead><tr><th scope="col">{text("Client", "客户端")}</th><th scope="col">{text("User", "用户")}</th><th className="num" scope="col">{text("Loads", "次数")}</th><th className="num" scope="col">{text("Failed", "失败")}</th></tr></thead>
            <tbody>{detail.sources.map((source) => <tr key={`${source.client_ip ?? ""}/${source.user ?? ""}`}>
              <th className="mono" scope="row">{source.client_ip ?? "—"}</th><td className="mono">{source.user ?? "—"}</td><td className="num">{count(source.loads)}</td><td className={`num${source.failed !== "0" ? " bad" : ""}`}>{count(source.failed)}</td>
            </tr>)}</tbody>
          </table></div>}
        <p className="history-muted">{text("The client IP is RUNTIME_DETAILS.client_ip; behind a load balancer or proxy it is the proxy's address.", "客户端 IP 来自 RUNTIME_DETAILS.client_ip；经负载均衡或代理时是代理地址。")}</p>
      </section>
    </div>
  </>;
}

export function BatchView({ overview, detail, broker, ...actions }: ViewProps & { detail: BatchLoadDetail; broker: boolean }): JSX.Element {
  const { text } = actions;
  const running = detail.running_states.map((state) => `${runningStateText(state.state, text)} ${state.loads}`).join(" · ");
  const name = broker ? "Broker Load" : "INSERT";
  return <>
    <KpiCards cards={[
      { ...loadsCard(overview, text, text("Loads", "次数")), note: running ? `${text("not finished", "未结束")}: ${running}` : loadsCard(overview, text).note },
      failedCard(overview, topReasons(detail.failure_reasons, text) || text("state = CANCELLED", "state = CANCELLED"), text),
      { label: text("Rows written", "写入行"), value: compact(overview.summary.loaded_rows), note: text("SINK_ROWS summed", "SINK_ROWS 之和"), caveat: overview.summary.loaded_rows_unknown !== "0" ? text(`${overview.summary.loaded_rows_unknown} unknown, left out`, `${overview.summary.loaded_rows_unknown} 条未知，未计入`) : null },
      { label: text("Scanned", "扫描"), value: compact(detail.scan_rows), unit: text("rows", "行"), note: `${text("data", "数据")} ${bytes(detail.scan_bytes)}`, caveat: detail.counts_unknown !== "0" ? text(`${detail.counts_unknown} with an unknown count, left out`, `${detail.counts_unknown} 条有未知计数，未计入`) : null },
      { label: text("Total time P95", "总耗时 P95"), value: seconds(detail.duration.p95_seconds), note: detail.duration.p95_seconds !== null ? `P50 ${seconds(detail.duration.p50_seconds)} · ${text("second precision", "秒级")}` : text("fewer than 20 finished", "已结束的不足 20 条") },
      { label: text("Near the timeout", "接近超时"), value: count(detail.near_timeout_count), note: text("used 80% or more of the timeout", "用了超时设置的 80% 以上"), bad: detail.near_timeout_count !== "0" },
    ]} label={text(`${name} summary`, `${name} 汇总`)} />
    <div className="history-row">
      <HourChart mode="outcome" onOpenHour={actions.onOpenHour} overview={overview} text={text} />
      <DurationCard distribution={detail.duration} note={broker ? text("Broker Load waits in a queue and prepares before it loads; that time is part of the total and of “execute” below.", "Broker Load 加载前有排队与准备，这段时间计入总耗时，也计入下方的“执行”段。") : text("INSERT starts loading when it is created, so there is no queue phase.", "INSERT 的开始加载时间就是创建时间，所以没有“排队”这一段。")} text={text} title={text("Total time", "总耗时分布")} />
    </div>
    <div className="history-row">
      <section aria-labelledby="load-slowest" className="history-card wide">
        <div className="history-card-head">
          <h2 id="load-slowest">{text(`Slowest ${name}`, `最慢的 ${name}`)} <span className="history-muted">{text("execute and publish", "执行与发布两段")}</span></h2>
          <span className="load-legend">
            <span><i className="run" aria-hidden="true" />{broker ? text("Before commit (creation → commit, queue included)", "提交前（创建 → 提交，含排队）") : text("Execute (creation → commit)", "执行（创建 → 提交）")}</span>
            <span><i className="warn" aria-hidden="true" />{text("Publish (commit → finish)", "发布（提交 → 完成）")}</span>
          </span>
        </div>
        {detail.slowest.length === 0 ? <p className="notify-empty">{text("No finished load with a known total time.", "没有已结束且总耗时已知的导入。")}</p> : <SlowList records={detail.slowest} segments text={text} />}
        <p className="history-muted">{text("The right column is the share of the timeout (PROPERTIES.timeout) used; 80% or more is red.", "右列是总耗时占超时设置（PROPERTIES.timeout）的比例，≥ 80% 标红。")}</p>
      </section>
      <div className="load-side-stack">
        <FailureReasons reasons={detail.failure_reasons} text={text} />
        <section aria-labelledby="load-running" className="history-card load-side">
          <h2 id="load-running">{text("Not finished", "进行中")}</h2>
          <p className="load-running-states">{running || text("None", "无")}</p>
        </section>
      </div>
    </div>
    <GroupTables onOpenTable={actions.onOpenTable} stream={false} tables={detail.tables} text={text} />
  </>;
}

export function RoutineView({ overview, detail, ...actions }: ViewProps & { detail: RoutineLoadDetail }): JSX.Element {
  const { text, nowMs } = actions;
  const tasksSupported = overview.support !== "unsupported_version";
  return <>
    <KpiCards cards={[
      routineJobsCard(overview, text),
      lagCard(overview, text),
      { label: text("Lifetime error rate", "累计错误率"), value: shareText(detail.error_share_permyriad) ?? "—", note: `${text("error rows", "错误行")} ${count(detail.error_rows)} / ${text("total", "总行")} ${detail.total_rows !== null ? compact(detail.total_rows) : "—"}`, caveat: detail.rows_unknown !== "0" ? text(`${detail.rows_unknown} jobs unreported, left out`, `${detail.rows_unknown} 个作业未报告，未计入`) : null },
      { label: text("Aborted tasks", "中止任务占比"), value: shareText(detail.abort_share_permyriad) ?? "—", note: `${text("committed", "提交")} ${count(detail.committed_tasks)} · ${text("aborted", "中止")} ${count(detail.aborted_tasks)}`, caveat: detail.tasks_unknown !== "0" ? text(`${detail.tasks_unknown} jobs unreported, left out`, `${detail.tasks_unknown} 个作业未报告，未计入`) : null, bad: (detail.abort_share_permyriad ?? 0) >= 1000 },
      { label: text("Recent tasks", "近期任务"), value: tasksSupported ? count(detail.held_tasks) : "—", note: tasksSupported ? `${text("held in loads", "loads 中的任务")}${overview.earliest_create_time ? ` · ${text("oldest", "最早")} ${feTime(overview.earliest_create_time)}` : ""}` : text("needs 3.3+", "需要 3.3 及以上") },
      { label: text("Average task time", "平均任务执行"), value: ms(detail.average_task_ms), note: text("task time ÷ committed tasks", "累计执行时间 ÷ 提交任务数"), caveat: detail.average_task_ms === null && (detail.tasks_unknown !== "0" || (detail.committed_tasks ?? "0") !== "0") ? text("not given: a job reports no task count or task time", "有作业没有报告任务数或执行时间，不给平均值") : null },
    ]} label={text("Routine Load summary", "Routine Load 汇总")} />
    <section aria-labelledby="load-jobs" className="history-card wide">
      <h2 id="load-jobs">{text("Job health, outcomes and throughput", "作业健康、成败与吞吐")} <span className="history-muted">{text("routine_load_jobs · since each job was created", "routine_load_jobs · 作业创建以来的累计值")}</span></h2>
      {detail.jobs.length === 0 ? <p className="notify-empty">{text("No Routine Load job.", "没有 Routine Load 作业。")}</p>
        : <div className="history-table-wrap"><table className="history-table load-table load-jobs-table">
          <thead><tr>
            <th scope="col">{text("Job", "作业")}</th><th scope="col">{text("State", "状态")}</th><th className="num" scope="col">{text("Tasks now / desired", "任务 当前 / 期望")}</th>
            <th className="num" scope="col">{text("Average load rate", "平均导入速率")}</th><th className="num" scope="col">{text("Average received", "平均接收")}</th><th className="num" scope="col">{text("Rows loaded", "累计导入行")}</th>
            <th className="num" scope="col">{text("Error rows (rate)", "错误行（率）")}</th><th className="num" scope="col">{text("Tasks committed / aborted", "任务 提交 / 中止")}</th><th className="num" scope="col">{text("Average task", "平均任务执行")}</th>
            <th className="num" scope="col">{text("Largest lag", "最大积压")}</th><th className="num" scope="col">{text("Newest consumed", "最新消费消息")}</th>
          </tr></thead>
          <tbody>{detail.jobs.map((job) => <tr key={job.job_id}>
            <th scope="row"><button className="load-link" data-drill={`job:${job.job_id}`} onClick={() => actions.onOpenJob(job.job_id)} type="button"><strong className="mono">{job.name}</strong><span className="load-sub history-muted">{job.db}.{job.table}</span></button></th>
            <td><span className={`load-state ${routineTone(job.state_class)}`}>{lagStateText(job.state_class, text)}</span></td>
            <td className="num">{count(job.current_tasks)} / {count(job.desired_tasks)}</td>
            <td className="num">{job.load_rows_rate !== null ? text(`${count(job.load_rows_rate)} rows/s`, `${count(job.load_rows_rate)} 行/s`) : "—"}</td>
            <td className="num">{job.received_bytes_rate !== null ? `${bytes(job.received_bytes_rate)}/s` : "—"}</td>
            <td className="num">{count(job.loaded_rows)}</td>
            <td className={`num${(job.error_share_permyriad ?? 0) > 0 ? " bad" : ""}`}>{count(job.error_rows)}{job.error_share_permyriad !== null && <span className="load-sub">{shareText(job.error_share_permyriad)}</span>}</td>
            <td className={`num${job.aborted_tasks !== null && job.aborted_tasks !== "0" ? " bad" : ""}`}>{count(job.committed_tasks)} / {count(job.aborted_tasks)}{job.abort_share_permyriad !== null && job.aborted_tasks !== "0" && <span className="load-sub">{shareText(job.abort_share_permyriad)}</span>}</td>
            <td className="num">{ms(job.average_task_ms)}</td>
            <td className="num">{job.max_lag !== null ? `${count(job.max_lag)}${job.max_lag_partition !== null ? ` · p${job.max_lag_partition}` : ""}` : "—"}</td>
            <td className="num">{ago(job.newest_consumed_ms, nowMs, text)}</td>
          </tr>)}</tbody>
        </table></div>}
      <p className="history-muted">{text("Rates are StarRocks' lifetime averages (rows or bytes over total task time), not the current rate; average task = task time ÷ committed tasks (aborted tasks add no time). Error rate = error rows ÷ total rows. A job pauses when the error rows within the last max_batch_rows × 10 rows exceed max_error_number; StarRocks does not expose that window, so no distance to the pause is estimated here.", "速率是 StarRocks 给出的“累计导入行（或字节）÷ 累计任务执行时间”，表示作业整个生命周期的平均吞吐，不是当前速率；平均任务执行 = 累计任务执行时间 ÷ 提交任务数（中止的任务不计时间）。错误率 = 累计错误行 ÷ 累计总行。自动暂停按最近 max_batch_rows × 10 行内的错误行是否超过 max_error_number 判断，这个窗口计数 StarRocks 不对外提供，所以这里不估算“离自动暂停还差多少”。")}</p>
    </section>
    {tasksSupported ? <div className="history-row">
      <HourChart mode="outcome" onOpenHour={actions.onOpenHour} overview={overview} text={text} unitLabel={text("tasks", "任务")} />
      <DurationCard distribution={detail.tasks} note={text("Task time = finish − creation (seconds), including scheduling, consuming, commit and publish; it alone cannot tell whether a batch ended on max_batch_interval, rows or bytes. Routine Load tasks have no millisecond phases.", "任务耗时 = 完成 − 创建（秒级），含调度、消费、提交与发布；单凭它看不出批次是因 max_batch_interval、行数还是字节数结束的。Routine Load 任务没有取事务、规划等毫秒级阶段耗时。")} text={text} title={text("Task time", "任务耗时分布")} />
    </div> : <p className="history-banner neutral">{text("Routine Load task records need StarRocks 3.3 or later; only the job totals are shown.", "Routine Load 任务记录需要 StarRocks 3.3 及以上；这里只显示作业累计值。")}</p>}
    <div className="history-row">
      <LagChart nowMs={nowMs} onOpenJob={actions.onOpenJob} overview={overview} text={text} />
      <section aria-labelledby="load-issues" className="history-card wide">
        <h2 id="load-issues">{text("Jobs to look at", "需要处理的作业")} <span className="history-muted">{text("paused or ended", "暂停或已结束")}</span></h2>
        {detail.issues.length === 0 ? <p className="notify-empty">{text("No paused or ended job.", "没有暂停或已结束的作业。")}</p>
          : <ul className="load-issues">{detail.issues.map((issue) => <li className={issue.state_class === "paused" ? "bad" : undefined} key={issue.job_id}>
            <div className="load-issue-head">
              <button className="load-link" data-drill={`issue:${issue.job_id}`} onClick={() => actions.onOpenJob(issue.job_id)} type="button"><strong className="mono">{issue.name}</strong></button>
              <span className={`load-state ${issue.state_class === "paused" ? "bad" : "off"}`}>{lagStateText(issue.state_class, text)}</span>
              <span className="history-muted">{issue.state_class === "paused" ? text("paused at", "暂停于") : text("ended at", "结束于")} {feTime(issue.time)}</span>
            </div>
            <p className="load-reason">{issue.reason ?? text("StarRocks gave no reason.", "StarRocks 没有给出原因。")}</p>
            {(issue.error_share_permyriad !== null || (issue.aborted_tasks !== null && issue.aborted_tasks !== "0")) && <p className="history-muted">{text(`Lifetime error rate ${shareText(issue.error_share_permyriad) ?? "—"} · aborted tasks ${count(issue.aborted_tasks)}`, `累计错误率 ${shareText(issue.error_share_permyriad) ?? "—"} · 中止任务 ${count(issue.aborted_tasks)}`)}</p>}
          </li>)}</ul>}
      </section>
    </div>
    {overview.routine.support === "without_lag" && <p className="history-banner neutral">{text("StarRocks 3.2 has no lag (OFFSET_LAG), error logs or task records; only the job totals and states are shown.", "3.2：没有积压（OFFSET_LAG）、错误日志和任务记录，只显示作业表中的累计值与状态。")}</p>}
  </>;
}

function DurationCard({ distribution, title, note, text }: { distribution: LoadDurationDistribution; title: string; note: string; text: LocalizedText }): JSX.Element {
  return <section aria-labelledby="load-duration" className="history-card load-side">
    <h2 id="load-duration">{title} <span className="history-muted">{text(`seconds · ${distribution.samples} finished`, `秒级 · ${distribution.samples} 条已结束`)}</span></h2>
    {distribution.samples === "0" ? <p className="notify-empty">{text("No finished record with a known time.", "没有已结束且耗时已知的记录。")}</p>
      : <ul aria-label={title} className="load-duration">{distribution.bins.map((bin) => {
        const label = durationBinText(bin.lower_seconds, bin.upper_seconds);
        return <li aria-label={`${label}${text(": ", "：")}${bin.loads}`} key={bin.lower_seconds}>
          <span className="mono">{label}</span>
          <span aria-hidden="true" className="load-bar-track"><span className="ok" style={{ width: bar(bin.permyriad) }} /></span>
          <span className="mono num">{count(bin.loads)}</span>
        </li>;
      })}</ul>}
    <p className="history-muted">{distribution.p95_seconds !== null ? `P50 ${seconds(distribution.p50_seconds)} · P95 ${seconds(distribution.p95_seconds)} · ${text("longest", "最长")} ${seconds(distribution.max_seconds)}. ` : distribution.samples !== "0" ? `${text("Fewer than 20 samples, no percentiles", "样本不足 20 条，不给分位数")} · ${text("longest", "最长")} ${seconds(distribution.max_seconds)}. ` : ""}{note}</p>
  </section>;
}

function FailureReasons({ reasons, text, stream = false }: { reasons: LoadFailureReason[]; text: LocalizedText; stream?: boolean }): JSX.Element {
  return <section aria-labelledby="load-reasons" className="history-card load-side">
    <h2 id="load-reasons">{text("Failure reasons", "失败原因")} <span className="history-muted">{stream ? text("ERROR_MSG grouped", "按 ERROR_MSG 归类") : text("by the type in ERROR_MSG", "按 ERROR_MSG 中的 type 归类")}</span></h2>
    {reasons.length === 0 ? <p className="notify-empty">{text("No failed load.", "没有失败的导入。")}</p>
      : <ul className="load-bars">{reasons.map((reason) => <li className="load-reason-row" key={reason.reason}>
        <span>{failureReasonText(reason.reason, text)}</span>
        <span className="mono">{count(reason.loads)}</span>
        <span aria-hidden="true" className="load-bar-track"><span className="bad" style={{ width: bar(reason.width_permyriad) }} /></span>
        <span className="history-muted">{text("latest", "最近一次")} {feTime(reason.latest_time)} · <span className="mono">{reason.latest_table}</span></span>
      </li>)}</ul>}
    <p className="history-muted">{stream ? text("Only StarRocks' fixed message starts are grouped; the rest is Other. The full text is in the load records.", "归类只认 StarRocks 固定的报错开头；其余归入“其他”，原文在导入记录里。") : text("StarRocks writes failures as “type:<type>; msg:<reason>”; the full text is in the load records.", "StarRocks 把失败写成“type:类型; msg:原因”，按类型归类，原文在导入记录里。")}</p>
  </section>;
}

function SlowList({ records, segments, text }: { records: LoadSlowRecord[]; segments: boolean; text: LocalizedText }): JSX.Element {
  return <ol className="load-bars load-slow">
    {records.map((record) => {
      const share = shareText(record.timeout_share_permyriad);
      const near = (record.timeout_share_permyriad ?? 0) >= 8000;
      const words = text(`${record.table} · ${record.label}: ${record.total_seconds} s${segments ? ` (execute ${record.execute_seconds ?? "—"} s, publish ${record.publish_seconds ?? "—"} s)` : ""}${share ? `, ${share} of the ${record.timeout_seconds} s timeout` : ""}`, `${record.table} · ${record.label}：${record.total_seconds} 秒${segments ? `（执行 ${record.execute_seconds ?? "—"} 秒，发布 ${record.publish_seconds ?? "—"} 秒）` : ""}${share ? `，用了超时 ${record.timeout_seconds} 秒的 ${share}` : ""}`);
      return <li key={record.job_id} title={words}>
        <span className="load-slow-name"><span className="mono load-bar-name" aria-label={words}>{record.table} · {record.label}</span>{segments && <span className="load-sub history-muted">{text(`execute ${seconds(record.execute_seconds)} · publish ${seconds(record.publish_seconds)}`, `执行 ${seconds(record.execute_seconds)} · 发布 ${seconds(record.publish_seconds)}`)}</span>}</span>
        {segments && <span aria-hidden="true" className="load-bar-track split"><span className="run" style={{ width: bar(record.execute_permyriad) }} /><span className="warn" style={{ width: bar(record.publish_permyriad) }} /></span>}
        <span className="mono num">{seconds(record.total_seconds)}</span>
        <span className={`num${near ? " bad" : " history-muted"}`}>{share ? text(`${share} of ${record.timeout_seconds} s`, `超时 ${record.timeout_seconds} 秒的 ${share}`) : "—"}</span>
      </li>;
    })}
  </ol>;
}

function GroupTables({ tables, stream, onOpenTable, text }: { tables: LoadGroupTable[]; stream: boolean; onOpenTable(table: string): void; text: LocalizedText }): JSX.Element {
  return <section aria-labelledby="load-type-tables" className="history-card wide">
    <h2 id="load-type-tables">{stream ? text("By table", "按表") : text("By target table", "按目标表")} <span className="history-muted">{stream ? text("Top 8 · by loads", "Top 8 · 按次数") : text("Top 8 · by rows written", "Top 8 · 按写入行")}</span></h2>
    {tables.length === 0 ? <p className="notify-empty">{text("No record of this type.", "没有这类导入记录。")}</p>
      : <div className="history-table-wrap"><table className="history-table load-table">
        <thead><tr>
          <th scope="col">{text("Table", "表")}</th><th className="num" scope="col">{text("Loads", "次数")}</th><th className="num" scope="col">{text("Failed", "失败")}</th>
          {stream ? <th className="num" scope="col">{text("Received", "接收数据")}</th> : <><th className="num" scope="col">{text("Rows scanned", "扫描行")}</th><th className="num" scope="col">{text("Data scanned", "扫描数据")}</th></>}
          <th className="num" scope="col">{text("Rows written", "写入行")}</th><th className="num" scope="col">{text("Filtered rows", "过滤行")}</th>
          {stream ? <th className="num" scope="col">{text("Plan P95", "规划 P95")}</th> : <th className="num" scope="col">{text("Total time P95", "总耗时 P95")}</th>}
          {!stream && <th scope="col">{text("Priority", "优先级")}</th>}
        </tr></thead>
        <tbody>{tables.map((table) => <tr key={table.table}>
          <th scope="row"><button className="load-link mono" data-drill={`table:${table.table}`} onClick={() => onOpenTable(table.table)} title={text(`Open the load records of ${table.table}`, `查看 ${table.table} 的导入记录`)} type="button">{table.table}</button>{table.counts_unknown !== "0" && <span className="load-sub history-muted">{text(`${table.counts_unknown} with unknown counts`, `${table.counts_unknown} 条计数未知`)}</span>}</th>
          <td className="num">{count(table.loads)}</td>
          <td className={`num${table.failed !== "0" ? " bad" : ""}`}>{count(table.failed)}</td>
          {stream ? <td className="num">{bytes(table.scan_bytes)}</td> : <><td className="num">{count(table.scan_rows)}</td><td className="num">{bytes(table.scan_bytes)}</td></>}
          <td className="num">{count(table.sink_rows)}</td>
          <td className="num">{count(table.filtered_rows)}</td>
          <td className="num">{stream ? ms(table.plan_p95_ms) : seconds(table.total_p95_seconds)}</td>
          {!stream && <td>{table.priority ?? "—"}</td>}
        </tr>)}</tbody>
      </table></div>}
  </section>;
}
