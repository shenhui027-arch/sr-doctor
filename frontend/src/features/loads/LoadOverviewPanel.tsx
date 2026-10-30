// Design board I4 (task 52): the load overview. One server summary of the
// records and Routine Load jobs StarRocks holds now; every count, bucket
// height and bar length arrives computed, and this only lays them out.
import { useEffect, useState } from "react";
import type { JSX, KeyboardEvent } from "react";
import type { LoadHourBucket, LoadKindFilter, LoadOverviewResponse, RoutineLoadStateClass } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { ago, compact, count, feTime, hourRangeText, hourText, ms, phaseBinText, shareText } from "./load-text";

export type OverviewFilters = { kind: LoadKindFilter | null; include_system: boolean };

const bar = (permyriad: number): string => `${permyriad / 100}%`;

function lastFilled(hours: readonly LoadHourBucket[]): number {
  for (let index = hours.length - 1; index >= 0; index -= 1) if (hours[index].loads !== "0") return index;
  return hours.length - 1;
}

function lagTone(state: RoutineLoadStateClass): string {
  return state === "paused" ? "bad" : state === "scheduling" ? "warn" : "ok";
}

function lagStateText(state: RoutineLoadStateClass, text: LocalizedText): string {
  return state === "paused" ? text("Paused", "已暂停") : state === "scheduling" ? text("Scheduling", "等待调度") : state === "running" ? text("Running", "运行中") : text("Unknown", "未知");
}

export function LoadOverviewPanel({ overview, nowMs, onOpenHour, onOpenTable, onOpenJob, text }: {
  overview: LoadOverviewResponse;
  /** Browser time, only to say how long ago a message was consumed. */
  nowMs: number;
  onOpenHour(hour: string): void;
  onOpenTable(table: string): void;
  onOpenJob(jobId: string): void;
  text: LocalizedText;
}): JSX.Element {
  const records = overview.support !== "unsupported_version";
  return <div className="history-page load-page">
    <OverviewCards overview={overview} text={text} />
    {records ? <div className="history-row">
      <HourChart onOpenHour={onOpenHour} overview={overview} text={text} />
      <FilteredTables onOpenTable={onOpenTable} overview={overview} text={text} />
    </div> : <p className="history-banner">{text("Load records need StarRocks 3.3 or later; only Routine Load is shown.", "导入记录需要 StarRocks 3.3 及以上；这里只显示 Routine Load。")}</p>}
    <div className="history-row">
      <LagChart nowMs={nowMs} onOpenJob={onOpenJob} overview={overview} text={text} />
      {records && <PhaseChart overview={overview} text={text} />}
    </div>
    <p className="history-banner neutral load-overview-foot" role="note">
      <span>
        {text("StarRocks keeps only the last 3 days and at most about 1000 load records per kind, and clears finished Stream Loads all at once past 1000; ", "StarRocks 只保留最近 3 天、每类最多约 1000 条导入记录，Stream Load 超过 1000 条时会一次清空已结束的记录；")}
        {overview.at_retention_limit ? text("the limit is reached, older ones are gone.", "当前已到上限，更早的已不可查。") : text("older ones are gone.", "更早的已不可查。")}
        {overview.hidden_system !== "0" && text(` ${overview.hidden_system} system loads hidden.`, `另有 ${overview.hidden_system} 次系统任务已隐藏。`)}
      </span>
      <span className="planned">Planned · {text("trends beyond 3 days and load alerts (L1/L2)", "超过 3 天的趋势与导入告警（L1/L2）")}</span>
    </p>
  </div>;
}

