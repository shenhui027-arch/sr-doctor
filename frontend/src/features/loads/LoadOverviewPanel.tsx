// Design board 07-loads: live load overview (5 KPIs + col-7 trend/phase card +
// col-5 type/anomaly/lag card). When type is Routine Load, RoutineLoadPanel is
// rendered on the same screen (decided item 2).
import { useEffect, useState } from "react";
import type { JSX, KeyboardEvent, ReactNode } from "react";
import type {
  LoadHourBucket,
  LoadKindFilter,
  LoadKindGroup,
  LoadOverviewResponse,
  RoutineLoadJob,
  RoutineLoadJobsResponse,
  RoutineLoadStateClass,
} from "../../../../contracts/generated/doctor-contracts";
import {
  AnimatedFigure,
  Button,
  Card,
  InfoTip,
  KpiCard,
  PageGrid,
  SegTabs,
  StateStamp,
} from "../../design";
import type { LocalizedText } from "../../i18n/locale";
import { BatchView, RoutineView, StreamView } from "./LoadTypeViews";
import { RoutineLoadPanel } from "./RoutineLoadPanel";
import {
  ago,
  bytes,
  compact,
  count,
  feTime,
  groupText,
  hourRangeText,
  hourText,
  ms,
  phaseBinText,
  routineDotClass,
  routineStateLabel,
  seconds,
  shareText,
} from "./load-text";

export type OverviewFilters = { kind: LoadKindFilter | null; include_system: boolean };

export const bar = (permyriad: number): string => `${permyriad / 100}%`;

function lastFilled(hours: readonly LoadHourBucket[]): number {
  for (let index = hours.length - 1; index >= 0; index -= 1) {
    if (hours[index].loads !== "0") return index;
  }
  return hours.length - 1;
}

export function lagStateText(state: RoutineLoadStateClass, text: LocalizedText): string {
  return routineStateLabel(state, text);
}

export type OverviewActions = {
  nowMs: number;
  onOpenHour(hour: string): void;
  onOpenTable(table: string): void;
  onOpenJob(jobId: string): void;
  onOpenAlerts?(): void;
  onOpenRoutineLog?(job: RoutineLoadJob): void;
  text: LocalizedText;
};

export type OverviewKpiSpec = {
  key: string;
  label: string;
  value: ReactNode;
  unit?: string | null;
  note?: string | null;
  caveat?: string | null;
  info?: ReactNode;
  bad?: boolean;
};

export function KpiCards({ cards, label }: { cards: OverviewKpiSpec[]; label: string }): JSX.Element {
  return (
    <section aria-label={label} className="kpi-grid kpi-grid--5">
      {cards.map((item) => {
        const footParts = [
          item.caveat ? (
            <span className="load-kpi-caveat kpi-foot--warn" key="caveat">
              {item.caveat}
            </span>
          ) : null,
          item.note ? (
            <span key="note" title={item.note}>
              {item.caveat ? " · " : ""}
              {item.note}
            </span>
          ) : null,
        ].filter(Boolean);
        return (
          <KpiCard
            foot={footParts.length > 0 ? <>{footParts}</> : undefined}
            info={item.info}
            key={item.key}
            label={item.label}
            tone={item.bad ? "critical" : "default"}
            value={
              typeof item.value === "string" ? (
                <>
                  <AnimatedFigure value={item.value} />
                  {item.unit ? <span className="kpi-unit">{item.unit}</span> : null}
                </>
              ) : (
                item.value
              )
            }
          />
        );
      })}
    </section>
  );
}

export function loadsCard(
  overview: LoadOverviewResponse,
  text: LocalizedText,
  label = text("Loads", "导入次数"),
): OverviewKpiSpec {
  const records = overview.support !== "unsupported_version";
  const hiddenNote =
    overview.hidden_system !== "0"
      ? text(` · ${overview.hidden_system} system loads hidden`, ` · 另有 ${overview.hidden_system} 次系统任务已隐藏`)
      : "";
  return {
    key: "loads",
    label,
    value: records ? count(overview.summary.loads) : <StateStamp detail={text("Needs 3.3+", "需要 3.3 及以上")} state="missing" />,
    unit: records ? text("loads", "次") : null,
    note: !records
      ? text("Needs StarRocks 3.3+", "需要 3.3 及以上")
      : overview.earliest_create_time
        ? `${overview.at_retention_limit ? text("at the limit · ", "已到上限 · ") : ""}${text("oldest", "最早")} ${feTime(overview.earliest_create_time)}${hiddenNote}`
        : text("no record held", "没有保留的记录"),
  };
}

