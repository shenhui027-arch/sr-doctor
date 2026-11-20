// Design board 07-loads: per-type overview panels (Stream Load, INSERT / Broker Load,
// and Routine Load fallback when only overview detail is available).
import { useState } from "react";
import type { JSX } from "react";
import type {
  BatchLoadDetail,
  LoadDurationDistribution,
  LoadFailureReason,
  LoadGroupTable,
  LoadOverviewResponse,
  LoadSlowRecord,
  RoutineLoadDetail,
  StreamLoadDetail,
} from "../../../../contracts/generated/doctor-contracts";
import { Card, InfoTip, PageGrid, SegTabs, StateStamp } from "../../design";
import type { LocalizedText } from "../../i18n/locale";
import {
  bar,
  failedCard,
  filteredCard,
  HourChartBody,
  KpiCards,
  lagCard,
  lagStateText,
  LagTableBody,
  loadsCard,
  PhaseChartBody,
  routineJobsCard,
  type OverviewActions,
} from "./LoadOverviewPanel";
import {
  ago,
  bytes,
  compact,
  count,
  durationBinText,
  failureReasonText,
  feTime,
  ms,
  routineDotClass,
  runningStateText,
  seconds,
  shareText,
} from "./load-text";

type ViewProps = OverviewActions & { overview: LoadOverviewResponse };

function topReasons(reasons: LoadFailureReason[], text: LocalizedText): string {
  return reasons
    .slice(0, 3)
    .map((reason) => `${failureReasonText(reason.reason, text)} ${reason.loads}`)
    .join(" · ");
}