function OverviewCards({ overview, text }: { overview: LoadOverviewResponse; text: LocalizedText }): JSX.Element {
  const records = overview.support !== "unsupported_version";
  const summary = overview.summary;
  const routine = overview.routine;
  const failedKinds = overview.by_kind.filter((kind) => kind.failed !== "0").map((kind) => `${kind.kind} ${count(kind.failed)}`).join(" · ");
  const worst = routine.lags[0] ?? null;
  const failedShare = shareText(overview.failed_share_permyriad);
  const filteredShare = summary.filtered_rows !== "0" ? shareText(summary.filtered_share_permyriad) : null;
  const unknownRows = summary.filtered_rows_unknown !== "0" || summary.loaded_rows_unknown !== "0";
  const cards: Array<{ label: string; value: string; unit?: string | null; note: string; bad?: boolean }> = [
    { label: text("Loads", "导入次数"), value: records ? count(summary.loads) : "—", note: !records ? text("needs 3.3+", "需要 3.3 及以上") : overview.earliest_create_time ? `${overview.at_retention_limit ? text("at the limit · ", "已到上限 · ") : ""}${text("oldest", "最早")} ${feTime(overview.earliest_create_time)}` : text("no record held", "没有保留的记录") },
    { label: text("Failed / cancelled", "失败 / 取消"), value: records ? count(summary.failed) : "—", unit: failedShare, note: failedKinds ? `${text("of which", "其中")} ${failedKinds}` : text("state = CANCELLED", "state = CANCELLED"), bad: records && summary.failed !== "0" },
    { label: text("Filtered rows", "过滤行"), value: records ? count(summary.filtered_rows) : "—", unit: filteredShare, note: !records ? "—" : unknownRows ? text(`${summary.filtered_rows_unknown} filtered / ${summary.loaded_rows_unknown} loaded unknown, left out`, `过滤 ${summary.filtered_rows_unknown} / 导入 ${summary.loaded_rows_unknown} 条未知，未计入`) : `${text("loaded", "导入行")} ${compact(summary.loaded_rows)}` },
    { label: text("Begin txn + plan, P95", "取事务 + 规划 P95"), value: summary.begin_plan_p95_ms !== null ? ms(summary.begin_plan_p95_ms) : "—", note: summary.begin_plan_p95_ms !== null ? text(`${overview.phases.begin_txn.samples} records with phases`, `${overview.phases.begin_txn.samples} 条带阶段耗时`) : text("fewer than 20 samples", "样本不足 20 条") },
    { label: "Routine Load", value: routine.support === "unsupported_version" ? "—" : `${routine.running} / ${routine.paused}`, unit: routine.support === "unsupported_version" ? null : text("running / paused", "运行 / 暂停"), note: routine.support === "unsupported_version" ? text("needs 3.2+", "需要 3.2 及以上") : text(`${routine.scheduling} scheduling · ${routine.ended} ended`, `等待调度 ${routine.scheduling} · 已结束 ${routine.ended}`), bad: routine.paused !== "0" },
    { label: text("Largest partition lag", "最大分区积压"), value: routine.support === "without_lag" ? text("unavailable", "不可用") : worst ? count(worst.lag) : "—", note: routine.support === "without_lag" ? text("3.2 has no OFFSET_LAG", "3.2 没有 OFFSET_LAG") : worst ? `${worst.job_name} · p${worst.partition}` : text("no live job", "没有在运行的作业"), bad: worst?.state_class === "paused" },
  ];
  return <section aria-label={text("Load summary", "导入汇总")} className="load-kpis">
    {cards.map((card) => <div className={`history-card${card.bad ? " load-card-bad" : ""}`} key={card.label}>
      <span className="history-muted">{card.label}</span>
      <strong className={card.bad ? "bad" : undefined} title={card.value}>{card.value}{card.unit ? <small> {card.unit}</small> : null}</strong>
      <span className="history-muted load-kpi-note" title={card.note}>{card.note}</span>
    </div>)}
  </section>;
}

