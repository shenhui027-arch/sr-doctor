import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { BatchLoadDetail, LoadHourBucket, LoadOverviewResponse, RoutineLoadDetail, StreamLoadDetail } from "../../../../contracts/generated/doctor-contracts";
import { LoadOverviewPanel } from "./LoadOverviewPanel";
import { LoadSummaryCard, LoadSummaryView } from "./LoadSummaryCard";
import { durationBinText, hourRangeText, phaseBinText, shareText } from "./load-text";

const chinese = (_english: string, value: string): string => value;
const noop = () => undefined;
const NOW = 1_793_348_156_695;

const hour = (key: string, loads: number, failed: number, coverage: LoadHourBucket["coverage"] = "complete"): LoadHourBucket => ({
  hour: key, coverage, loads: String(loads), failed: String(failed), filtered_rows: failed ? "1204" : "0",
  filtered_unknown: "0", loads_permyriad: loads * 250, failed_permyriad: failed * 250, top_failed_table: failed ? "ods.orders" : null, top_failed_count: failed ? String(failed) : null,
  segments: loads ? [{ group: "stream_load", loads: String(loads - 2 > 0 ? loads - 2 : loads), permyriad: (loads - 2 > 0 ? loads - 2 : loads) * 250 }, ...(loads - 2 > 0 ? [{ group: "insert" as const, loads: "2", permyriad: 500 }] : [])] : [],
  volume: String(loads * 1_000_000), volume_unknown: "0", volume_permyriad: loads * 250,
});

const overview: LoadOverviewResponse = {
  support: "full", fe_time_zone: "Asia/Shanghai", at_retention_limit: true, earliest_create_time: "2026-10-28 06:12:40", complete_from: "2026-10-28 06:12:40", hidden_system: "6",
  summary: { loads: "1000", failed: "17", loaded_rows: "48200000", filtered_rows: "3204", loaded_rows_unknown: "0", filtered_rows_unknown: "0", filtered_share_permyriad: 1, begin_plan_p95_ms: "142" },
  failed_share_permyriad: 170, records_outside_hours: "0",
  by_kind: [{ kind: "STREAM_LOAD", loads: "900", failed: "12" }, { kind: "INSERT", loads: "100", failed: "5" }],
  hours: [hour("2026-10-28 05", 0, 0, "unknown"), hour("2026-10-28 06", 12, 0), hour("2026-10-30 06", 31, 6), hour("2026-10-30 07", 0, 0)],
  hour_axis_ticks: ["40", "20", "0"],
  groups: [
    { group: "stream_load", loads: "900", failed: "12", sink_rows: "48000000", sink_rows_unknown: "0", filtered_rows: "3204", filtered_unknown: "0", total_p95_seconds: "2" },
    { group: "insert", loads: "100", failed: "5", sink_rows: "200000", sink_rows_unknown: "1", filtered_rows: "0", filtered_unknown: "0", total_p95_seconds: null },
  ],
  all_loads: "1000", volume_unit: "rows", volume_axis_ticks: ["40000000", "20000000", "0"], stream: null, batch: null, routine_detail: null,
  tables: [{ table: "ods.orders", loads: "612", failed: "11", filtered_rows: "2980", filtered_unknown: "0", width_permyriad: 10_000 }],
  phases: {
    bins: [{ lower_ms: "0", upper_ms: "10", begin_txn: "402", plan: "318", receive_data: "12", begin_txn_permyriad: 10_000, plan_permyriad: 7_900, receive_data_permyriad: 300 }, { lower_ms: "500", upper_ms: "1000", begin_txn: "4", plan: "10", receive_data: "0", begin_txn_permyriad: 100, plan_permyriad: 250, receive_data_permyriad: 0 }, { lower_ms: "5000", upper_ms: null, begin_txn: "0", plan: "0", receive_data: "0", begin_txn_permyriad: 0, plan_permyriad: 0, receive_data_permyriad: 0 }],
    axis_ticks: ["400", "200", "0"],
    begin_txn: { samples: "642", p50_ms: "3", p95_ms: "21" },
    plan: { samples: "642", p50_ms: "6", p95_ms: "128" },
    receive_data: { samples: "642", p50_ms: "0", p95_ms: "0" },
  },
  routine: {
    support: "full", jobs: "5", running: "2", scheduling: "1", paused: "1", ended: "1", error_rows: "1284", error_rows_unknown: "0", truncated: false,
    lags: [{ job_id: "2", job_name: "events_rl", db: "dwd", table: "user_events", partition: "1", lag: "240784", state_class: "paused", consumed_message_ms: String(NOW - 2_700_000), width_permyriad: 10_000 }],
    max_running_lag: { job_id: "1", job_name: "orders_rl", db: "ods", table: "orders", partition: "3", lag: "48210", state_class: "running", consumed_message_ms: null, width_permyriad: 8_800 },
    paused_jobs: [{ job_id: "2", name: "events_rl", db: "dwd", table: "user_events", max_lag: "240784", pause_time: "2026-10-30 08:57:04", reason: "current error rows: 1272 is more than max error num: 1000" }],
  },
  limitations: [],
};