export function StreamView({
  overview,
  detail,
  ...actions
}: ViewProps & { detail: StreamLoadDetail }): JSX.Element {
  const { text } = actions;
  const [leftTab, setLeftTab] = useState<"hours" | "phases" | "commit">("hours");
  const [rightTab, setRightTab] = useState<"reasons" | "tables" | "sources">("reasons");
  const phases = overview.phases;
  const beginP95 = phases.begin_txn.p95_ms !== null ? ms(phases.begin_txn.p95_ms) : "—";
  const planP95 = phases.plan.p95_ms !== null ? ms(phases.plan.p95_ms) : "—";

  return (
    <div className="load-view-stack">
      <KpiCards
        cards={[
          loadsCard(overview, text, text("Loads", "导入次数")),
          failedCard(overview, topReasons(detail.failure_reasons, text), text),
          filteredCard(
            overview,
            text(`${compact(detail.unselected_rows)} rows unselected`, `另有未选中行 ${compact(detail.unselected_rows)}`),
            text,
            detail.unselected_unknown !== "0"
              ? text(
                  `unselected rows: ${detail.unselected_unknown} unknown, left out`,
                  `未选中行 ${detail.unselected_unknown} 条未知，未计入`,
                )
              : null,
          ),
          {
            key: "received_bytes",
            label: text("Data received", "接收数据"),
            value: bytes(detail.received_bytes),
            note: `${text("written", "写入")} ${count(overview.summary.loaded_rows)} ${text("rows", "行")}`,
            caveat:
              detail.received_bytes_unknown !== "0"
                ? text(`${detail.received_bytes_unknown} unknown, left out`, `${detail.received_bytes_unknown} 条未知，未计入`)
                : null,
          },
          {
            key: "phase_p95",
            label: text("Begin txn / Plan P95", "取事务 / 规划 P95"),
            value: `${beginP95} / ${planP95}`,
            note:
              phases.begin_txn.p95_ms !== null && phases.plan.p95_ms !== null
                ? `${text("Begin P50", "取事务 P50")} ${ms(phases.begin_txn.p50_ms)} · ${text("Plan P50", "规划 P50")} ${ms(phases.plan.p50_ms)}`
                : text("fewer than 20 samples", "样本不足 20 条"),
          },
        ]}
        label={text("Stream Load summary", "Stream Load 汇总")}
      />

      <PageGrid>
        <Card
          actions={
            <SegTabs<"hours" | "phases" | "commit">
              aria-label={text("Stream Load chart switch", "Stream Load 左卡切换")}
              items={[
                { value: "hours", label: text("Hourly trend", "每小时趋势") },
                { value: "phases", label: text("Millisecond phases", "阶段耗时分布") },
                { value: "commit", label: text("Commit & publish", "提交与发布") },
              ]}
              onValueChange={setLeftTab}
              size="sm"
              value={leftTab}
            />
          }
          footer={
            <span>
              {text(
                "Total = finish − creation; publish wait = finish − commit (second precision)",
                "总耗时 = 完成 − 创建；发布等待 = 完成 − 提交（秒级精度）",
              )}
            </span>
          }
          span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
          title={text("Stream Load timing & volume", "Stream Load 趋势与耗时")}
          variant="chart"
        >
          <div hidden={leftTab !== "hours"}>
            <HourChartBody mode="outcome" onOpenHour={actions.onOpenHour} overview={overview} text={text} />
          </div>
          <div hidden={leftTab !== "phases"}>
            <PhaseChartBody overview={overview} text={text} />
          </div>
          <div className="detail-stack" hidden={leftTab !== "commit"}>
            <dl className="load-facts">
              <div>
                <dt>{text("Total time P95", "总耗时 P95")}</dt>
                <dd>{seconds(detail.total.p95_seconds)}</dd>
                <dd className="card-subtitle">
                  {detail.total.p95_seconds !== null
                    ? `P50 ${seconds(detail.total.p50_seconds)} · ${text("longest", "最长")} ${seconds(detail.total.max_seconds)}`
                    : text("fewer than 20 finished", "已结束的不足 20 条")}
                </dd>
              </div>
              <div>
                <dt>{text("Publish wait P95", "发布等待 P95")}</dt>
                <dd>{seconds(detail.publish_p95_seconds)}</dd>
                <dd className="card-subtitle">{text("commit → finish", "提交 → 完成")}</dd>
              </div>
              <div className={detail.near_timeout_count !== "0" ? "bad" : undefined}>
                <dt>{text("Near timeout", "接近超时")}</dt>
                <dd>{count(detail.near_timeout_count)}</dd>
                <dd className="card-subtitle">
                  {text("used ≥ 80% of timeout", "已使用超时设置的 80% 以上")}
                </dd>
              </div>
            </dl>
            {detail.near_timeout.length > 0 ? (
              <SlowList records={detail.near_timeout} segments={false} text={text} />
            ) : null}
          </div>
        </Card>

        <Card
          actions={
            <SegTabs<"reasons" | "tables" | "sources">
              aria-label={text("Stream Load breakdown switch", "Stream Load 右卡切换")}
              items={[
                { value: "reasons", label: text("Failures", "失败原因") },
                { value: "tables", label: text("By table", "按表") },
                { value: "sources", label: text("Sources", "客户端来源") },
              ]}
              onValueChange={setRightTab}
              size="sm"
              value={rightTab}
            />
          }
          footer={
            <span>
              {text(
                "Behind a load balancer or proxy, client IP reflects the proxy address",
                "经负载均衡或代理转发时，客户端 IP 显示为代理地址",
              )}
            </span>
          }
          span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
          title={text("Failures, tables & sources", "失败原因、分表与来源")}
          variant="table"
        >
          <div className="card-bd--compact" hidden={rightTab !== "reasons"}>
            <FailureReasonsBody reasons={detail.failure_reasons} text={text} />
          </div>
          <div hidden={rightTab !== "tables"}>
            <GroupTablesBody onOpenTable={actions.onOpenTable} stream tables={detail.tables} text={text} />
          </div>
          <div hidden={rightTab !== "sources"}>
            {detail.sources.length === 0 ? (
              <div className="state-box">
                <span>{text("No source reported.", "没有客户端来源信息。")}</span>
              </div>
            ) : (
              <table aria-label={text("Stream Load sources", "Stream Load 来源表")} className="data-table">
                <thead>
                  <tr>
                    <th scope="col">{text("Client", "客户端")}</th>
                    <th scope="col">{text("User", "用户")}</th>
                    <th className="num" scope="col">{text("Loads", "次数")}</th>
                    <th className="num" scope="col">{text("Failed", "失败")}</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.sources.map((source) => (
                    <tr key={`${source.client_ip ?? ""}/${source.user ?? ""}`}>
                      <th className="mono" scope="row">
                        {source.client_ip ?? "—"}
                      </th>
                      <td className="mono">{source.user ?? "—"}</td>
                      <td className="num">{count(source.loads)}</td>
                      <td className={`num${source.failed !== "0" ? " load-num-critical" : ""}`}>
                        {count(source.failed)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </Card>
      </PageGrid>
    </div>
  );
}

export function BatchView({
  overview,
  detail,
  broker,
  ...actions
}: ViewProps & { detail: BatchLoadDetail; broker: boolean }): JSX.Element {
  const { text } = actions;
  const [leftTab, setLeftTab] = useState<"hours" | "duration" | "slowest">("hours");
  const [rightTab, setRightTab] = useState<"reasons" | "tables">("reasons");
  const running = detail.running_states
    .map((state) => `${runningStateText(state.state, text)} ${state.loads}`)
    .join(" · ");
  const name = broker ? "Broker Load" : "INSERT";

  return (
    <div className="load-view-stack">
      <KpiCards
        cards={[
          {
            ...loadsCard(overview, text, text("Loads", "导入次数")),
            note: running ? `${text("not finished", "未结束")}: ${running}` : loadsCard(overview, text).note,
          },
          failedCard(overview, topReasons(detail.failure_reasons, text), text),
          {
            key: "written",
            label: text("Rows written", "写入行"),
            value: compact(overview.summary.loaded_rows),
            unit: text("rows", "行"),
            caveat:
              overview.summary.loaded_rows_unknown !== "0"
                ? text(
                    `${overview.summary.loaded_rows_unknown} unknown, left out`,
                    `${overview.summary.loaded_rows_unknown} 条未知，未计入`,
                  )
                : null,
          },
          {
            key: "scanned",
            label: text("Scanned", "扫描"),
            value: compact(detail.scan_rows),
            unit: text("rows", "行"),
            note: `${text("data", "数据")} ${bytes(detail.scan_bytes)}`,
            caveat:
              detail.counts_unknown !== "0"
                ? text(
                    `${detail.counts_unknown} with an unknown count, left out`,
                    `${detail.counts_unknown} 条有未知计数，未计入`,
                  )
                : null,
          },
          {
            key: "total_p95",
            label: text("Total time P95", "总耗时 P95"),
            value: seconds(detail.duration.p95_seconds),
            note:
              detail.duration.p95_seconds !== null
                ? `P50 ${seconds(detail.duration.p50_seconds)} · ${text("near timeout", "接近超时")} ${count(detail.near_timeout_count)}`
                : text("fewer than 20 finished", "已结束的不足 20 条"),
            bad: detail.near_timeout_count !== "0",
          },
        ]}
        label={text(`${name} summary`, `${name} 汇总`)}
      />

      <PageGrid>
        <Card
          actions={
            <SegTabs<"hours" | "duration" | "slowest">
              aria-label={text(`${name} chart switch`, `${name} 左卡切换`)}
              items={[
                { value: "hours", label: text("Hourly trend", "每小时趋势") },
                { value: "duration", label: text("Duration bins", "总耗时分布") },
                { value: "slowest", label: text(`Slowest ${name}`, `最慢的 ${name}`) },
              ]}
              onValueChange={setLeftTab}
              size="sm"
              value={leftTab}
            />
          }
          footer={
            <span>
              {broker
                ? text(
                    "Broker Load queues and prepares before loading; included in Before commit",
                    "Broker Load 加载前有排队与准备，计入总耗时与“提交前”段",
                  )
                : text(
                    "INSERT starts loading on creation; there is no queue phase",
                    "INSERT 开始加载时间即创建时间，没有“排队”这一段",
                  )}
            </span>
          }
          span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
          title={text(`${name} trend & duration`, `${name} 趋势与耗时`)}
          variant="chart"
        >
          <div hidden={leftTab !== "hours"}>
            <HourChartBody mode="outcome" onOpenHour={actions.onOpenHour} overview={overview} text={text} />
          </div>
          <div hidden={leftTab !== "duration"}>
            <DurationBody distribution={detail.duration} text={text} title={text("Total time", "总耗时分布")} />
          </div>
          <div className="detail-stack" hidden={leftTab !== "slowest"}>
            <div className="load-legend">
              <span>
                <i aria-hidden="true" className="run" />
                {broker
                  ? text("Before commit (creation → commit, queue included)", "提交前（创建 → 提交，含排队）")
                  : text("Execute (creation → commit)", "执行（创建 → 提交）")}
              </span>
              <span>
                <i aria-hidden="true" className="warn" />
                {text("Publish (commit → finish)", "发布（提交 → 完成）")}
              </span>
            </div>
            {detail.slowest.length === 0 ? (
              <div className="state-box">
                <span>{text("No finished load with a known total time.", "没有已结束且总耗时已知的导入。")}</span>
              </div>
            ) : (
              <SlowList records={detail.slowest} segments text={text} />
            )}
          </div>
        </Card>

        <Card
          actions={
            <SegTabs<"reasons" | "tables">
              aria-label={text(`${name} breakdown switch`, `${name} 右卡切换`)}
              items={[
                { value: "reasons", label: text("Failures & active", "失败与进行中") },
                { value: "tables", label: text("By target table", "按目标表") },
              ]}
              onValueChange={setRightTab}
              size="sm"
              value={rightTab}
            />
          }
          footer={
            <span>
              {text("Active states", "进行中状态")}：{running || text("None", "无")}
            </span>
          }
          span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
          title={text(`${name} failures & tables`, `${name} 异常与分表`)}
          variant="table"
        >
          <div className="card-bd--compact" hidden={rightTab !== "reasons"}>
            <FailureReasonsBody reasons={detail.failure_reasons} text={text} />
          </div>
          <div hidden={rightTab !== "tables"}>
            <GroupTablesBody onOpenTable={actions.onOpenTable} stream={false} tables={detail.tables} text={text} />
          </div>
        </Card>
      </PageGrid>
    </div>
  );
}

export function RoutineView({
  overview,
  detail,
  ...actions
}: ViewProps & { detail: RoutineLoadDetail }): JSX.Element {
  const { text, nowMs } = actions;
  const [rightTab, setRightTab] = useState<"lag" | "issues" | "tasks">("lag");
  const tasksSupported = overview.support !== "unsupported_version";

  return (
    <div className="load-view-stack">
      <KpiCards
        cards={[
          routineJobsCard(overview, text),
          lagCard(overview, text),
          {
            key: "error_rate",
            label: text("Lifetime error rate", "累计错误率"),
            value: shareText(detail.error_share_permyriad) ?? "—",
            note: `${text("error rows", "错误行")} ${count(detail.error_rows)} / ${text("total", "总行")} ${detail.total_rows !== null ? compact(detail.total_rows) : "—"}`,
            caveat:
              detail.rows_unknown !== "0"
                ? text(`${detail.rows_unknown} jobs unreported, left out`, `${detail.rows_unknown} 个作业未报告，未计入`)
                : null,
          },
          {
            key: "aborted_tasks",
            label: text("Aborted tasks", "中止任务占比"),
            value: shareText(detail.abort_share_permyriad) ?? "—",
            note: `${text("committed", "提交")} ${count(detail.committed_tasks)} · ${text("aborted", "中止")} ${count(detail.aborted_tasks)}`,
            caveat:
              detail.tasks_unknown !== "0"
                ? text(`${detail.tasks_unknown} jobs unreported, left out`, `${detail.tasks_unknown} 个作业未报告，未计入`)
                : null,
            bad: (detail.abort_share_permyriad ?? 0) >= 1000,
          },
          {
            key: "avg_task",
            label: text("Average task time", "平均任务执行"),
            value: ms(detail.average_task_ms),
            note: tasksSupported
              ? `${text("held tasks", "近期任务")} ${count(detail.held_tasks)}`
              : text("task records need 3.3+", "任务记录需要 StarRocks 3.3 及以上"),
            caveat:
              detail.average_task_ms === null &&
              (detail.tasks_unknown !== "0" || (detail.committed_tasks ?? "0") !== "0")
                ? text(
                    "not given: a job reports no task count or task time",
                    "有作业没有报告任务数或执行时间，不给平均值",
                  )
                : null,
          },
        ]}
        label={text("Routine Load summary", "Routine Load 汇总")}
      />

      <PageGrid>
        <Card
          footer={
            <span>
              {overview.routine.support === "without_lag"
                ? text(
                    "3.2: no partition lag, error logs or task records; lifetime job totals only",
                    "3.2：没有积压、错误日志和任务记录，只显示作业累计值与状态",
                  )
                : text(
                    "Lifetime throughput = total rows ÷ cumulative task time, not current rate",
                    "吞吐速率为生命周期累计值 ÷ 累计任务耗时，不是当前瞬时速率",
                  )}
            </span>
          }
          span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
          title={text("Routine Load job health", "Routine Load 作业健康与吞吐")}
          titleExtra={
            <InfoTip label={text("About Routine Load task timing", "Routine Load 指标口径说明")}>
              {text(
                "Task time alone cannot distinguish whether a batch ended on max_batch_interval, row limit or byte limit.",
                "任务耗时含调度、消费、提交与发布；单凭它看不出批次是因时间间隔、行数还是字节数结束。",
              )}
            </InfoTip>
          }
          variant="table"
        >
          {detail.jobs.length === 0 ? (
            <div className="state-box">
              <span>{text("No Routine Load job.", "没有 Routine Load 作业。")}</span>
            </div>
          ) : (
            <table aria-label={text("Routine Load jobs", "Routine Load 作业列表")} className="data-table">
              <thead>
                <tr>
                  <th scope="col">{text("Job", "作业")}</th>
                  <th scope="col">{text("State", "状态")}</th>
                  <th className="num" scope="col">{text("Tasks", "任务数")}</th>
                  <th className="num" scope="col">{text("Avg rows/s", "平均导入速率")}</th>
                  <th className="num" scope="col">{text("Avg bytes/s", "平均接收")}</th>
                  <th className="num" scope="col">{text("Error rows", "错误行")}</th>
                  <th className="num" scope="col">{text("Committed / aborted", "提交 / 中止")}</th>
                  <th className="num" scope="col">{text("Unconsumed", "未消费条数")}</th>
                </tr>
              </thead>
              <tbody>
                {detail.jobs.map((job) => (
                  <tr key={job.job_id}>
                    <th scope="row">
                      <button
                        className="row-pick-btn load-row-pick"
                        data-drill={`job:${job.job_id}`}
                        onClick={() => actions.onOpenJob(job.job_id)}
                        type="button"
                      >
                        <strong className="mono">{job.name}</strong>
                        <span className="card-subtitle mono">
                          {job.db}.{job.table}
                        </span>
                      </button>
                    </th>
                    <td>
                      <span className="status-inline">
                        <span aria-hidden="true" className={routineDotClass(job.state_class)} />
                        <span>{lagStateText(job.state_class, text)}</span>
                      </span>
                    </td>
                    <td className="num">
                      {count(job.current_tasks)} / {count(job.desired_tasks)}
                    </td>
                    <td className="num">
                      {job.load_rows_rate !== null
                        ? text(`${count(job.load_rows_rate)} rows/s`, `${count(job.load_rows_rate)} 行/s`)
                        : "—"}
                    </td>
                    <td className="num">
                      {job.received_bytes_rate !== null ? `${bytes(job.received_bytes_rate)}/s` : "—"}
                    </td>
                    <td className={`num${(job.error_share_permyriad ?? 0) > 0 ? " load-num-critical" : ""}`}>
                      {count(job.error_rows)}
                      {job.error_share_permyriad !== null ? (
                        <span className="load-sub">{shareText(job.error_share_permyriad)}</span>
                      ) : null}
                    </td>
                    <td className={`num${job.aborted_tasks !== null && job.aborted_tasks !== "0" ? " load-num-critical" : ""}`}>
                      {count(job.committed_tasks)} / {count(job.aborted_tasks)}
                      {job.abort_share_permyriad !== null && job.aborted_tasks !== "0" ? (
                        <span className="load-sub">{shareText(job.abort_share_permyriad)}</span>
                      ) : null}
                    </td>
                    <td className="num">
                      {job.max_lag !== null
                        ? `${count(job.max_lag)}${job.max_lag_partition !== null ? ` · p${job.max_lag_partition}` : ""}`
                        : "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Card>

        <Card
          actions={
            <SegTabs<"lag" | "issues" | "tasks">
              aria-label={text("Routine Load detail switch", "Routine Load 右卡切换")}
              items={[
                { value: "lag", label: text("Partition lag", "分区积压") },
                { value: "issues", label: text("Paused / ended", "异常作业") },
                { value: "tasks", label: text("Task rhythm", "任务节奏") },
              ]}
              onValueChange={setRightTab}
              size="sm"
              value={rightTab}
            />
          }
          span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
          title={text("Partition lag, issues & tasks", "分区积压与异常作业")}
          variant="table"
        >
          <div hidden={rightTab !== "lag"}>
            <LagTableBody nowMs={nowMs} onOpenJob={actions.onOpenJob} overview={overview} text={text} />
          </div>
          <div className="card-bd--compact" hidden={rightTab !== "issues"}>
            {detail.issues.length === 0 ? (
              <div className="state-box">
                <span>{text("No paused or ended job.", "没有暂停或已结束的作业。")}</span>
              </div>
            ) : (
              <ul className="load-issues">
                {detail.issues.map((issue) => (
                  <li className={issue.state_class === "paused" ? "bad" : undefined} key={issue.job_id}>
                    <div className="load-issue-head">
                      <button
                        className="row-pick-btn mono"
                        data-drill={`issue:${issue.job_id}`}
                        onClick={() => actions.onOpenJob(issue.job_id)}
                        type="button"
                      >
                        <strong>{issue.name}</strong>
                      </button>
                      <span className="status-inline">
                        <span aria-hidden="true" className={routineDotClass(issue.state_class)} />
                        <span>{lagStateText(issue.state_class, text)}</span>
                      </span>
                      <span className="card-subtitle">
                        {issue.state_class === "paused"
                          ? text("paused at", "暂停于")
                          : text("ended at", "结束于")}{" "}
                        {feTime(issue.time)}
                      </span>
                    </div>
                    <p className="load-reason">
                      {issue.reason ?? text("StarRocks gave no reason.", "StarRocks 没有给出原因。")}
                    </p>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <div className="card-bd--compact" hidden={rightTab !== "tasks"}>
            {tasksSupported ? (
              <div className="detail-stack">
                <HourChartBody
                  mode="outcome"
                  onOpenHour={actions.onOpenHour}
                  overview={overview}
                  text={text}
                  unitLabel={text("tasks", "任务")}
                />
                <DurationBody distribution={detail.tasks} text={text} title={text("Task time", "任务耗时分布")} />
              </div>
            ) : (
              <div className="state-box">
                <StateStamp
                  detail={text("Routine Load task records need StarRocks 3.3 or later.", "任务记录需要 StarRocks 3.3 及以上。")}
                  state="missing"
                />
              </div>
            )}
          </div>
        </Card>
      </PageGrid>
    </div>
  );
}

function DurationBody({
  distribution,
  title,
  text,
}: {
  distribution: LoadDurationDistribution;
  title: string;
  text: LocalizedText;
}): JSX.Element {
  if (distribution.samples === "0") {
    return (
      <div className="state-box">
        <span>{text("No finished record with a known time.", "没有已结束且耗时已知的记录。")}</span>
      </div>
    );
  }
  return (
    <div className="detail-stack">
      <ul aria-label={title} className="load-duration">
        {distribution.bins.map((bin) => {
          const label = durationBinText(bin.lower_seconds, bin.upper_seconds);
          return (
            <li aria-label={`${label}${text(": ", "：")}${bin.loads}`} key={bin.lower_seconds}>
              <span className="mono">{label}</span>
              <span aria-hidden="true" className="load-bar-track">
                <span className="ok" style={{ width: bar(bin.permyriad) }} />
              </span>
              <span className="mono num">{count(bin.loads)}</span>
            </li>
          );
        })}
      </ul>
      <p className="card-subtitle">
        {distribution.p95_seconds !== null
          ? `P50 ${seconds(distribution.p50_seconds)} · P95 ${seconds(distribution.p95_seconds)} · ${text("longest", "最长")} ${seconds(distribution.max_seconds)}`
          : `${text("Fewer than 20 samples, no percentiles", "样本不足 20 条，不给分位数")} · ${text("longest", "最长")} ${seconds(distribution.max_seconds)}`}
      </p>
    </div>
  );
}

function FailureReasonsBody({
  reasons,
  text,
}: {
  reasons: LoadFailureReason[];
  text: LocalizedText;
}): JSX.Element {
  if (reasons.length === 0) {
    return (
      <div className="state-box">
        <span>{text("No failed load.", "没有失败的导入。")}</span>
      </div>
    );
  }
  return (
    <ul className="load-bars">
      {reasons.map((reason) => (
        <li className="load-reason-row" key={reason.reason}>
          <span>{failureReasonText(reason.reason, text)}</span>
          <span className="mono">{count(reason.loads)}</span>
          <span aria-hidden="true" className="load-bar-track">
            <span className="bad" style={{ width: bar(reason.width_permyriad) }} />
          </span>
          <span className="card-subtitle">
            {text("latest", "最近一次")} {feTime(reason.latest_time)} ·{" "}
            <span className="mono">{reason.latest_table}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}

function SlowList({
  records,
  segments,
  text,
}: {
  records: LoadSlowRecord[];
  segments: boolean;
  text: LocalizedText;
}): JSX.Element {
  return (
    <ol className="load-bars load-slow">
      {records.map((record) => {
        const share = shareText(record.timeout_share_permyriad);
        const near = (record.timeout_share_permyriad ?? 0) >= 8000;
        const words = text(
          `${record.table} · ${record.label}: ${record.total_seconds} s${segments ? ` (execute ${record.execute_seconds ?? "—"} s, publish ${record.publish_seconds ?? "—"} s)` : ""}${share ? `, ${share} of the ${record.timeout_seconds} s timeout` : ""}`,
          `${record.table} · ${record.label}：${record.total_seconds} 秒${segments ? `（执行 ${record.execute_seconds ?? "—"} 秒，发布 ${record.publish_seconds ?? "—"} 秒）` : ""}${share ? `，超时 ${record.timeout_seconds} 秒的 ${share}` : ""}`,
        );
        return (
          <li key={record.job_id} title={words}>
            <span className="load-slow-name">
              <span aria-label={words} className="mono load-bar-name">
                {record.table} · {record.label}
              </span>
              {segments ? (
                <span className="load-sub card-subtitle">
                  {text(
                    `execute ${seconds(record.execute_seconds)} · publish ${seconds(record.publish_seconds)}`,
                    `执行 ${seconds(record.execute_seconds)} · 发布 ${seconds(record.publish_seconds)}`,
                  )}
                </span>
              ) : null}
            </span>
            {segments ? (
              <span aria-hidden="true" className="load-bar-track split">
                <span className="run" style={{ width: bar(record.execute_permyriad) }} />
                <span className="warn" style={{ width: bar(record.publish_permyriad) }} />
              </span>
            ) : null}
            <span className="mono num">{seconds(record.total_seconds)}</span>
            <span className={`num${near ? " load-num-critical" : " card-subtitle"}`}>
              {share ? text(`${share} of ${record.timeout_seconds} s`, `超时 ${record.timeout_seconds} 秒的 ${share}`) : "—"}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

function GroupTablesBody({
  tables,
  stream,
  onOpenTable,
  text,
}: {
  tables: LoadGroupTable[];
  stream: boolean;
  onOpenTable(table: string): void;
  text: LocalizedText;
}): JSX.Element {
  if (tables.length === 0) {
    return (
      <div className="state-box">
        <span>{text("No record of this type.", "没有这类导入记录。")}</span>
      </div>
    );
  }
  return (
    <table aria-label={text("Loads by table", "按表汇总")} className="data-table">
      <thead>
        <tr>
          <th scope="col">{text("Table", "表")}</th>
          <th className="num" scope="col">{text("Loads", "次数")}</th>
          <th className="num" scope="col">{text("Failed", "失败")}</th>
          {stream ? (
            <th className="num" scope="col">{text("Received", "接收数据")}</th>
          ) : (
            <>
              <th className="num" scope="col">{text("Rows scanned", "扫描行")}</th>
              <th className="num" scope="col">{text("Data scanned", "扫描数据")}</th>
            </>
          )}
          <th className="num" scope="col">{text("Rows written", "写入行")}</th>
          <th className="num" scope="col">{text("Filtered", "过滤行")}</th>
          <th className="num" scope="col">
            {stream ? text("Plan P95", "规划 P95") : text("Total P95", "总耗时 P95")}
          </th>
          {!stream ? <th scope="col">{text("Priority", "优先级")}</th> : null}
        </tr>
      </thead>
      <tbody>
        {tables.map((table) => (
          <tr key={table.table}>
            <th scope="row">
              <button
                className="row-pick-btn mono"
                data-drill={`table:${table.table}`}
                onClick={() => onOpenTable(table.table)}
                title={text(`Open the load records of ${table.table}`, `查看 ${table.table} 的导入记录`)}
                type="button"
              >
                {table.table}
              </button>
              {table.counts_unknown !== "0" ? (
                <span className="load-sub load-warn-text">
                  {text(`${table.counts_unknown} with unknown counts`, `${table.counts_unknown} 条计数未知`)}
                </span>
              ) : null}
            </th>
            <td className="num">{count(table.loads)}</td>
            <td className={`num${table.failed !== "0" ? " load-num-critical" : ""}`}>
              {count(table.failed)}
            </td>
            {stream ? (
              <td className="num">{bytes(table.scan_bytes)}</td>
            ) : (
              <>
                <td className="num">{count(table.scan_rows)}</td>
                <td className="num">{bytes(table.scan_bytes)}</td>
              </>
            )}
            <td className="num">{count(table.sink_rows)}</td>
            <td className="num">{count(table.filtered_rows)}</td>
            <td className="num">
              {stream ? ms(table.plan_p95_ms) : seconds(table.total_p95_seconds)}
            </td>
            {!stream ? <td>{table.priority ?? "—"}</td> : null}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