function HourChart({ overview, onOpenHour, text }: { overview: LoadOverviewResponse; onOpenHour(hour: string): void; text: LocalizedText }): JSX.Element {
  const hours = overview.hours;
  const fallback = lastFilled(hours);
  const [selected, setSelected] = useState(fallback);
  // A refresh may move the window; keep the same hour when it is still drawn.
  const [pickedHour, setPickedHour] = useState<string | null>(null);
  useEffect(() => {
    const index = pickedHour === null ? -1 : hours.findIndex((hour) => hour.hour === pickedHour);
    setSelected(index >= 0 ? index : fallback);
  }, [hours, pickedHour, fallback]);
  const pick = (index: number) => { setSelected(index); setPickedHour(hours[index]?.hour ?? null); };
  const current = hours[selected] ?? null;
  const firstKnown = hours.findIndex((hour) => hour.coverage !== "unknown");
  const anyPartial = hours.some((hour) => hour.coverage === "partial");
  const zone = overview.fe_time_zone ?? text("FE time zone", "FE 时区");
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0;
    if (step === 0) return;
    event.preventDefault();
    let next = selected + step;
    while (hours[next]?.coverage === "unknown") next += step;
    if (next < 0 || next >= hours.length) return;
    pick(next);
    event.currentTarget.querySelector<HTMLButtonElement>(`button[data-hour="${hours[next].hour}"]`)?.focus();
  };
  const labels = hours.length > 0 ? [0, Math.floor(hours.length / 3), Math.floor((2 * hours.length) / 3), hours.length - 1] : [];
  return <section aria-labelledby="load-hours" className="history-card wide">
    <div className="history-card-head">
      <h2 id="load-hours">{text("Loads and failures", "导入次数与失败")} <span className="history-muted">{text(`hourly · by creation time (${zone}, second precision)`, `每小时 · 按创建时间（${zone}，秒级）`)}</span></h2>
      <span className="load-legend">
        <span><i className="ok" aria-hidden="true" />{text("Succeeded or running", "成功 / 进行中")}</span>
        <span><i className="bad" aria-hidden="true" />{text("Failed / cancelled", "失败 / 取消")}</span>
        {anyPartial && <span><i className="partial" aria-hidden="true" />{text("Possibly incomplete", "可能不完整")}</span>}
        {firstKnown > 0 && <span><i className="gone" aria-hidden="true" />{text("Before the oldest record", "早于最早一条（不可查）")}</span>}
      </span>
    </div>
    {hours.length === 0 ? <p className="notify-empty">{text("StarRocks holds no load record right now.", "StarRocks 当前没有保留任何导入记录。")}</p> : <>
      <div className="load-hour-chart">
        <div aria-hidden="true" className="load-axis">{overview.hour_axis_ticks.map((tick, index) => <span key={index}>{count(tick)}</span>)}</div>
        <div aria-label={text("Hourly loads; use the arrow keys to move between hours", "每小时导入次数；可用左右方向键切换小时")} className="load-hour-plot" onKeyDown={onKeyDown} role="group">
          {hours.map((hour, index) => hour.coverage === "unknown"
            ? <span aria-hidden="true" className="load-hour gone" key={hour.hour} title={text(`${hourText(hour.hour)} · before the oldest record`, `${hourText(hour.hour)} · 早于最早一条，StarRocks 已不可查`)} />
            : <button aria-label={text(`${hourRangeText(hour.hour)}: ${hour.loads} loads, ${hour.failed} failed${hour.coverage === "partial" ? ", possibly incomplete" : ""}`, `${hourRangeText(hour.hour)}：导入 ${hour.loads}，失败 ${hour.failed}${hour.coverage === "partial" ? "，可能不完整" : ""}`)} aria-pressed={index === selected} className={`load-hour${hour.coverage === "partial" ? " partial" : ""}`} data-hour={hour.hour} key={hour.hour} onClick={() => pick(index)} tabIndex={index === selected ? 0 : -1} title={`${hourRangeText(hour.hour)} · ${text(`${hour.loads} loads · ${hour.failed} failed`, `导入 ${hour.loads} · 失败 ${hour.failed}`)}`} type="button">
              <span className="fail" style={{ height: bar(hour.failed_permyriad) }} />
              <span className="okay" style={{ height: bar(hour.loads_permyriad - hour.failed_permyriad) }} />
            </button>)}
          {firstKnown > 0 && <span aria-hidden="true" className="load-unknown-zone" style={{ width: `${(firstKnown / hours.length) * 100}%` }}>{firstKnown / hours.length > 0.12 && <span>{text("Unknown: before the oldest record StarRocks holds", "未知：早于 StarRocks 保留的最早一条记录")}</span>}</span>}
          {firstKnown > 0 && <span aria-hidden="true" className={`load-earliest${firstKnown / hours.length > 0.6 ? " flip" : ""}`} style={{ left: `${(firstKnown / hours.length) * 100}%` }}>{text("oldest", "最早一条")} {feTime(overview.earliest_create_time)}</span>}
        </div>
        <span />
        <div aria-hidden="true" className="load-hour-labels">{labels.map((index) => <span key={index}>{hourText(hours[index].hour)}</span>)}</div>
      </div>
      {anyPartial && overview.complete_from && <p className="history-muted">{text(`Striped hours may be incomplete: StarRocks may have dropped some kinds' records before ${feTime(overview.complete_from)}, so their bars show only what is still held.`, `斜纹柱是可能不完整的小时：某类导入在 ${feTime(overview.complete_from)} 之前的记录可能已被 StarRocks 清理，柱高只是仍保留的部分。`)}</p>}
      {overview.records_outside_hours !== "0" && <p className="history-muted">{text(`${overview.records_outside_hours} records have no readable creation time or are older than the drawn hours; they count in the cards only.`, `另有 ${overview.records_outside_hours} 条记录没有可读的创建时间或早于图中的小时，只计入上方指标卡。`)}</p>}
      {current && current.coverage !== "unknown" && <div className="load-hour-detail">
        <button aria-label={text("Previous hour", "上一小时")} className="load-hour-step" disabled={selected <= firstKnown} onClick={() => pick(selected - 1)} type="button">‹</button>
        <strong role="status">{hourRangeText(current.hour)}</strong>
        <button aria-label={text("Next hour", "下一小时")} className="load-hour-step" disabled={selected >= hours.length - 1} onClick={() => pick(selected + 1)} type="button">›</button>
        {current.coverage === "partial" && <span className="load-partial-note">{text("possibly incomplete", "可能不完整")}</span>}
        <span>{text("loads", "导入")} <span className="mono">{count(current.loads)}</span></span>
        <span className={current.failed !== "0" ? "bad" : undefined}>{text("failed", "失败")} <span className="mono">{count(current.failed)}</span></span>
        <span>{text("filtered rows", "过滤行")} <span className="mono">{count(current.filtered_rows)}</span>{current.filtered_unknown !== "0" && text(` (+${current.filtered_unknown} unknown)`, `（另 ${current.filtered_unknown} 条未知）`)}</span>
        {current.top_failed_table && <span>{text("most failures", "失败最多")} <span className="mono">{current.top_failed_table}</span>（{current.top_failed_count}）</span>}
        <button className="button secondary" data-drill="hour" disabled={current.loads === "0"} onClick={() => onOpenHour(current.hour)} type="button">{text("Records of this hour", "查看这一小时的导入记录")}</button>
      </div>}
    </>}
  </section>;
}