const stream: StreamLoadDetail = {
  received_bytes: "19756849561", received_bytes_unknown: "0", unselected_rows: "1120", unselected_unknown: "0", publish_p95_seconds: "1", near_timeout_count: "1",
  total: { bins: [{ lower_seconds: "0", upper_seconds: "1", loads: "500", permyriad: 10_000 }, { lower_seconds: "1", upper_seconds: "10", loads: "120", permyriad: 2_400 }], samples: "620", p50_seconds: "0", p95_seconds: "2", max_seconds: "41" },
  near_timeout: [{ job_id: "9", label: "orders_0612", table: "ods.orders", total_seconds: "287", execute_seconds: "286", publish_seconds: "1", timeout_seconds: "300", timeout_share_permyriad: 9_566, execute_permyriad: 9_965, publish_permyriad: 34 }],
  failure_reasons: [{ reason: "filtered_ratio", loads: "7", latest_time: "2026-10-30 06:12:40", latest_table: "ods.orders", width_permyriad: 10_000 }, { reason: "timeout", loads: "3", latest_time: "2026-10-30 03:12:00", latest_table: "ods.payments", width_permyriad: 4_285 }],
  tables: [{ table: "ods.orders", loads: "210", failed: "9", scan_rows: "0", scan_bytes: "12025908428", sink_rows: "31402118", filtered_rows: "2980", counts_unknown: "0", total_p95_seconds: "2", plan_p95_ms: "142", priority: null }],
  sources: [{ client_ip: "10.20.3.17", user: "etl_writer", loads: "388", failed: "8" }],
};

const batch: BatchLoadDetail = {
  scan_rows: "131000000", scan_bytes: "45742364262", counts_unknown: "2", near_timeout_count: "3",
  duration: { bins: [{ lower_seconds: "0", upper_seconds: "1", loads: "38", permyriad: 3_725 }, { lower_seconds: "60", upper_seconds: "300", loads: "24", permyriad: 2_352 }, { lower_seconds: "1800", upper_seconds: null, loads: "0", permyriad: 0 }], samples: "206", p50_seconds: "4", p95_seconds: "184", max_seconds: "1640" },
  slowest: [{ job_id: "31", label: "insert_6f1a", table: "dwd.order_wide", total_seconds: "1640", execute_seconds: "1622", publish_seconds: "18", timeout_seconds: "1800", timeout_share_permyriad: 9_111, execute_permyriad: 9_890, publish_permyriad: 109 }],
  failure_reasons: [{ reason: "TIMEOUT", loads: "3", latest_time: "2026-10-30 03:12:00", latest_table: "dwd.order_wide", width_permyriad: 10_000 }],
  running_states: [{ state: "LOADING", loads: "2" }],
  tables: [{ table: "dwd.order_wide", loads: "48", failed: "3", scan_rows: "52118004", scan_bytes: "19541180416", sink_rows: "50002113", filtered_rows: "0", counts_unknown: "0", total_p95_seconds: "1640", plan_p95_ms: null, priority: "NORMAL" }],
};