export function failedCard(
  overview: LoadOverviewResponse,
  note: string,
  text: LocalizedText,
): OverviewKpiSpec {
  const records = overview.support !== "unsupported_version";
  return {
    key: "failed",
    label: text("Failed / cancelled", "失败 / 取消"),
    value: records ? count(overview.summary.failed) : <StateStamp state="missing" />,
    unit: records ? shareText(overview.failed_share_permyriad) : null,
    note: note || null,
    bad: records && overview.summary.failed !== "0",
  };
}

export function filteredCard(
  overview: LoadOverviewResponse,
  note: string,
  text: LocalizedText,
  caveat: string | null = null,
): OverviewKpiSpec {
  const summary = overview.summary;
  const unknownRows = summary.filtered_rows_unknown !== "0" || summary.loaded_rows_unknown !== "0";
  const combinedCaveat =
    [
      unknownRows
        ? text(
            `${summary.filtered_rows_unknown} filtered / ${summary.loaded_rows_unknown} loaded unknown, left out`,
            `过滤 ${summary.filtered_rows_unknown} / 导入 ${summary.loaded_rows_unknown} 条未知，未计入`,
          )
        : null,
      caveat,
    ]
      .filter(Boolean)
      .join(text("; ", "；")) || null;
  return {
    key: "filtered",
    label: text("Filtered rows", "过滤行"),
    value:
      overview.support !== "unsupported_version" ? (
        count(summary.filtered_rows)
      ) : (
        <StateStamp state="missing" />
      ),
    unit: summary.filtered_rows !== "0" ? shareText(summary.filtered_share_permyriad) : null,
    note: note || null,
    caveat: combinedCaveat,
  };
}

export function routineJobsCard(
  overview: LoadOverviewResponse,
  text: LocalizedText,
): OverviewKpiSpec {
  const routine = overview.routine;
  const unsupported = routine.support === "unsupported_version";
  return {
    key: "routine_jobs",
    label: text("Routine Load jobs", "Routine Load 作业"),
    value: unsupported ? (
      <StateStamp detail={text("Needs 3.2+", "需要 3.2 及以上")} state="missing" />
    ) : (
      `${routine.running} / ${routine.paused}`
    ),
    unit: unsupported ? null : text("running / paused", "运行 / 暂停"),
    note: unsupported
      ? text("Needs StarRocks 3.2+", "需要 3.2 及以上")
      : text(
          `${routine.scheduling} waiting · ${routine.ended} ended`,
          `等待调度 ${routine.scheduling} · 已结束 ${routine.ended}`,
        ),
    bad: routine.paused !== "0",
  };
}

export function lagCard(overview: LoadOverviewResponse, text: LocalizedText): OverviewKpiSpec {
  const routine = overview.routine;
  const worst = routine.lags[0] ?? null;
  return {
    key: "lag",
    label: text("Largest partition lag", "最大分区积压"),
    value:
      routine.support === "without_lag" ? (
        <StateStamp detail={text("StarRocks 3.2 has no partition lag", "3.2 没有分区积压位点")} state="missing" />
      ) : worst ? (
        count(worst.lag)
      ) : (
        "—"
      ),
    unit: worst && routine.support !== "without_lag" ? text("msgs", "条") : null,
    note:
      routine.support === "without_lag"
        ? text("StarRocks 3.2 has no partition lag", "3.2 没有分区积压位点")
        : worst
          ? `${worst.job_name} · p${worst.partition} · ${lagStateText(worst.state_class, text)}`
          : routine.jobs === "0"
            ? text("no Routine Load job", "没有 Routine Load 作业")
            : text("no partition lag reported now", "当前没有可用的分区积压数据"),
    bad: worst?.state_class === "paused",
  };
}

const GROUP_ORDER: LoadKindGroup[] = ["stream_load", "insert", "routine_load", "broker", "other"];

