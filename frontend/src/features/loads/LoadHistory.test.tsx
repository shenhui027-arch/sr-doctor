import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LoadErrorLogResponse, LoadHistoryDay, LoadHistoryErrorLogsResponse, LoadHistoryResponse, LoadHistoryRoutineResponse } from "../../../../contracts/generated/doctor-contracts";
import { ErrorLogBody } from "./LoadErrorLogDialog";
import { ErrorLogsView, RoutineView, StatsView, ppmText, utcDay } from "./LoadHistoryPanel";

const chinese = (_english: string, value: string): string => value;
const noop = () => undefined;
const DAY = 86_400;
const START = 1_793_232_000 - 6 * DAY;

const day = (index: number, overrides: Partial<LoadHistoryDay> = {}): LoadHistoryDay => ({
  start_seconds: START + index * DAY, coverage: "complete", complete_hours: 24, partial_hours: 0, not_collected_hours: 0, partial_reasons: [], segment: 0,
  loads: "120", failed: "0", loaded_rows: "1000", filtered_rows: "0", unknown_counts: 0,
  by_kind: [{ kind: "stream_load", loads: "100", height_permyriad: 5_000 }, { kind: "insert", loads: "20", height_permyriad: 1_000 }],
  failed_rate_ppm: 0, failed_rate_y_permyriad: 0, filtered_rate_ppm: 0, filtered_rate_y_permyriad: 0, ...overrides,
});

const history: LoadHistoryResponse = {
  target: "127.0.0.1:9030",
  collection: { watermark_seconds: START + 6 * DAY + 3_600, last_run_seconds: START + 6 * DAY + 3_700, last_rows_read: 37 },
  days: [
    day(0, { coverage: "not_collected", complete_hours: 0, not_collected_hours: 24, loads: null, failed: null, loaded_rows: null, filtered_rows: null, by_kind: [], failed_rate_ppm: null, failed_rate_y_permyriad: null, filtered_rate_ppm: null, filtered_rate_y_permyriad: null }),
    day(1), day(2), day(3, { segment: 1 }), day(4, { segment: 1 }),
    day(5, { segment: 1, coverage: "partial", partial_hours: 1, complete_hours: 23, partial_reasons: ["stream_records_cleared"], failed: "4", failed_rate_ppm: 4_600, failed_rate_y_permyriad: 10_000 }),
    day(6, { segment: 1, coverage: "partial", complete_hours: 1, filtered_rows: "12", unknown_counts: 2 }),
  ],
  segments: [
    { engine_versions: ["3.5.13"], rule_versions: ["r1"], first_day_seconds: START + DAY },
    { engine_versions: ["3.5.14"], rule_versions: ["r1"], first_day_seconds: START + 3 * DAY },
  ],
  kinds: [{ kind: "stream_load", loads: "600", failed: "4" }, { kind: "insert", loads: "120", failed: "0" }],
  all_loads: "720",
  routine_jobs: 0,
  totals: { loads: "720", failed: "4", failed_ppm: 5_555, loaded_rows: "241000000", loaded_unknown: 2, filtered_rows: "12", filtered_unknown: 0, filtered_ppm: 80, tables_with_filtered: 1, partial_hours: 1, not_collected_hours: 24 },
  count_axis: ["0", "100", "200"], rate_axis: ["0", "2500", "5000"],
  tables: [
    { database: "ods", table: "orders", kinds: ["stream_load"], loads: "600", failed: "4", loaded_rows: "240000000", filtered_rows: "12", unknown_counts: 0, saved_logs: 3,
      top_reason: { category: "too_long", column: "name", target_type: "VARCHAR(5)", sample_share_permyriad: 6_200, sampled_rows: 31, new_since_seconds: START + 5 * DAY } },
    { database: "ads", table: "gmv", kinds: ["insert"], loads: "120", failed: "0", loaded_rows: "1000000", filtered_rows: "0", unknown_counts: 0, saved_logs: 0, top_reason: null },
  ],
  table_options: ["ads.gmv", "ods.orders"],
  error_logs: { logs: 5, saved: 3, raw_bytes: "13002342", limit_bytes: "52428800", used_permyriad: 2_480, earliest_saved_seconds: START + DAY, missed: 1 },
  limitations: [],
};