function FilteredTables({ overview, onOpenTable, text }: { overview: LoadOverviewResponse; onOpenTable(table: string): void; text: LocalizedText }): JSX.Element {
  return <section aria-labelledby="load-filtered" className="history-card load-side">
    <h2 id="load-filtered">{text("Tables with the most filtered rows", "过滤行最多的表")} <span className="history-muted">Top {overview.tables.length || 8}</span></h2>
    {overview.tables.length === 0 ? <p className="notify-empty">{text("No filtered row in the records StarRocks holds.", "StarRocks 保留的记录里没有被过滤的行。")}</p>
      : <ol className="load-bars">
        {overview.tables.map((table) => <li key={table.table}>
          <button className="load-bar-row" data-drill={`table:${table.table}`} onClick={() => onOpenTable(table.table)} title={text(`Open the load records of ${table.table}`, `查看 ${table.table} 的导入记录`)} type="button">
            <span className="mono load-bar-name">{table.table}</span>
            <span className="mono">{text(`${count(table.filtered_rows)} rows`, `${count(table.filtered_rows)} 行`)}</span>
            <span aria-hidden="true" className="load-bar-track"><span className="filtered" style={{ width: bar(table.width_permyriad) }} /></span>
            <span className="history-muted">{text(`${table.loads} loads · ${table.failed} failed`, `${table.loads} 次导入 · 失败 ${table.failed}`)}{table.filtered_unknown !== "0" && text(` · ${table.filtered_unknown} unknown`, ` · ${table.filtered_unknown} 条过滤行未知`)}</span>
          </button>
        </li>)}
      </ol>}
    <p className="history-muted">{text("Click a table for its load records. Filtered rows sum FILTERED_ROWS; bars are log-scaled.", "点表名进入导入记录并按该表筛选。过滤行 = FILTERED_ROWS 之和；条长按对数刻度。")}</p>
  </section>;
}

