// Design boards I4-I8 (tasks 52, 56): the load overview. One server summary of
// the records and Routine Load jobs StarRocks holds now, split by load type;
// every count, bucket height and bar length arrives computed, and this only
// lays them out.
import { useEffect, useState } from "react";
import type { JSX, KeyboardEvent } from "react";
import type { LoadHourBucket, LoadKindFilter, LoadKindGroup, LoadOverviewResponse, RoutineLoadStateClass } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { BatchView, RoutineView, StreamView } from "./LoadTypeViews";
import { ago, bytes, compact, count, feTime, groupText, hourRangeText, hourText, ms, phaseBinText, seconds, shareText } from "./load-text";

export type OverviewFilters = { kind: LoadKindFilter | null; include_system: boolean };

export const bar = (permyriad: number): string => `${permyriad / 100}%`;

function lastFilled(hours: readonly LoadHourBucket[]): number {
  for (let index = hours.length - 1; index >= 0; index -= 1) if (hours[index].loads !== "0") return index;
  return hours.length - 1;
}

function lagTone(state: RoutineLoadStateClass): string {
  return state === "paused" ? "bad" : state === "scheduling" ? "warn" : "ok";
}

export function lagStateText(state: RoutineLoadStateClass, text: LocalizedText): string {
  return state === "paused" ? text("Paused", "已暂停") : state === "scheduling" ? text("Scheduling", "等待调度") : state === "running" ? text("Running", "运行中") : state === "stopped" ? text("Stopped", "已停止") : state === "cancelled" ? text("Cancelled", "已取消") : text("Unknown", "未知");
}

export type OverviewActions = {
  /** Browser time, only to say how long ago a message was consumed. */
  nowMs: number;
  onOpenHour(hour: string): void;
  onOpenTable(table: string): void;
  onOpenJob(jobId: string): void;
  text: LocalizedText;
};

export function LoadOverviewPanel({ overview, kind, ...actions }: OverviewActions & { overview: LoadOverviewResponse; kind: LoadKindFilter | null }): JSX.Element {
  const { text } = actions;
  const records = overview.support !== "unsupported_version";
  const body = overview.stream ? <StreamView detail={overview.stream} overview={overview} {...actions} />
    : overview.batch ? <BatchView broker={kind === "broker"} detail={overview.batch} overview={overview} {...actions} />
      : overview.routine_detail ? <RoutineView detail={overview.routine_detail} overview={overview} {...actions} />
        : <>
          <AllCards overview={overview} text={text} />
          {records ? <HourChart mode="groups" onOpenHour={actions.onOpenHour} overview={overview} text={text} />
            : <p className="history-banner">{overview.routine.support === "unsupported_version"
              ? text("Load records need StarRocks 3.3 or later and Routine Load jobs 3.2 or later; neither is available on this release.", "导入记录需要 StarRocks 3.3 及以上，Routine Load 作业需要 3.2 及以上；当前版本都不可用。")
              : text("Load records need StarRocks 3.3 or later; only Routine Load is shown.", "导入记录需要 StarRocks 3.3 及以上；这里只显示 Routine Load。")}</p>}
          {records && <div className="history-row">
            <GroupCompare overview={overview} text={text} />
            <FilteredTables onOpenTable={actions.onOpenTable} overview={overview} text={text} />
          </div>}
          {!records && <LagChart nowMs={actions.nowMs} onOpenJob={actions.onOpenJob} overview={overview} text={text} />}
        </>;
  return <div className="history-page load-page">
    {body}
    <p className="history-banner neutral load-overview-foot" role="note">
      <span>
        {text("StarRocks keeps only the last 3 days and at most about 1000 load records per kind, and clears finished Stream Loads and Routine Load tasks all at once past 1000; ", "StarRocks 只在 FE 内存里保留最近 3 天、每类最多约 1000 条导入记录，Stream Load 与 Routine Load 任务超过 1000 条时会一次清空已结束的记录；")}
        {overview.at_retention_limit ? text("the limit is reached, older ones are gone.", "当前已到上限，更早的已不可查。") : text("older ones are gone.", "更早的已不可查。")}
        {overview.hidden_system !== "0" && text(` ${overview.hidden_system} system loads hidden.`, `另有 ${overview.hidden_system} 次系统任务已隐藏。`)}
      </span>
      <span className="planned">Planned · {text("trends beyond 3 days and load alerts (L1/L2)", "超过 3 天的趋势与导入告警（L1/L2）")}</span>
    </p>
  </div>;
}