const routineDetail: RoutineLoadDetail = {
  error_rows: "2906", total_rows: "9806112", rows_unknown: "0", error_share_permyriad: 2, committed_tasks: "3980", aborted_tasks: "170", tasks_unknown: "0", abort_share_permyriad: 409, average_task_ms: "9800", held_tasks: "141",
  tasks: { bins: [{ lower_seconds: "5", upper_seconds: "10", loads: "71", permyriad: 10_000 }], samples: "141", p50_seconds: "9", p95_seconds: "14", max_seconds: "41" },
  jobs: [{ job_id: "2", name: "events_rl", db: "dwd", table: "user_events", state: "PAUSED", state_class: "paused", current_tasks: "0", desired_tasks: "2", load_rows_rate: "9880", received_bytes_rate: "3460300", loaded_rows: "781664", total_rows: "784450", error_rows: "2786", error_share_permyriad: 35, committed_tasks: "176", aborted_tasks: "158", abort_share_permyriad: 4_730, average_task_ms: "11400", max_batch_interval_seconds: "10", max_lag: "240784", max_lag_partition: "1", newest_consumed_ms: String(NOW - 2_700_000) },
    { job_id: "5", name: "fresh_rl", db: "ods", table: "t", state: "RUNNING", state_class: "running", current_tasks: "1", desired_tasks: "1", load_rows_rate: null, received_bytes_rate: null, loaded_rows: "0", total_rows: "0", error_rows: "0", error_share_permyriad: null, committed_tasks: "0", aborted_tasks: "0", abort_share_permyriad: null, average_task_ms: null, max_batch_interval_seconds: "10", max_lag: null, max_lag_partition: null, newest_consumed_ms: null }],
  issues: [{ job_id: "2", name: "events_rl", state: "PAUSED", state_class: "paused", time: "2026-10-30 08:57:04", reason: "current error rows: 1272 is more than max error num: 1000", error_share_permyriad: 35, aborted_tasks: "158" }],
};