function LagChart({ overview, nowMs, onOpenJob, text }: { overview: LoadOverviewResponse; nowMs: number; onOpenJob(jobId: string): void; text: LocalizedText }): JSX.Element {
  const routine = overview.routine;
  return <section aria-labelledby="load-lags" className="history-card wide">
    <div className="history-card-head">
      <h2 id="load-lags">{text("Routine Load partition lag", "Routine Load 分区积压")} <span className="history-muted">{text("Top 10 · current", "Top 10 · 当前值")}</span></h2>
      <span className="load-legend">
        <span><i className="run" aria-hidden="true" />{text("Running", "运行中")}</span>
        <span><i className="bad" aria-hidden="true" />{text("Paused", "已暂停")}</span>
        <span><i className="warn" aria-hidden="true" />{text("Scheduling", "等待调度")}</span>
      </span>
    </div>
    {routine.support === "unsupported_version" ? <p className="notify-empty">{text("Routine Load jobs need StarRocks 3.2 or later.", "Routine Load 作业需要 StarRocks 3.2 及以上。")}</p>
      : routine.support === "without_lag" ? <p className="notify-empty">{text("StarRocks 3.2 has no OFFSET_LAG, so lag is unavailable.", "StarRocks 3.2 没有 OFFSET_LAG，积压不可用。")}</p>
        : routine.lags.length === 0 ? <p className="notify-empty">{text("No live Routine Load job reports partition lag.", "没有在运行的 Routine Load 作业报告分区积压。")}</p>
          : <ol className="load-bars load-lags">
            {routine.lags.map((lag) => <li key={`${lag.job_id}:${lag.partition}`}>
              <button aria-label={text(`${lag.job_name} partition ${lag.partition}, ${lagStateText(lag.state_class, text)}, lag ${lag.lag}, consumed message ${ago(lag.consumed_message_ms, nowMs, text)}; open the job`, `${lag.job_name} 分区 ${lag.partition}，${lagStateText(lag.state_class, text)}，积压 ${lag.lag}，已消费消息 ${ago(lag.consumed_message_ms, nowMs, text)}；打开作业`)} className="load-bar-row" data-drill={`lag:${lag.job_id}:${lag.partition}`} onClick={() => onOpenJob(lag.job_id)} title={text(`Open ${lag.job_name}`, `打开作业 ${lag.job_name}`)} type="button">
                <span className="mono load-bar-name">{lag.job_name} · p{lag.partition}</span>
                <span className={`load-state ${lagTone(lag.state_class)}`}>{lagStateText(lag.state_class, text)}</span>
                <span aria-hidden="true" className="load-bar-track"><span className={lagTone(lag.state_class)} style={{ width: bar(lag.width_permyriad) }} /></span>
                <span className="mono num">{count(lag.lag)}</span>
                <span className="history-muted num">{ago(lag.consumed_message_ms, nowMs, text)}</span>
              </button>
            </li>)}
          </ol>}
    <p className="history-muted">{text("Lag is the latest position minus the consumed one (OFFSET_LAG); the right column is how long ago the consumed message was written. Bars are log-scaled. Click a row for the job.", "积压 = 最新位置 − 已消费位置（OFFSET_LAG），右列是已消费消息的时间距今。条长按对数刻度，便于同时看清几十与几十万。点一行打开该作业。")}</p>
  </section>;
}