/** `caveat` says what the value leaves out; unlike `note` it is never cut. */
export type KpiCard = { label: string; value: string; unit?: string | null; note: string; caveat?: string | null; bad?: boolean };

export function KpiCards({ cards, label }: { cards: KpiCard[]; label: string }): JSX.Element {
  return <section aria-label={label} className={`load-kpis${cards.length === 5 ? " five" : ""}`}>
    {cards.map((card) => <div className={`history-card${card.bad ? " load-card-bad" : ""}`} key={card.label}>
      <span className="history-muted">{card.label}</span>
      <strong className={card.bad ? "bad" : undefined} title={card.value}>{card.value}{card.unit ? <small> {card.unit}</small> : null}</strong>
      <span className="history-muted load-kpi-note" title={card.note}>{card.note}</span>
      {card.caveat && <span className="load-kpi-caveat">{card.caveat}</span>}
    </div>)}
  </section>;
}

/** The load count card: how many and since when. */
export function loadsCard(overview: LoadOverviewResponse, text: LocalizedText, label = text("Loads", "导入次数")): KpiCard {
  const records = overview.support !== "unsupported_version";
  return { label, value: records ? count(overview.summary.loads) : "—", unit: records ? text("loads", "次") : null, note: !records ? text("needs 3.3+", "需要 3.3 及以上") : overview.earliest_create_time ? `${overview.at_retention_limit ? text("at the limit · ", "已到上限 · ") : ""}${text("oldest", "最早")} ${feTime(overview.earliest_create_time)}` : text("no record held", "没有保留的记录") };
}

export function failedCard(overview: LoadOverviewResponse, note: string, text: LocalizedText): KpiCard {
  const records = overview.support !== "unsupported_version";
  return { label: text("Failed / cancelled", "失败 / 取消"), value: records ? count(overview.summary.failed) : "—", unit: shareText(overview.failed_share_permyriad), note, bad: records && overview.summary.failed !== "0" };
}

export function filteredCard(overview: LoadOverviewResponse, note: string, text: LocalizedText, caveat: string | null = null): KpiCard {
  const summary = overview.summary;
  const unknownRows = summary.filtered_rows_unknown !== "0" || summary.loaded_rows_unknown !== "0";
  return { label: text("Filtered rows", "过滤行"), value: overview.support !== "unsupported_version" ? count(summary.filtered_rows) : "—", unit: summary.filtered_rows !== "0" ? shareText(summary.filtered_share_permyriad) : null, note, caveat: [unknownRows ? text(`${summary.filtered_rows_unknown} filtered / ${summary.loaded_rows_unknown} loaded unknown, left out`, `过滤 ${summary.filtered_rows_unknown} / 导入 ${summary.loaded_rows_unknown} 条未知，未计入`) : null, caveat].filter(Boolean).join(text("; ", "；")) || null };
}

export function routineJobsCard(overview: LoadOverviewResponse, text: LocalizedText): KpiCard {
  const routine = overview.routine;
  const unsupported = routine.support === "unsupported_version";
  return { label: text("Routine Load jobs", "Routine Load 作业"), value: unsupported ? "—" : `${routine.running} / ${routine.paused}`, unit: unsupported ? null : text("running / paused", "运行 / 暂停"), note: unsupported ? text("needs 3.2+", "需要 3.2 及以上") : text(`${routine.scheduling} scheduling · ${routine.ended} ended`, `等待调度 ${routine.scheduling} · 已结束 ${routine.ended}`), bad: routine.paused !== "0" };
}