describe("load overview, all types (design board I5)", () => {
  it("lays out the server's cards, stacked hours, types and tables", () => {
    const html = renderToStaticMarkup(<LoadOverviewPanel kind={null} nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={overview} text={chinese} />);
    expect(html).toContain("已到上限 · 最早 10-28 06:12:40");
    expect(html).toContain("1.70%");
    expect(html).toContain("其中 Stream Load 12 · INSERT 5");
    expect(html).toContain("2 / 1");
    expect(html).toContain("events_rl · p1 · 已暂停");
    // The last hour with loads is selected, not the empty newest one.
    expect(html).toContain("10-30 06:00–07:00");
    expect(html).toContain("失败最多 <span class=\"mono\">ods.orders</span>（6）");
    // Hours before the oldest record are unknown, not buttons with zero.
    expect(html).toContain("早于最早一条，StarRocks 已不可查");
    expect(html.match(/class="load-hour"/g)).toHaveLength(3);
    // Stacked by type, each segment as tall as the server says, with a failure mark.
    expect(html).toContain("class=\"segment insert\" style=\"height:5%\"");
    expect(html).toContain("class=\"fail-mark on\"");
    expect(html).toContain("Stream Load 29，INSERT 2，失败 6");
    // The type table: counts, unknowns kept apart, the timing basis in words.
    expect(html).toContain("按类型对比");
    expect(html).toContain("另 1 条未知");
    expect(html).toContain("秒级：执行 + 发布");
    expect(html).toContain("2,980 行");
    expect(html).toContain("另有 6 次系统任务已隐藏");
    expect(html).toContain("导入告警见“告警 › 导入”");
    expect(html).toContain("未知：早于 StarRocks 保留的最早一条记录");
    expect(html).toContain("aria-label=\"上一小时\"");
    expect(html).not.toContain("阶段耗时分布");
  });

  it("draws partial hours with their held counts and says they may be incomplete", () => {
    const partial = { ...overview, complete_from: "2026-10-30 06:05:00", hours: [hour("2026-10-28 05", 0, 0, "unknown"), hour("2026-10-28 06", 12, 0, "partial"), hour("2026-10-30 06", 31, 6)] };
    const html = renderToStaticMarkup(<LoadOverviewPanel kind={null} nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={partial} text={chinese} />);
    expect(html).toContain("class=\"load-hour partial\"");
    expect(html).toContain("Stream Load 10，INSERT 2，可能不完整");
    expect(html).toContain("某类导入在 10-30 06:05:00 之前的记录可能已被 StarRocks 清理");
  });

  it("says when versions or data cannot show a chart", () => {
    const old = renderToStaticMarkup(<LoadOverviewPanel kind={null} nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, support: "unsupported_version", routine: { ...overview.routine, support: "without_lag", lags: [], max_running_lag: null } }} text={chinese} />);
    expect(old).toContain("导入记录需要 StarRocks 3.3 及以上");
    expect(old).toContain("3.2 没有 OFFSET_LAG");
    expect(old).not.toContain("按类型对比");
    const v31 = renderToStaticMarkup(<LoadOverviewPanel kind={null} nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, support: "unsupported_version", routine: { ...overview.routine, support: "unsupported_version", lags: [], max_running_lag: null } }} text={chinese} />);
    expect(v31).toContain("当前版本都不可用");
    expect(v31).not.toContain("这里只显示 Routine Load");
    const empty = renderToStaticMarkup(<LoadOverviewPanel kind={null} nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, hours: [], tables: [], groups: [] }} text={chinese} />);
    expect(empty).toContain("StarRocks 当前没有保留这类导入记录");
    expect(empty).toContain("StarRocks 当前没有保留任何导入记录");
    expect(empty).toContain("没有被过滤的行");
    const thin = renderToStaticMarkup(<LoadOverviewPanel kind="stream_load" nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, stream: { ...stream, total: { ...stream.total, p95_seconds: null, p50_seconds: null } }, phases: { ...overview.phases, begin_txn: { samples: "3", p50_ms: null, p95_ms: null } } }} text={chinese} />);
    expect(thin).toContain("样本不足 20 条");
    expect(thin).toContain("已结束的不足 20 条");
    expect(renderToStaticMarkup(<LoadSummaryView current={{ overview: { ...overview, summary: { ...overview.summary, begin_plan_p95_ms: null } }, error: null }} onOpen={noop} onRetry={noop} text={chinese} />)).toContain("—<small> 样本不足 20 条</small>");
  });

  it("formats shares, hours and bins without guessing", () => {
    expect(shareText(1)).toBe("0.01%");
    expect(shareText(10_000)).toBe("100.00%");
    expect(shareText(null)).toBeNull();
    expect(hourRangeText("2026-10-30 23")).toBe("10-30 23:00–00:00");
    expect(phaseBinText("500", "1000")).toBe("500 ms–1 s");
    expect(phaseBinText("10", "50")).toBe("10–50 ms");
    expect(phaseBinText("1000", "5000")).toBe("1–5 s");
    expect(phaseBinText("5000", null)).toBe("≥ 5 s");
    expect(durationBinText("0", "1")).toBe("< 1 s");
    expect(durationBinText("10", "60")).toBe("10 s–1 min");
    expect(durationBinText("60", "300")).toBe("1–5 min");
    expect(durationBinText("1800", null)).toBe("≥ 30 min");
  });
});