export function HourChartBody({
  overview,
  onOpenHour,
  text,
  mode,
  unitLabel,
}: {
  overview: LoadOverviewResponse;
  onOpenHour(hour: string): void;
  text: LocalizedText;
  mode: "groups" | "outcome";
  unitLabel?: string;
}): JSX.Element {
  const hours = overview.hours;
  const fallback = lastFilled(hours);
  const [selected, setSelected] = useState(fallback);
  const [pickedHour, setPickedHour] = useState<string | null>(null);

  useEffect(() => {
    const index = pickedHour === null ? -1 : hours.findIndex((hour) => hour.hour === pickedHour);
    setSelected(index >= 0 ? index : fallback);
  }, [hours, pickedHour, fallback]);

  const pick = (index: number) => {
    setSelected(index);
    setPickedHour(hours[index]?.hour ?? null);
  };

  const current = hours[selected] ?? null;
  const firstKnown = hours.findIndex((hour) => hour.coverage !== "unknown");
  const anyPartial = hours.some((hour) => hour.coverage === "partial");
  const groups = mode === "groups";
  const showVolume = !groups;
  const isBytes = overview.volume_unit === "bytes";
  const volumeText = (value: string | null) =>
    value === null ? text("unknown", "未知") : isBytes ? bytes(value) : text(`${compact(value)} rows`, `${compact(value)} 行`);
  const volumeWithUnknown = (hour: LoadHourBucket) =>
    `${volumeText(hour.volume)}${hour.volume !== null && hour.volume_unknown !== "0" ? text(` (+${hour.volume_unknown} unknown)`, `（另 ${hour.volume_unknown} 条未知）`) : ""}`;
  const loadsWord = unitLabel ?? text("loads", "导入");
  const present = GROUP_ORDER.filter((group) =>
    hours.some((hour) => hour.segments.some((segment) => segment.group === group)),
  );

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
      : text(
          `${hour.loads} ${loadsWord}, ${hour.failed} failed, ${volumeWithUnknown(hour)}`,
          `${loadsWord} ${hour.loads}，失败 ${hour.failed}，${volumeWithUnknown(hour)}`,
        );
    return `${hourRangeText(hour.hour)}${text(": ", "：")}${parts || text("none", "无")}${
      groups && hour.failed !== "0" ? text(`, ${hour.failed} failed`, `，失败 ${hour.failed}`) : ""
    }${hour.coverage === "partial" ? text(", possibly incomplete", "，可能不完整") : ""}`;
  };

  const known = hours.map((hour, index) => ({ hour, index })).filter(({ hour }) => hour.coverage !== "unknown");
  const lines: string[][] = [];
  hours.forEach((hour, index) => {
    if (hour.coverage === "unknown" || hour.volume_permyriad === null) {
      if (lines.at(-1)?.length !== 0) lines.push([]);
      return;
    }
    if (lines.length === 0) lines.push([]);
    lines[lines.length - 1].push(`${((index + 0.5) / hours.length) * 100},${100 - hour.volume_permyriad / 100}`);
  });

  const labels =
    hours.length > 0
      ? [0, Math.floor(hours.length / 3), Math.floor((2 * hours.length) / 3), hours.length - 1]
      : [];

  if (hours.length === 0) {
    return (
      <div className="state-box">
        <span>
          {text("StarRocks holds no load record of this type right now.", "StarRocks 当前没有保留这类导入记录。")}
        </span>
      </div>
    );
  }

  return (
    <div className="detail-stack">
      <div className="load-legend">
        {groups ? (
          present.map((group) => (
            <span key={group}>
              <i aria-hidden="true" className={`group ${group}`} />
              {groupText(group, text)}
            </span>
          ))
        ) : (
          <>
            <span>
              <i aria-hidden="true" className="ok" />
              {text("Succeeded / in progress", "成功 / 进行中")}
            </span>
            <span>
              <i aria-hidden="true" className="bad" />
              {text("Failed / cancelled", "失败 / 取消")}
            </span>
            <span>
              <i aria-hidden="true" className="line" />
              {isBytes ? text("Data received (right axis)", "接收数据（右轴）") : text("Rows written (right axis)", "写入行（右轴）")}
            </span>
          </>
        )}
        {groups && hours.some((hour) => hour.failed !== "0") ? (
          <span>
            <i aria-hidden="true" className="fail-mark" />
            {text("Hour with failures", "有失败的小时")}
          </span>
        ) : null}
        {anyPartial ? (
          <span>
            <i aria-hidden="true" className="partial" />
            {text("Possibly incomplete", "可能不完整")}
          </span>
        ) : null}
        {firstKnown > 0 ? (
          <span>
            <i aria-hidden="true" className="gone" />
            {text("Before oldest record", "早于最早一条（不可查）")}
          </span>
        ) : null}
      </div>

      <div className={`load-hour-chart${showVolume ? " with-volume" : ""}`}>
        <div aria-hidden="true" className="load-axis">
          {overview.hour_axis_ticks.map((tick, index) => (
            <span key={index}>{count(tick)}</span>
          ))}
        </div>
        <div
          aria-label={text(
            "Hourly loads; use the arrow keys to move between hours",
            "每小时导入次数；可用左右方向键切换小时",
          )}
          className="load-hour-plot"
          onKeyDown={onKeyDown}
          role="group"
        >
          {hours.map((hour, index) =>
            hour.coverage === "unknown" ? (
              <span
                aria-hidden="true"
                className="load-hour gone"
                key={hour.hour}
                title={text(
                  `${hourText(hour.hour)} · before the oldest record`,
                  `${hourText(hour.hour)} · 早于最早一条，StarRocks 已不可查`,
                )}
              />
            ) : (
              <button
                aria-label={hourLabel(hour)}
                aria-pressed={index === selected}
                className={`load-hour${hour.coverage === "partial" ? " partial" : ""}`}
                data-hour={hour.hour}
                key={hour.hour}
                onClick={() => pick(index)}
                tabIndex={index === selected ? 0 : -1}
                title={hourLabel(hour)}
                type="button"
              >
                {groups ? (
                  <>
                    <span className={`fail-mark${hour.failed !== "0" ? " on" : ""}`} />
                    {[...hour.segments].reverse().map((segment) => (
                      <span
                        className={`segment ${segment.group}`}
                        key={segment.group}
                        style={{ height: bar(segment.permyriad) }}
                      />
                    ))}
                  </>
                ) : (
                  <>
                    <span className="fail" style={{ height: bar(hour.failed_permyriad) }} />
                    <span className="okay" style={{ height: bar(hour.loads_permyriad - hour.failed_permyriad) }} />
                  </>
                )}
              </button>
            ),
          )}
          {showVolume && known.length > 0 ? (
            <svg
              aria-hidden="true"
              className="load-volume-line"
              preserveAspectRatio="none"
              viewBox="0 0 100 100"
            >
              {lines
                .filter((points) => points.length > 1)
                .map((points) => (
                  <polyline key={points[0]} points={points.join(" ")} />
                ))}
            </svg>
          ) : null}
          {showVolume &&
            known.map(
              ({ hour, index }) =>
                hour.volume_permyriad !== null &&
                hour.volume_permyriad > 0 && (
                  <span
                    aria-hidden="true"
                    className="load-volume-dot"
                    key={`v${hour.hour}`}
                    style={{
                      left: `${((index + 0.5) / hours.length) * 100}%`,
                      bottom: `${hour.volume_permyriad / 100}%`,
                    }}
                  />
                ),
            )}
          {firstKnown > 0 ? (
            <span
              aria-hidden="true"
              className="load-unknown-zone"
              style={{ width: `${(firstKnown / hours.length) * 100}%` }}
            >
              {firstKnown / hours.length > 0.12 ? (
                <span>
                  {text(
                    "Unknown: before the oldest record StarRocks holds",
                    "未知：早于 StarRocks 保留的最早一条记录",
                  )}
                </span>
              ) : null}
            </span>
          ) : null}
          {firstKnown > 0 ? (
            <span
              aria-hidden="true"
              className={`load-earliest${firstKnown / hours.length > 0.6 ? " flip" : ""}`}
              style={{ left: `${(firstKnown / hours.length) * 100}%` }}
            >
              {text("oldest", "最早一条")} {feTime(overview.earliest_create_time)}
            </span>
          ) : null}
        </div>
        {showVolume ? (
          <div aria-hidden="true" className="load-axis right">
            {overview.volume_axis_ticks.map((tick, index) => (
              <span key={index}>{isBytes ? bytes(tick) : compact(tick)}</span>
            ))}
          </div>
        ) : null}
        <span />
        <div aria-hidden="true" className="load-hour-labels">
          {labels.map((index) => (
            <span key={index}>{hourText(hours[index].hour)}</span>
          ))}
        </div>
      </div>

      {anyPartial && overview.complete_from ? (
        <p className="card-subtitle">
          {text(
            `Striped hours may be incomplete: StarRocks may have dropped some kinds' records before ${feTime(overview.complete_from)}, so their bars show only what is still held.`,
            `斜纹柱为可能不完整的小时：某类导入在 ${feTime(overview.complete_from)} 之前的记录可能已被 StarRocks 清理，柱高仅代表仍保留部分。`,
          )}
        </p>
      ) : null}
      {overview.records_outside_hours !== "0" ? (
        <p className="card-subtitle">
          {text(
            `${overview.records_outside_hours} records have no readable creation time or are older than the drawn hours; they count in the KPIs only.`,
            `另有 ${overview.records_outside_hours} 条记录无可读创建时间或早于图中时段，仅计入顶部汇总。`,
          )}
        </p>
      ) : null}

      {current && current.coverage !== "unknown" ? (
        <div className="load-hour-detail">
          <button
            aria-label={text("Previous hour", "上一小时")}
            className="load-hour-step"
            disabled={selected <= firstKnown}
            onClick={() => pick(selected - 1)}
            type="button"
          >
            ‹
          </button>
          <strong role="status">{hourRangeText(current.hour)}</strong>
          <button
            aria-label={text("Next hour", "下一小时")}
            className="load-hour-step"
            disabled={selected >= hours.length - 1}
            onClick={() => pick(selected + 1)}
            type="button"
          >
            ›
          </button>
          {current.coverage === "partial" ? (
            <StateStamp state="partial" />
          ) : null}
          {groups ? (
            current.segments.map((segment) => (
              <span key={segment.group}>
                {groupText(segment.group, text)} <span className="mono">{count(segment.loads)}</span>
              </span>
            ))
          ) : (
            <span>
              {loadsWord} <span className="mono">{count(current.loads)}</span>
            </span>
          )}
          <span className={current.failed !== "0" ? "load-num-critical" : undefined}>
            {text("failed", "失败")} <span className="mono">{count(current.failed)}</span>
          </span>
          {showVolume ? (
            <span>
              {isBytes ? text("received", "接收") : text("written", "写入")}{" "}
              <span className="mono">{volumeText(current.volume)}</span>
              {current.volume_unknown !== "0"
                ? text(` (+${current.volume_unknown} unknown)`, `（另 ${current.volume_unknown} 条未知）`)
                : ""}
            </span>
          ) : (
            <span>
              {text("filtered rows", "过滤行")} <span className="mono">{count(current.filtered_rows)}</span>
              {current.filtered_unknown !== "0"
                ? text(` (+${current.filtered_unknown} unknown)`, `（另 ${current.filtered_unknown} 条未知）`)
                : ""}
            </span>
          )}
          {current.top_failed_table ? (
            <span>
              {text("most failures", "失败最多")} <span className="mono">{current.top_failed_table}</span>（
              {current.top_failed_count}）
            </span>
          ) : null}
          <Button
            className="load-hour-drill"
            data-drill="hour"
            disabled={current.loads === "0"}
            onClick={() => onOpenHour(current.hour)}
            size="sm"
            variant="secondary"
          >
            {text("Records of this hour", "查看这一小时的导入记录")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

export function PhaseChartBody({
  overview,
  text,
}: {
  overview: LoadOverviewResponse;
  text: LocalizedText;
}): JSX.Element {
  const phases = overview.phases;
  const quantiles = (q: LoadOverviewResponse["phases"]["plan"]) =>
    q.p95_ms !== null
      ? `P50 ${ms(q.p50_ms)} · P95 ${ms(q.p95_ms)}`
      : text("fewer than 20 samples", "样本不足 20 条");
  const samples = [phases.begin_txn.samples, phases.plan.samples, phases.receive_data.samples];
  const empty = samples.every((value) => value === "0");
  const sampleText = samples.every((value) => value === samples[0])
    ? text(`${samples[0]} records`, `${samples[0]} 条`)
    : text(
        `begin txn ${samples[0]} · plan ${samples[1]} · receive ${samples[2]} records`,
        `取事务 ${samples[0]} · 规划 ${samples[1]} · 接收 ${samples[2]} 条`,
      );

  if (empty) {
    return (
      <div className="state-box">
        <span>{text("No record reports Stream Load millisecond phase times.", "没有记录报告毫秒级阶段耗时。")}</span>
      </div>
    );
  }

  return (
    <div className="detail-stack">
      <div className="load-legend">
        <span>
          <i aria-hidden="true" className="run" />
          {text("Begin txn", "取事务")} {quantiles(phases.begin_txn)}
        </span>
        <span>
          <i aria-hidden="true" className="warn" />
          {text("Plan", "规划")} {quantiles(phases.plan)}
        </span>
        <span>
          <i aria-hidden="true" className="receive" />
          {text("Receive data", "接收数据")} {quantiles(phases.receive_data)}
        </span>
        <span className="card-subtitle">· {sampleText}</span>
      </div>

      <div className="load-hour-chart">
        <div aria-hidden="true" className="load-axis short">
          {phases.axis_ticks.map((tick, index) => (
            <span key={index}>{count(tick)}</span>
          ))}
        </div>
        <ul aria-label={text("Records per time bin", "各耗时区间的记录数")} className="load-phase-plot three">
          {phases.bins.map((bin) => {
            const label = phaseBinText(bin.lower_ms, bin.upper_ms);
            const words = text(
              `${label}: begin txn ${bin.begin_txn}, plan ${bin.plan}, receive data ${bin.receive_data}`,
              `${label}：取事务 ${bin.begin_txn}，规划 ${bin.plan}，接收数据 ${bin.receive_data}`,
            );
            return (
              <li aria-label={words} key={bin.lower_ms} title={words}>
                <span className="run" style={{ height: bar(bin.begin_txn_permyriad) }} />
                <span className="warn" style={{ height: bar(bin.plan_permyriad) }} />
                <span className="receive" style={{ height: bar(bin.receive_data_permyriad) }} />
              </li>
            );
          })}
        </ul>
        <span />
        <div aria-hidden="true" className="load-phase-labels">
          {phases.bins.map((bin) => (
            <span key={bin.lower_ms}>{phaseBinText(bin.lower_ms, bin.upper_ms)}</span>
          ))}
        </div>
      </div>

      <details className="disclosure load-phase-data">
        <summary>{text("Show phase numbers", "查看分布数据")}</summary>
        <table className="data-table">
          <thead>
            <tr>
              <th>{text("Bin", "耗时分档")}</th>
              <th className="num">{text("Begin txn", "取事务")}</th>
              <th className="num">{text("Plan", "规划")}</th>
              <th className="num">{text("Receive data", "接收数据")}</th>
            </tr>
          </thead>
          <tbody>
            {phases.bins.map((bin) => (
              <tr key={bin.lower_ms}>
                <td className="mono">{phaseBinText(bin.lower_ms, bin.upper_ms)}</td>
                <td className="num">{count(bin.begin_txn)}</td>
                <td className="num">{count(bin.plan)}</td>
                <td className="num">{count(bin.receive_data)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>

      <p className="card-subtitle">
        {text(
          "Measured for Stream Load only. High plan P95 typically indicates a busy FE or many tablets. Receive data is reported by BE at commit; whether it matches client ReadDataTimeMs remains to be verified.",
          "仅 Stream Load 测量毫秒级三阶段。规划 P95 偏高通常代表 FE 繁忙或 tablet 较多；接收数据由 BE 在提交时上报，是否与客户端 ReadDataTimeMs 同口径尚待核实。",
        )}
      </p>
    </div>
  );
}

export function LagTableBody({
  overview,
  nowMs,
  onOpenJob,
  text,
}: {
  overview: LoadOverviewResponse;
  nowMs: number;
  onOpenJob(jobId: string): void;
  text: LocalizedText;
}): JSX.Element {
  const routine = overview.routine;
  if (routine.support === "unsupported_version") {
    return (
      <div className="state-box">
        <span>{text("Routine Load jobs need StarRocks 3.2 or later.", "Routine Load 作业需要 StarRocks 3.2 及以上。")}</span>
      </div>
    );
  }
  if (routine.support === "without_lag") {
    return (
      <div className="state-box">
        <span>{text("StarRocks 3.2 has no partition lag metrics.", "StarRocks 3.2 没有分区积压数据。")}</span>
      </div>
    );
  }
  if (routine.lags.length === 0) {
    return (
      <div className="state-box">
        <span>{text("No live Routine Load job reports partition lag.", "当前没有运行中或暂停的作业报告分区积压。")}</span>
      </div>
    );
  }
  return (
    <table aria-label={text("Routine Load partition lag", "Routine Load 最大分区积压")} className="data-table">
      <thead>
        <tr>
          <th scope="col">{text("Job · partition", "作业 · 分区")}</th>
          <th scope="col">{text("State", "状态")}</th>
          <th className="num" scope="col">{text("Unconsumed", "未消费条数")}</th>
          <th className="num" scope="col">{text("Last consumed", "最新消费")}</th>
        </tr>
      </thead>
      <tbody>
        {routine.lags.map((lag) => (
          <tr key={`${lag.job_id}:${lag.partition}`}>
            <td>
              <button
                aria-label={text(
                  `${lag.job_name} partition ${lag.partition}, ${lagStateText(lag.state_class, text)}, lag ${lag.lag}, consumed message ${ago(lag.consumed_message_ms, nowMs, text)}; open the job`,
                  `${lag.job_name} 分区 ${lag.partition}，${lagStateText(lag.state_class, text)}，积压 ${lag.lag}，已消费消息 ${ago(lag.consumed_message_ms, nowMs, text)}；打开作业`,
                )}
                className="row-pick-btn mono"
                data-drill={`lag:${lag.job_id}:${lag.partition}`}
                onClick={() => onOpenJob(lag.job_id)}
                type="button"
              >
                {lag.job_name} · p{lag.partition}
              </button>
            </td>
            <td>
              <span className="status-inline">
                <span aria-hidden="true" className={routineDotClass(lag.state_class)} />
                <span>{lagStateText(lag.state_class, text)}</span>
              </span>
            </td>
            <td className="num">
              <strong>{count(lag.lag)}</strong>
            </td>
            <td className="num">{ago(lag.consumed_message_ms, nowMs, text)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function AllTypesOverview({
  overview,
  nowMs,
  onOpenHour,
  onOpenTable,
  onOpenJob,
  onOpenAlerts,
  text,
}: OverviewActions & { overview: LoadOverviewResponse }): JSX.Element {
  const [leftTab, setLeftTab] = useState<"hours" | "phases">("hours");
  const [rightTab, setRightTab] = useState<"groups" | "tables" | "lag">("groups");
  const records = overview.support !== "unsupported_version";
  const zone = overview.fe_time_zone ?? text("FE time zone", "FE 时区");

  const failedGroups = overview.groups
    .filter((group) => group.failed !== "0")
    .map((group) => `${groupText(group.group, text)} ${count(group.failed)}`)
    .join(" · ");

  const basis: Record<LoadKindGroup, string> = {
    stream_load: text("seconds; ms phases in Stream Load", "秒级；另有毫秒级阶段"),
    insert: text("seconds: execute + publish", "秒级：执行 + 发布"),
    routine_load: text("seconds; job totals in Routine Load", "秒级；作业累计见 Routine Load"),
    broker: text("seconds", "秒级"),
    other: text("seconds", "秒级"),
  };

  return (
    <div className="load-view-stack">
      <KpiCards
        cards={[
          loadsCard(overview, text),
          failedCard(overview, failedGroups ? `${text("of which", "其中")} ${failedGroups}` : "", text),
          filteredCard(overview, `${text("written", "导入行")} ${compact(overview.summary.loaded_rows)}`, text),
          routineJobsCard(overview, text),
          lagCard(overview, text),
        ]}
        label={text("Load summary", "导入实时汇总")}
      />

      {!records ? (
        <PageGrid>
          <Card
            span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
            title={text("Live load records", "实时导入记录")}
            variant="detail"
          >
            <div className="state-box">
              <strong>
                {overview.routine.support === "unsupported_version"
                  ? text(
                      "Load records need StarRocks 3.3 or later and Routine Load jobs 3.2 or later; neither is available on this release.",
                      "导入记录需要 StarRocks 3.3 及以上，Routine Load 作业需要 3.2 及以上；当前版本都不可用。",
                    )
                  : text(
                      "Load records need StarRocks 3.3 or later; only Routine Load is shown.",
                      "导入记录需要 StarRocks 3.3 及以上；这里只显示 Routine Load。",
                    )}
              </strong>
            </div>
          </Card>
          <Card
            span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
            title={text("Routine Load partition lag", "Routine Load 分区积压")}
            variant="table"
          >
            <LagTableBody nowMs={nowMs} onOpenJob={onOpenJob} overview={overview} text={text} />
          </Card>
        </PageGrid>
      ) : (
        <PageGrid>
          <Card
            actions={
              <SegTabs<"hours" | "phases">
                aria-label={text("Overview chart switch", "概览左图切换")}
                items={[
                  { value: "hours", label: text("Hourly trend", "每小时趋势") },
                  { value: "phases", label: text("Stream ms phases", "Stream 毫秒阶段分布") },
                ]}
                onValueChange={setLeftTab}
                size="sm"
                value={leftTab}
              />
            }
            footer={
              <span>
                {text(
                  `Creation time (${zone}, second precision)`,
                  `按创建时间聚合（${zone}，秒级精度）`,
                )}
              </span>
            }
            span={{ xl: 7, lg: 7, md: 8, sm: 4 }}
            title={
              leftTab === "hours"
                ? text(`Loads per hour · by type (${zone})`, `每小时导入次数 · 按类型（${zone}）`)
                : text("Stream Load millisecond phases", "Stream Load 阶段耗时分布")
            }
            variant="chart"
          >
            {leftTab === "hours" ? (
              <HourChartBody mode="groups" onOpenHour={onOpenHour} overview={overview} text={text} />
            ) : (
              <PhaseChartBody overview={overview} text={text} />
            )}
          </Card>

          <Card
            actions={
              <SegTabs<"groups" | "tables" | "lag">
                aria-label={text("Overview breakdown switch", "概览右卡切换")}
                items={[
                  { value: "groups", label: text("By type", "按类型对比") },
                  { value: "tables", label: text("Filtered tables", "过滤表 Top") },
                  { value: "lag", label: text("Partition lag", "分区积压") },
                ]}
                onValueChange={setRightTab}
                size="sm"
                value={rightTab}
              />
            }
            footer={
              <>
                <span>
                  {text(
                    "Total time = finish − creation (second precision, <1s counts as 0s)",
                    "总耗时 = 完成 − 创建（秒级精度，<1s 记 0s）",
                  )}
                </span>
                {onOpenAlerts ? (
                  <Button onClick={onOpenAlerts} size="sm" variant="ghost">
                    {text("Load alerts →", "导入告警 →")}
                  </Button>
                ) : (
                  <span className="card-subtitle">
                    {text("Alerts: Alerts › Loads", "导入告警见“告警 › 导入”")}
                  </span>
                )}
              </>
            }
            span={{ xl: 5, lg: 5, md: 8, sm: 4 }}
            title={text("Types & anomalies", "类型与异常排行")}
            titleExtra={
              <InfoTip label={text("About timing and filtered-row totals", "耗时口径与过滤表说明")}>
                {text(
                  "Click any table row to inspect its load records, or click a Routine Load partition row to inspect that job's partition offsets.",
                  "点击过滤表行可跳转查看该表导入记录；点击分区积压行可查看对应 Routine Load 作业位点明细。",
                )}
              </InfoTip>
            }
            variant="table"
          >
            <div hidden={rightTab !== "groups"}>
              {overview.groups.length === 0 ? (
                <div className="state-box">
                  <span>
                    {text("StarRocks holds no load record right now.", "StarRocks 当前没有保留任何导入记录。")}
                  </span>
                </div>
              ) : (
                <table aria-label={text("By load type", "按导入类型对比")} className="data-table">
                  <thead>
                    <tr>
                      <th scope="col">{text("Type", "按类型对比")}</th>
                      <th className="num" scope="col">{text("Loads", "次数")}</th>
                      <th className="num" scope="col">{text("Failed", "失败")}</th>
                      <th className="num" scope="col">{text("Rows written", "写入行")}</th>
                      <th className="num" scope="col">{text("Total P95", "总耗时 P95")}</th>
                      <th scope="col">{text("Timing", "耗时口径")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {overview.groups.map((group) => (
                      <tr key={group.group}>
                        <th scope="row">
                          <span aria-hidden="true" className={`load-group-dot ${group.group}`} />
                          {groupText(group.group, text)}
                        </th>
                        <td className="num">{count(group.loads)}</td>
                        <td className={`num${group.failed !== "0" ? " load-num-critical" : ""}`}>
                          {count(group.failed)}
                        </td>
                        <td className="num">
                          {compact(group.sink_rows)}
                          {group.sink_rows_unknown !== "0" ? (
                            <span className="load-sub load-warn-text">
                              {text(`+${group.sink_rows_unknown} unknown`, `另 ${group.sink_rows_unknown} 条未知`)}
                            </span>
                          ) : null}
                        </td>
                        <td className="num">
                          {group.total_p95_seconds !== null ? (
                            seconds(group.total_p95_seconds)
                          ) : (
                            <span className="card-subtitle" title={text("fewer than 20 finished", "已结束的不足 20 条")}>
                              —
                            </span>
                          )}
                        </td>
                        <td className="card-subtitle">{basis[group.group]}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            <div hidden={rightTab !== "tables"}>
              {overview.tables.length === 0 ? (
                <div className="state-box">
                  <span>
                    {text("No filtered row in the records StarRocks holds.", "StarRocks 保留的记录里没有被过滤的行。")}
                  </span>
                </div>
              ) : (
                <table aria-label={text("Tables with filtered rows", "有过滤行的表排行")} className="data-table">
                  <thead>
                    <tr>
                      <th scope="col">{text("Table", "库.表")}</th>
                      <th className="num" scope="col">{text("Loads", "导入次数")}</th>
                      <th className="num" scope="col">{text("Failed", "失败")}</th>
                      <th className="num" scope="col">{text("Filtered rows", "过滤行数")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {overview.tables.map((table) => (
                      <tr key={table.table}>
                        <td>
                          <button
                            className="row-pick-btn mono"
                            data-drill={`table:${table.table}`}
                            onClick={() => onOpenTable(table.table)}
                            title={text(`Open the load records of ${table.table}`, `查看 ${table.table} 的导入记录`)}
                            type="button"
                          >
                            {table.table}
                          </button>
                        </td>
                        <td className="num">{count(table.loads)}</td>
                        <td className={`num${table.failed !== "0" ? " load-num-critical" : ""}`}>
                          {count(table.failed)}
                        </td>
                        <td className="num">
                          {text(`${count(table.filtered_rows)} rows`, `${count(table.filtered_rows)} 行`)}
                          {table.filtered_unknown !== "0" ? (
                            <span className="load-sub load-warn-text">
                              {text(`+${table.filtered_unknown} unknown`, `另 ${table.filtered_unknown} 条未知`)}
                            </span>
                          ) : null}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            <div hidden={rightTab !== "lag"}>
              <LagTableBody nowMs={nowMs} onOpenJob={onOpenJob} overview={overview} text={text} />
            </div>
          </Card>
        </PageGrid>
      )}
    </div>
  );
}

export function LoadOverviewPanel({
  overview,
  kind,
  routineJobs = null,
  initialRoutineJob = null,
  ...actions
}: OverviewActions & {
  overview: LoadOverviewResponse;
  kind: LoadKindFilter | null;
  /** Live Routine Load jobs when `kind === "routine_load"` (decided item 2). */
  routineJobs?: RoutineLoadJobsResponse | null;
  initialRoutineJob?: string | null;
}): JSX.Element {
  if (kind === "routine_load" && routineJobs) {
    return (
      <RoutineLoadPanel
        initialJob={initialRoutineJob}
        jobs={routineJobs}
        key={initialRoutineJob ?? ""}
        nowMs={actions.nowMs}
        onOpenAlerts={actions.onOpenAlerts}
        onOpenLog={actions.onOpenRoutineLog ?? (() => undefined)}
        overview={overview}
        text={actions.text}
      />
    );
  }
  if (overview.stream) {
    return <StreamView detail={overview.stream} overview={overview} {...actions} />;
  }
  if (overview.batch) {
    return <BatchView broker={kind === "broker"} detail={overview.batch} overview={overview} {...actions} />;
  }
  if (overview.routine_detail) {
    return <RoutineView detail={overview.routine_detail} overview={overview} {...actions} />;
  }
  return <AllTypesOverview overview={overview} {...actions} />;
}