function PhaseChart({ overview, text }: { overview: LoadOverviewResponse; text: LocalizedText }): JSX.Element {
  const phases = overview.phases;
  const quantiles = (q: LoadOverviewResponse["phases"]["plan"]) => q.p95_ms !== null ? `P50 ${ms(q.p50_ms)} · P95 ${ms(q.p95_ms)}` : text("fewer than 20 samples", "样本不足 20 条");
  const empty = phases.begin_txn.samples === "0" && phases.plan.samples === "0";
  return <section aria-labelledby="load-phases" className="history-card wide">
    <div className="history-card-head">
      <h2 id="load-phases">{text("Begin txn / plan time", "取事务 / 规划耗时分布")} <span className="history-muted">{text(`${phases.begin_txn.samples} records with phases`, `${phases.begin_txn.samples} 条带阶段耗时的导入`)}</span></h2>
      <span className="load-legend">
        <span><i className="run" aria-hidden="true" />{text("Begin txn", "取事务")} {quantiles(phases.begin_txn)}</span>
        <span><i className="warn" aria-hidden="true" />{text("Plan", "规划")} {quantiles(phases.plan)}</span>
      </span>
    </div>
    {empty ? <p className="notify-empty">{text("No record reports phase times (RUNTIME_DETAILS).", "没有记录报告阶段耗时（RUNTIME_DETAILS）。")}</p> : <div className="load-hour-chart">
      <div aria-hidden="true" className="load-axis short">{phases.axis_ticks.map((tick, index) => <span key={index}>{count(tick)}</span>)}</div>
      <ul aria-label={text("Records per time bin", "各耗时区间的记录数")} className="load-phase-plot">
        {phases.bins.map((bin) => {
          const label = phaseBinText(bin.lower_ms, bin.upper_ms);
          return <li aria-label={text(`${label}: begin txn ${bin.begin_txn}, plan ${bin.plan}`, `${label}：取事务 ${bin.begin_txn}，规划 ${bin.plan}`)} key={bin.lower_ms} title={text(`${label} · begin txn ${bin.begin_txn} · plan ${bin.plan}`, `${label} · 取事务 ${bin.begin_txn} · 规划 ${bin.plan}`)}>
            <span className="run" style={{ height: bar(bin.begin_txn_permyriad) }} />
            <span className="warn" style={{ height: bar(bin.plan_permyriad) }} />
          </li>;
        })}
      </ul>
      <span />
      <div aria-hidden="true" className="load-phase-labels">{phases.bins.map((bin) => <span key={bin.lower_ms}>{phaseBinText(bin.lower_ms, bin.upper_ms)}</span>)}</div>
    </div>}
    {!empty && <details className="load-phase-data">
      <summary>{text("Show the numbers", "查看分布数据")}</summary>
      <table className="history-table">
        <thead><tr><th>{text("Time", "区间")}</th><th className="num">{text("Begin txn", "取事务")}</th><th className="num">{text("Plan", "规划")}</th></tr></thead>
        <tbody>{phases.bins.map((bin) => <tr key={bin.lower_ms}><td className="mono">{phaseBinText(bin.lower_ms, bin.upper_ms)}</td><td className="num">{count(bin.begin_txn)}</td><td className="num">{count(bin.plan)}</td></tr>)}</tbody>
      </table>
    </details>}
    <p className="history-muted">{text("Milliseconds from RUNTIME_DETAILS; records without it are left out. A high plan P95 usually means a busy FE or many tablets. Receive time and total time measure differently and stay out of this chart.", "来自 RUNTIME_DETAILS 的毫秒值；没有该字段的导入不计入。规划 P95 偏高通常意味着 FE 繁忙或表的 tablet 很多。接收数据与总耗时口径不同，不放在这张图里。")}</p>
  </section>;
}