export function lagCard(overview: LoadOverviewResponse, text: LocalizedText): KpiCard {
  const routine = overview.routine;
  const worst = routine.lags[0] ?? null;
  return { label: text("Largest partition lag", "最大分区积压"), value: routine.support === "without_lag" ? text("unavailable", "不可用") : worst ? count(worst.lag) : "—", unit: worst && routine.support !== "without_lag" ? text("messages", "条") : null, note: routine.support === "without_lag" ? text("3.2 has no OFFSET_LAG", "3.2 没有 OFFSET_LAG") : worst ? `${worst.job_name} · p${worst.partition} · ${lagStateText(worst.state_class, text)}` : routine.jobs === "0" ? text("no Routine Load job", "没有 Routine Load 作业") : text("no partition lag reported now", "当前没有可用的分区积压数据"), bad: worst?.state_class === "paused" };
}

function AllCards({ overview, text }: { overview: LoadOverviewResponse; text: LocalizedText }): JSX.Element {
  const failedGroups = overview.groups.filter((group) => group.failed !== "0").map((group) => `${groupText(group.group, text)} ${count(group.failed)}`).join(" · ");
  return <KpiCards cards={[
    loadsCard(overview, text),
    failedCard(overview, failedGroups ? `${text("of which", "其中")} ${failedGroups}` : text("state = CANCELLED", "state = CANCELLED"), text),
    filteredCard(overview, `${text("loaded", "导入行")} ${compact(overview.summary.loaded_rows)}`, text),
    routineJobsCard(overview, text),
    lagCard(overview, text),
  ]} label={text("Load summary", "导入汇总")} />;
}

/** Group colors: distinct in lightness as well as hue, each at least 3:1 on white. */
const GROUP_ORDER: LoadKindGroup[] = ["stream_load", "insert", "routine_load", "broker", "other"];

function GroupCompare({ overview, text }: { overview: LoadOverviewResponse; text: LocalizedText }): JSX.Element {
  const basis: Record<LoadKindGroup, string> = {
    stream_load: text("seconds; millisecond phases in Stream Load", "秒级；另有毫秒级取事务 / 规划"),
    insert: text("seconds: execute + publish", "秒级：执行 + 发布"),
    routine_load: text("seconds; job totals under Routine Load", "秒级；作业累计指标见 Routine Load"),
    broker: text("seconds", "秒级"),
    other: text("seconds", "秒级"),
  };
  return <section aria-labelledby="load-groups" className="history-card wide">
    <h2 id="load-groups">{text("By type", "按类型对比")} <span className="history-muted">{text("records StarRocks holds now", "StarRocks 当前保留的记录")}</span></h2>
    {overview.groups.length === 0 ? <p className="notify-empty">{text("StarRocks holds no load record right now.", "StarRocks 当前没有保留任何导入记录。")}</p>
      : <div className="history-table-wrap"><table className="history-table load-table">
        <thead><tr>
          <th scope="col">{text("Type", "类型")}</th><th className="num" scope="col">{text("Loads", "次数")}</th><th className="num" scope="col">{text("Failed / cancelled", "失败 / 取消")}</th>
          <th className="num" scope="col">{text("Rows written", "写入行")}</th><th className="num" scope="col">{text("Filtered rows", "过滤行")}</th><th className="num" scope="col">{text("Total time P95", "总耗时 P95")}</th><th scope="col">{text("Timing", "耗时口径")}</th>
        </tr></thead>
        <tbody>{overview.groups.map((group) => <tr key={group.group}>
          <th scope="row"><span className={`load-group-dot ${group.group}`} aria-hidden="true" />{groupText(group.group, text)}</th>
          <td className="num">{count(group.loads)}</td>
          <td className={`num${group.failed !== "0" ? " bad" : ""}`}>{count(group.failed)}</td>
          <td className="num">{count(group.sink_rows)}{group.sink_rows_unknown !== "0" && <span className="load-sub">{text(`+${group.sink_rows_unknown} unknown`, `另 ${group.sink_rows_unknown} 条未知`)}</span>}</td>
          <td className="num">{count(group.filtered_rows)}{group.filtered_unknown !== "0" && <span className="load-sub">{text(`+${group.filtered_unknown} unknown`, `另 ${group.filtered_unknown} 条未知`)}</span>}</td>
          <td className="num">{group.total_p95_seconds !== null ? seconds(group.total_p95_seconds) : <span className="history-muted" title={text("fewer than 20 finished", "已结束的不足 20 条")}>—</span>}</td>
          <td className="history-muted">{basis[group.group]}</td>
        </tr>)}</tbody>
      </table></div>}
    <p className="history-muted">{text("Total time is finish minus creation; both have second precision, so loads under a second count as 0 s. Only Stream Load also has millisecond phases. A Routine Load task is one batch the FE commits for a job.", "总耗时 = 完成时间 − 创建时间，两者都只到秒，不足 1 秒记为 0 秒；只有 Stream Load 另有毫秒级阶段耗时。Routine Load 任务是 FE 为作业提交的每个批次。")}</p>
  </section>;
}