describe("load overview by type (design boards I6-I8)", () => {
  const view = (kind: "stream_load" | "insert" | "broker" | "routine_load", data: Partial<LoadOverviewResponse>) => renderToStaticMarkup(<LoadOverviewPanel kind={kind} nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, ...data }} text={chinese} />);

  it("Stream Load: bytes, three phases, publish, near-timeout loads, tables and sources", () => {
    const html = view("stream_load", { stream, volume_unit: "bytes" });
    expect(html).toContain("接收数据");
    expect(html).toContain("18.4 GB");
    expect(html).toContain("取事务 P50 3 ms · P95 21 ms");
    expect(html).toContain("&lt; 10 ms");
    expect(html).toContain("查看分布数据");
    expect(html).toContain("class=\"receive\" style=\"height:3%\"");
    expect(html).toContain("尚待核实");
    // Split into succeeded and failed with the volume line, not stacked by type.
    expect(html).toContain("height:15%");
    expect(html).toContain("<polyline");
    expect(html).toContain("接收数据（右轴）");
    expect(html).toContain("发布等待 P95");
    expect(html).toContain("超时 300 秒的 95.66%");
    expect(html).toContain("过滤行超过 max_filter_ratio");
    expect(html).toContain("最近一次 10-30 06:12:40");
    expect(html).toContain("10.20.3.17");
    expect(html).toContain("142 ms");
    expect(html).not.toContain("按类型对比");
    expect(html).toContain("load-volume-dot");
    expect(view("stream_load", { stream: { ...stream, unselected_unknown: "2" } })).toContain("2 条未知，未计入");
  });

  it("keeps unknown volumes, lone phases and missing lag apart from zero", () => {
    // An hour whose loads all leave the volume unknown is a gap, never 0 B.
    const [gone, first, second, last] = overview.hours;
    const html = view("stream_load", { stream, volume_unit: "bytes", hours: [gone, first, { ...second, volume: null, volume_unknown: "31", volume_permyriad: null }, last] });
    expect(html).toContain("失败 6，未知");
    expect(html).not.toContain("失败 6，0 B");
    expect(html).not.toContain("<polyline");
    // Only the receive phase has samples: the chart still draws.
    const receiveOnly = { samples: "0", p50_ms: null, p95_ms: null };
    const lone = view("stream_load", { stream, phases: { ...overview.phases, begin_txn: receiveOnly, plan: receiveOnly } });
    expect(lone).not.toContain("没有记录报告阶段耗时");
    expect(lone).toContain("取事务 0 · 规划 0 · 接收");
    // Jobs without a reported lag are not "no job".
    const noLag = view("routine_load", { routine_detail: routineDetail, routine: { ...overview.routine, lags: [] } });
    expect(noLag).toContain("当前没有可用的分区积压数据");
    // Incompleteness has its own uncut line and one kind never hides another.
    const both = view("stream_load", { stream: { ...stream, unselected_unknown: "2" }, summary: { ...overview.summary, filtered_rows_unknown: "1" } });
    expect(both).toContain("<span class=\"load-kpi-caveat\">过滤 1 / 导入 0 条未知，未计入；未选中行 2 条未知，未计入</span>");
    const untimed = view("routine_load", { routine_detail: { ...routineDetail, average_task_ms: null, tasks_unknown: "1" } });
    expect(untimed).toContain("<span class=\"load-kpi-caveat\">1 个作业未报告，未计入</span>");
    expect(untimed).toContain("有作业没有报告任务数或执行时间，不给平均值");
  });

  it("INSERT and Broker Load: seconds, the two segments of the slowest, failure types and running loads", () => {
    const html = view("insert", { batch });
    expect(html).toContain("INSERT 汇总");
    expect(html).toContain("未结束: 执行中 2");
    expect(html).toContain("184 s");
    expect(html).toContain("没有“排队”这一段");
    expect(html).toContain("1–5 min");
    expect(html).toContain("最慢的 INSERT");
    expect(html).toContain("class=\"run\" style=\"width:98.9%\"");
    expect(html).toContain("执行 1622 秒，发布 18 秒");
    expect(html).toContain("超时（TIMEOUT）");
    expect(html).toContain("2 条有未知计数，未计入");
    expect(html).toContain("NORMAL");
    const broker = view("broker", { batch });
    expect(broker).toContain("最慢的 Broker Load");
    // Broker Load queues before it loads; only INSERT has no queue phase.
    expect(broker).toContain("加载前有排队与准备");
    expect(broker).toContain("提交前（创建 → 提交，含排队）");
    expect(broker).not.toContain("没有“排队”这一段");
  });

  it("Routine Load: job health from lifetime counters, task rhythm, lag and jobs to look at", () => {
    const html = view("routine_load", { routine_detail: routineDetail });
    expect(html).toContain("0.02%");
    expect(html).toContain("4.09%");
    expect(html).toContain("9.8 s");
    expect(html).toContain("9,880 行/s");
    expect(html).toContain("3.3 MB/s");
    // No rate before the first committed task: shown as missing, never 0.
    expect(html).toMatch(/fresh_rl[\s\S]*?<td class="num">—<\/td><td class="num">—<\/td>/);
    expect(html).toContain("47.30%");
    expect(html).toContain("不是当前速率");
    expect(html).toContain("每小时任务与导入行");
    expect(html).toContain("events_rl 分区 1，已暂停，积压 240784");
    expect(html).toContain("45 分钟前");
    expect(html).toContain("暂停于 10-30 08:57:04");
    expect(html).toContain("current error rows: 1272 is more than max error num: 1000");
    const v32 = view("routine_load", { routine_detail: { ...routineDetail, held_tasks: "0" }, support: "unsupported_version", routine: { ...overview.routine, support: "without_lag" } });
    expect(v32).toContain("任务记录需要 StarRocks 3.3 及以上");
    expect(v32).toContain("3.2：没有积压");
    expect(html).toContain("单凭它看不出批次");
    // Jobs without counters are left out and said so; none at all is unknown.
    const partial = view("routine_load", { routine_detail: { ...routineDetail, rows_unknown: "1", tasks_unknown: "1" } });
    expect(partial).toContain("1 个作业未报告，未计入");
    const none = view("routine_load", { routine_detail: { ...routineDetail, error_rows: null, total_rows: null, error_share_permyriad: null, rows_unknown: "2" } });
    expect(none).toContain("错误行 — / 总行 —");
  });
});