describe("load history statistics (design board I9)", () => {
  it("lays out the server totals, days, segments, coverage and tables", () => {
    const html = renderToStaticMarkup(<StatsView history={history} onOpenLogs={noop} text={chinese} />);
    expect(html).toContain("已结束的导入");
    expect(html).toContain("跨 2 个版本段，不跨段比较");
    expect(html).toContain("24 小时未采集，未计入");
    expect(html).toContain("2 次没有报告行数，未计入");
    expect(html).toContain("0.56%");
    expect(html).toContain("0.008%");
    expect(html).toContain("12.4 MB / 50 MB");
    expect(html).toContain("1 份读取前已被清理");
    expect(html).toContain(`${utcDay(START + 3 * DAY)} 起 StarRocks 3.5.14 · 不跨段比较`);
    expect(html).toContain(`${utcDay(START)} 未采集`);
    expect(html).toContain("cov-not_collected");
    expect(html).toContain("cov-partial");
    // The latest collected day is selected and explains why it is partial.
    expect(html).toContain("过滤行");
    expect(html).toContain("2 个计数没有报告，未计入");
    expect(html).toContain("字符串超长");
    expect(html).toContain("name · VARCHAR(5)");
    expect(html).toContain("样本内占比 62.00%");
    expect(html).toContain("只统计已采集的时段");
    expect(html).toContain(`新出现 · ${utcDay(START + 5 * DAY)}`);
    expect(html).toContain("错误日志 3");
    expect(html).toContain("没有过滤行");
    // Uncollected days and the version change break the rate line: days 1-2
    // and days 3-6 are separate runs, never one line across segments.
    expect((html.match(/class="rate-failed"/g) ?? []).length).toBe(2);
    expect(html).toContain("未采集的天与 StarRocks 版本变化处断开");
    // With a type selected, the failure note does not list other types.
    const insertOnly = renderToStaticMarkup(<StatsView history={history} kind="insert" onOpenLogs={noop} text={chinese} />);
    expect(insertOnly).not.toContain("Stream Load 4");
    expect(insertOnly).toContain("占已结束导入");
  });

  it("formats small rates with enough digits", () => {
    expect(ppmText(80)).toBe("0.008%");
    expect(ppmText(4_600)).toBe("0.46%");
    expect(ppmText(333_333)).toBe("33.3%");
    expect(ppmText(0)).toBe("0%");
    expect(ppmText(null)).toBe("—");
  });
});

const routine: LoadHistoryRoutineResponse = {
  target: "127.0.0.1:9030", cell_seconds: 3_600, range_start_seconds: START, limitations: [],
  jobs: [{
    job_id: "10276", name: "events_rl", database: "dwd", table: "user_events", current_state: "PAUSED", pauses: 2,
    cells: [{ start_seconds: START, state: "not_collected" }, { start_seconds: START + 3_600, state: "running" }, { start_seconds: START + 7_200, state: "paused" }],
    loaded_rows: "68800000", error_rows: "1272", committed_tasks: "4902", aborted_tasks: "3", unknown_intervals: 1, max_lag: "240784",
  }, {
    job_id: "10300", name: "fresh_rl", database: "dwd", table: "fresh", current_state: "RUNNING", pauses: 0,
    cells: [{ start_seconds: START, state: "unknown" }], loaded_rows: null, error_rows: null, committed_tasks: null, aborted_tasks: null, unknown_intervals: 0, max_lag: null,
  }],
  selected: {
    job_id: "10276", name: "events_rl", lag_known: true, lag_axis: ["0", "125000", "250000"],
    lag: [
      { start_seconds: START, max_lag: null, y_permyriad: null, paused: false, collected: false },
      { start_seconds: START + 3_600, max_lag: "1200", y_permyriad: 48, paused: false, collected: true },
      { start_seconds: START + 7_200, max_lag: "240784", y_permyriad: 9_631, paused: true, collected: true },
    ],
    events: [{ at_seconds: START + 7_200, from_state: "RUNNING", to_state: "PAUSED", reason: "ErrorReason{errCode = 102, msg='current error rows is more than max error num'}" }],
    days: [
      { start_seconds: START, loaded_rows: null, error_rows: null, loaded_height_permyriad: 0, error_height_permyriad: 0, unknown_intervals: 0, collected: false },
      { start_seconds: START + DAY, loaded_rows: "2100000", error_rows: "8", loaded_height_permyriad: 5_000, error_height_permyriad: 2, unknown_intervals: 0, collected: true },
      { start_seconds: START + 2 * DAY, loaded_rows: null, error_rows: null, loaded_height_permyriad: 0, error_height_permyriad: 0, unknown_intervals: 2, collected: true },
    ],
    rows_axis: ["0", "2000000", "4000000"],
  },
};