export function HourChart({ overview, onOpenHour, text, mode, unitLabel }: {
  overview: LoadOverviewResponse;
  onOpenHour(hour: string): void;
  text: LocalizedText;
  /** Stacked by type, or split into succeeded and failed with the volume line. */
  mode: "groups" | "outcome";
  /** What a bar counts, when not loads (Routine Load tasks). */
  unitLabel?: string;
}): JSX.Element {
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
  const groups = mode === "groups";
  const showVolume = !groups;
  const isBytes = overview.volume_unit === "bytes";
  const volumeText = (value: string | null) => (value === null ? text("unknown", "未知") : isBytes ? bytes(value) : text(`${compact(value)} rows`, `${compact(value)} 行`));
  const volumeWithUnknown = (hour: LoadHourBucket) => `${volumeText(hour.volume)}${hour.volume !== null && hour.volume_unknown !== "0" ? text(` (+${hour.volume_unknown} unknown)`, `（另 ${hour.volume_unknown} 条未知）`) : ""}`;
  const loadsWord = unitLabel ?? text("loads", "导入");
  const present = GROUP_ORDER.filter((group) => hours.some((hour) => hour.segments.some((segment) => segment.group === group)));
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
  const hourLabel = (hour: LoadHourBucket) => {
    const parts = groups
      ? hour.segments.map((segment) => `${groupText(segment.group, text)} ${segment.loads}`).join(text(", ", "，"))
      : text(`${hour.loads} ${loadsWord}, ${hour.failed} failed, ${volumeWithUnknown(hour)}`, `${loadsWord} ${hour.loads}，失败 ${hour.failed}，${volumeWithUnknown(hour)}`);
    return `${hourRangeText(hour.hour)}${text(": ", "：")}${parts || text("none", "无")}${groups && hour.failed !== "0" ? text(`, ${hour.failed} failed`, `，失败 ${hour.failed}`) : ""}${hour.coverage === "partial" ? text(", possibly incomplete", "，可能不完整") : ""}`;
  };
  const known = hours.map((hour, index) => ({ hour, index })).filter(({ hour }) => hour.coverage !== "unknown");
  // One polyline per run of hours with a known volume; unknown hours break it.
  const lines: string[][] = [];
  hours.forEach((hour, index) => {
    if (hour.coverage === "unknown" || hour.volume_permyriad === null) { if (lines.at(-1)?.length !== 0) lines.push([]); return; }
    if (lines.length === 0) lines.push([]);
    lines[lines.length - 1].push(`${((index + 0.5) / hours.length) * 100},${100 - hour.volume_permyriad / 100}`);
  });
  const labels = hours.length > 0 ? [0, Math.floor(hours.length / 3), Math.floor((2 * hours.length) / 3), hours.length - 1] : [];
  const title = groups ? text("Loads per hour, by type", "每小时导入次数 · 按类型")
    : unitLabel ? text(`${unitLabel} and rows per hour`, `每小时${unitLabel}与导入行`)
      : isBytes ? text("Loads, failures and data received per hour", "每小时次数、失败与接收数据量") : text("Loads, failures and rows written per hour", "每小时次数、失败与写入行");
  return <section aria-labelledby="load-hours" className="history-card wide">
    <div className="history-card-head">
      <h2 id="load-hours">{title} <span className="history-muted">{text(`by creation time (${zone}, second precision)`, `按创建时间（${zone}，秒级）`)}</span></h2>
      <span className="load-legend">
        {groups ? present.map((group) => <span key={group}><i className={`group ${group}`} aria-hidden="true" />{groupText(group, text)}</span>)
          : <>
            <span><i className="ok" aria-hidden="true" />{text("Succeeded or running", "成功 / 进行中")}</span>
            <span><i className="bad" aria-hidden="true" />{text("Failed / cancelled", "失败 / 取消")}</span>
            <span><i className="line" aria-hidden="true" />{isBytes ? text("Data received (right axis)", "接收数据（右轴）") : text("Rows written (right axis)", "写入行（右轴）")}</span>
          </>}
        {groups && hours.some((hour) => hour.failed !== "0") && <span><i className="fail-mark" aria-hidden="true" />{text("Hour with failures", "有失败的小时")}</span>}
        {anyPartial && <span><i className="partial" aria-hidden="true" />{text("Possibly incomplete", "可能不完整")}</span>}
        {firstKnown > 0 && <span><i className="gone" aria-hidden="true" />{text("Before the oldest record", "早于最早一条（不可查）")}</span>}
      </span>
    </div>
    {hours.length === 0 ? <p className="notify-empty">{text("StarRocks holds no load record of this type right now.", "StarRocks 当前没有保留这类导入记录。")}</p> : <>
      <div className={`load-hour-chart${showVolume ? " with-volume" : ""}`}>
        <div aria-hidden="true" className="load-axis">{overview.hour_axis_ticks.map((tick, index) => <span key={index}>{count(tick)}</span>)}</div>
        <div aria-label={text("Hourly loads; use the arrow keys to move between hours", "每小时导入次数；可用左右方向键切换小时")} className="load-hour-plot" onKeyDown={onKeyDown} role="group">
          {hours.map((hour, index) => hour.coverage === "unknown"
            ? <span aria-hidden="true" className="load-hour gone" key={hour.hour} title={text(`${hourText(hour.hour)} · before the oldest record`, `${hourText(hour.hour)} · 早于最早一条，StarRocks 已不可查`)} />
            : <button aria-label={hourLabel(hour)} aria-pressed={index === selected} className={`load-hour${hour.coverage === "partial" ? " partial" : ""}`} data-hour={hour.hour} key={hour.hour} onClick={() => pick(index)} tabIndex={index === selected ? 0 : -1} title={hourLabel(hour)} type="button">
              {groups ? <>
                <span className={`fail-mark${hour.failed !== "0" ? " on" : ""}`} />
                {[...hour.segments].reverse().map((segment) => <span className={`segment ${segment.group}`} key={segment.group} style={{ height: bar(segment.permyriad) }} />)}
              </> : <>
                <span className="fail" style={{ height: bar(hour.failed_permyriad) }} />
                <span className="okay" style={{ height: bar(hour.loads_permyriad - hour.failed_permyriad) }} />
              </>}
            </button>)}
          {showVolume && known.length > 0 && <svg aria-hidden="true" className="load-volume-line" preserveAspectRatio="none" viewBox="0 0 100 100">{lines.filter((points) => points.length > 1).map((points) => <polyline key={points[0]} points={points.join(" ")} />)}</svg>}
          {showVolume && known.map(({ hour, index }) => hour.volume_permyriad !== null && hour.volume_permyriad > 0 && <span aria-hidden="true" className="load-volume-dot" key={`v${hour.hour}`} style={{ left: `${((index + 0.5) / hours.length) * 100}%`, bottom: `${hour.volume_permyriad / 100}%` }} />)}
          {firstKnown > 0 && <span aria-hidden="true" className="load-unknown-zone" style={{ width: `${(firstKnown / hours.length) * 100}%` }}>{firstKnown / hours.length > 0.12 && <span>{text("Unknown: before the oldest record StarRocks holds", "未知：早于 StarRocks 保留的最早一条记录")}</span>}</span>}
          {firstKnown > 0 && <span aria-hidden="true" className={`load-earliest${firstKnown / hours.length > 0.6 ? " flip" : ""}`} style={{ left: `${(firstKnown / hours.length) * 100}%` }}>{text("oldest", "最早一条")} {feTime(overview.earliest_create_time)}</span>}
        </div>
        {showVolume && <div aria-hidden="true" className="load-axis right">{overview.volume_axis_ticks.map((tick, index) => <span key={index}>{isBytes ? bytes(tick) : compact(tick)}</span>)}</div>}
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
        {groups ? current.segments.map((segment) => <span key={segment.group}>{groupText(segment.group, text)} <span className="mono">{count(segment.loads)}</span></span>)
          : <span>{loadsWord} <span className="mono">{count(current.loads)}</span></span>}
        <span className={current.failed !== "0" ? "bad" : undefined}>{text("failed", "失败")} <span className="mono">{count(current.failed)}</span></span>
        {showVolume && <span>{isBytes ? text("received", "接收") : text("written", "写入")} <span className="mono">{volumeText(current.volume)}</span>{current.volume_unknown !== "0" && text(` (+${current.volume_unknown} unknown)`, `（另 ${current.volume_unknown} 条未知）`)}</span>}
        {!showVolume && <span>{text("filtered rows", "过滤行")} <span className="mono">{count(current.filtered_rows)}</span>{current.filtered_unknown !== "0" && text(` (+${current.filtered_unknown} unknown)`, `（另 ${current.filtered_unknown} 条未知）`)}</span>}
        {current.top_failed_table && <span>{text("most failures", "失败最多")} <span className="mono">{current.top_failed_table}</span>（{current.top_failed_count}）</span>}
        <button className="button secondary" data-drill="hour" disabled={current.loads === "0"} onClick={() => onOpenHour(current.hour)} type="button">{text("Records of this hour", "查看这一小时的导入记录")}</button>
      </div>}
    </>}
  </section>;
}