describe("audit overview load card (design board O3)", () => {
  it("shows a loading state before the read returns", () => {
    const html = renderToStaticMarkup(<LoadSummaryCard api={{ readLoadOverview: () => new Promise(() => undefined) }} connectionId="c" describeFailure={() => "x"} onOpen={noop} text={chinese} />);
    expect(html).toContain("正在读取导入信息");
    expect(html).toContain("aria-busy=\"true\"");
  });

  it("summarises loads and the newest paused job, and says it ignores the audit window", () => {
    const html = renderToStaticMarkup(<LoadSummaryView current={{ overview, error: null }} onOpen={noop} onRetry={noop} text={chinese} />);
    expect(html).toContain("不随上方时间窗变化");
    expect(html).toContain("打开导入概览");
    expect(html).toContain("1.70%");
    expect(html).toContain("events_rl 已暂停 · 积压 240,784");
    expect(html).toContain("orders_rl · p3 · 48,210");
    expect(html).toContain("10-28 05:00 到 10-30 07:00 每小时导入次数");
    expect(html).toContain("未知：早于 10-28 06:12:40");
    expect(renderToStaticMarkup(<LoadSummaryView current={{ overview, error: null, readAt: NOW }} onOpen={noop} onRetry={noop} text={chinese} />)).toContain("（本机时间）");
    expect(html).toContain("刷新");
  });

  it("keeps failures, empty clusters and old versions apart", () => {
    expect(renderToStaticMarkup(<LoadSummaryView current={{ overview: null, error: "请求超时" }} onOpen={noop} onRetry={noop} text={chinese} />)).toContain("读取导入信息失败（请求超时），不影响上方审计概览");
    const none = { ...overview, summary: { ...overview.summary, loads: "0" }, routine: { ...overview.routine, running: "0", scheduling: "0", paused: "0", ended: "0" } };
    expect(renderToStaticMarkup(<LoadSummaryView current={{ overview: none, error: null }} onOpen={noop} onRetry={noop} text={chinese} />)).toContain("StarRocks 当前没有保留任何导入记录，也没有 Routine Load 作业");
    const v32 = renderToStaticMarkup(<LoadSummaryView current={{ overview: { ...overview, support: "unsupported_version", routine: { ...overview.routine, support: "without_lag", paused_jobs: [], max_running_lag: null } }, error: null }} onOpen={noop} onRetry={noop} text={chinese} />);
    expect(v32).toContain("只显示 Routine Load 的状态");
    expect(v32).toContain("3.2 没有积压数据");
  });
});