describe("load history Routine Load (design board I10)", () => {
  it("shows job state cells, sums, lag, events and unknown days", () => {
    const html = renderToStaticMarkup(<RoutineView onJob={noop} routine={routine} text={chinese} />);
    expect(html).toContain("events_rl");
    expect(html).toContain("state-paused");
    expect(html).toContain("state-not_collected");
    expect(html).toContain("4,902 / 3");
    // No known interval: the sums say unknown, never zero.
    expect((html.match(/>未知</g) ?? []).length).toBeGreaterThanOrEqual(3);
    expect(html).toContain("state-unknown");
    expect(html).toContain("240,784");
    expect(html).toContain("累计值变小（作业重建）或两次快照相隔太久的区间记为未知");
    expect(html).toContain("lag-paused");
    expect(html).toContain("lag-gap-hatch");
    expect(html).toContain("不代表真实切换时刻");
    expect(html).toContain("导入行（已观测）");
    expect(html).toContain("current error rows is more than max error num");
    expect(html).toContain(`${utcDay(START)} 未采集`);
    expect(html).toContain(`${utcDay(START + 2 * DAY)} 未知`);
  });

  it("says lag is unknown before 3.3 instead of drawing zero", () => {
    const html = renderToStaticMarkup(<RoutineView onJob={noop} routine={{ ...routine, selected: { ...routine.selected!, lag_known: false } }} text={chinese} />);
    expect(html).toContain("3.3 之前的 StarRocks 没有 OFFSET_LAG，积压未知");
  });
});

const errorLogs: LoadHistoryErrorLogsResponse = {
  target: "127.0.0.1:9030", total: 2, table_options: ["ods.orders"], limitations: [],
  storage: history.error_logs,
  reasons: [{ database: "ods", table: "orders", category: "too_long", column: "name", target_type: "VARCHAR(5)", sampled_rows: 1_240, loads: 31, first_seconds: START + DAY, last_seconds: START + 6 * DAY, new: false }],
  logs: [
    { job_id: "10256", label: "orders_20261030_0915", database: "ods", table: "orders", kind: "stream_load", finish_seconds: START + 6 * DAY, filtered_rows: "6", state: "saved", attempts: 0, saved_seconds: START + 6 * DAY + 300, sampled: 6, truncated: false,
      categories: [{ category: "too_long", column: "name", target_type: "VARCHAR(5)", count: 2 }] },
    { job_id: "9981", label: "orders_20261006_2209", database: "ods", table: "orders", kind: "stream_load", finish_seconds: START, filtered_rows: "880", state: "purged", attempts: 0, saved_seconds: null, sampled: 0, truncated: false, categories: [] },
  ],
};

describe("saved error logs (design board I11)", () => {
  it("lists facts only, with a View button for saved text", () => {
    const html = renderToStaticMarkup(<ErrorLogsView category={null} logs={errorLogs} offset={0} onCategory={noop} onOffset={noop} onOpen={noop} onTable={noop} table={null} text={chinese} />);
    expect(html).toContain("12.4 MB");
    expect(html).toContain("1,240");
    expect(html).toContain("orders_20261030_0915");
    expect(html).toContain("读取前已被清理");
    expect((html.match(/>查看</g) ?? []).length).toBe(1);
    expect(html).toContain("原始行含业务数据");
    expect(html).toContain("能登录 SR Doctor 的人都能看到所有库的原文");
    expect(html).not.toContain("2026-10-30");
  });

  it("says why a trimmed copy has no text", () => {
    const empty: LoadErrorLogResponse = { job_id: "1", state: "none", entries: [], categories: [], truncated: false, limitations: [] };
    expect(renderToStaticMarkup(<ErrorLogBody log={empty} storedState="trimmed" text={chinese} />)).toContain("保存的原文已按 50 MB 上限删除");
  });
});