function FilteredTables({ overview, onOpenTable, text }: { overview: LoadOverviewResponse; onOpenTable(table: string): void; text: LocalizedText }): JSX.Element {
  return <section aria-labelledby="load-filtered" className="history-card load-side">
    <h2 id="load-filtered">{text("Tables with the most filtered rows", "过滤行最多的表")} <span className="history-muted">{text("all types · ", "所有类型 · ")}Top {overview.tables.length || 8}</span></h2>
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

export function LagChart({ overview, nowMs, onOpenJob, text }: { overview: LoadOverviewResponse; nowMs: number; onOpenJob(jobId: string): void; text: LocalizedText }): JSX.Element {
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

export function PhaseChart({ overview, text }: { overview: LoadOverviewResponse; text: LocalizedText }): JSX.Element {
  const phases = overview.phases;
  const quantiles = (q: LoadOverviewResponse["phases"]["plan"]) => q.p95_ms !== null ? `P50 ${ms(q.p50_ms)} · P95 ${ms(q.p95_ms)}` : text("fewer than 20 samples", "样本不足 20 条");
  const samples = [phases.begin_txn.samples, phases.plan.samples, phases.receive_data.samples];
  const empty = samples.every((value) => value === "0");
  const sampleText = samples.every((value) => value === samples[0]) ? text(`${samples[0]} records`, `${samples[0]} 条`) : text(`begin txn ${samples[0]} · plan ${samples[1]} · receive ${samples[2]} records`, `取事务 ${samples[0]} · 规划 ${samples[1]} · 接收 ${samples[2]} 条`);
  return <section aria-labelledby="load-phases" className="history-card wide">
    <div className="history-card-head">
      <h2 id="load-phases">{text("Phase times", "阶段耗时分布")} <span className="history-muted">{text("milliseconds", "毫秒")} · {sampleText}</span></h2>
      <span className="load-legend">
        <span><i className="run" aria-hidden="true" />{text("Begin txn", "取事务")} {quantiles(phases.begin_txn)}</span>
        <span><i className="warn" aria-hidden="true" />{text("Plan", "规划")} {quantiles(phases.plan)}</span>
        <span><i className="receive" aria-hidden="true" />{text("Receive data", "接收数据")} {quantiles(phases.receive_data)}</span>
      </span>
    </div>
    {empty ? <p className="notify-empty">{text("No record reports phase times (RUNTIME_DETAILS).", "没有记录报告阶段耗时（RUNTIME_DETAILS）。")}</p> : <div className="load-hour-chart">
      <div aria-hidden="true" className="load-axis short">{phases.axis_ticks.map((tick, index) => <span key={index}>{count(tick)}</span>)}</div>
      <ul aria-label={text("Records per time bin", "各耗时区间的记录数")} className="load-phase-plot three">
        {phases.bins.map((bin) => {
          const label = phaseBinText(bin.lower_ms, bin.upper_ms);
          const words = text(`${label}: begin txn ${bin.begin_txn}, plan ${bin.plan}, receive data ${bin.receive_data}`, `${label}：取事务 ${bin.begin_txn}，规划 ${bin.plan}，接收数据 ${bin.receive_data}`);
          return <li aria-label={words} key={bin.lower_ms} title={words}>
            <span className="run" style={{ height: bar(bin.begin_txn_permyriad) }} />
            <span className="warn" style={{ height: bar(bin.plan_permyriad) }} />
            <span className="receive" style={{ height: bar(bin.receive_data_permyriad) }} />
          </li>;
        })}
      </ul>
      <span />
      <div aria-hidden="true" className="load-phase-labels">{phases.bins.map((bin) => <span key={bin.lower_ms}>{phaseBinText(bin.lower_ms, bin.upper_ms)}</span>)}</div>
    </div>}
    {!empty && <details className="load-phase-data">
      <summary>{text("Show the numbers", "查看分布数据")}</summary>
      <table className="history-table">
        <thead><tr><th>{text("Time", "区间")}</th><th className="num">{text("Begin txn", "取事务")}</th><th className="num">{text("Plan", "规划")}</th><th className="num">{text("Receive data", "接收数据")}</th></tr></thead>
        <tbody>{phases.bins.map((bin) => <tr key={bin.lower_ms}><td className="mono">{phaseBinText(bin.lower_ms, bin.upper_ms)}</td><td className="num">{count(bin.begin_txn)}</td><td className="num">{count(bin.plan)}</td><td className="num">{count(bin.receive_data)}</td></tr>)}</tbody>
      </table>
    </details>}
    <p className="history-muted">{text("All three come from RUNTIME_DETAILS; only Stream Load measures them. A high plan P95 usually means a busy FE or many tablets. Receive data is reported by the BE at commit; in our test runs it was always 0, so whether it matches the client's ReadDataTimeMs is still to be verified.", "三段都来自 RUNTIME_DETAILS，只有 Stream Load 会测量。规划 P95 偏高通常意味着 FE 繁忙或表的 tablet 很多。接收数据由 BE 在提交时上报，测试环境里一直是 0，是否与客户端返回的 ReadDataTimeMs 同口径尚待核实。")}</p>
  </section>;
}
