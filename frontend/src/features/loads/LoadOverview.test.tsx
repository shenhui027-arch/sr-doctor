import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LoadHourBucket, LoadOverviewResponse } from "../../../../contracts/generated/doctor-contracts";
import { LoadOverviewPanel } from "./LoadOverviewPanel";
import { LoadSummaryCard, LoadSummaryView } from "./LoadSummaryCard";
import { hourRangeText, phaseBinText, shareText } from "./load-text";

const chinese = (_english: string, value: string): string => value;
const noop = () => undefined;
const NOW = 1_793_348_156_695;

const hour = (key: string, loads: number, failed: number, coverage: LoadHourBucket["coverage"] = "complete"): LoadHourBucket => ({
  hour: key, coverage, loads: String(loads), failed: String(failed), filtered_rows: failed ? "1204" : "0",
  filtered_unknown: "0", loads_permyriad: loads * 250, failed_permyriad: failed * 250, top_failed_table: failed ? "ods.orders" : null, top_failed_count: failed ? String(failed) : null,
});

const overview: LoadOverviewResponse = {
  support: "full", fe_time_zone: "Asia/Shanghai", at_retention_limit: true, earliest_create_time: "2026-10-28 06:12:40", complete_from: "2026-10-28 06:12:40", hidden_system: "6",
  summary: { loads: "1000", failed: "17", loaded_rows: "48200000", filtered_rows: "3204", loaded_rows_unknown: "0", filtered_rows_unknown: "0", filtered_share_permyriad: 1, begin_plan_p95_ms: "142" },
  failed_share_permyriad: 170, records_outside_hours: "0",
  by_kind: [{ kind: "STREAM_LOAD", loads: "900", failed: "12" }, { kind: "INSERT", loads: "100", failed: "5" }],
  hours: [hour("2026-10-28 05", 0, 0, "unknown"), hour("2026-10-28 06", 12, 0), hour("2026-10-30 06", 31, 6), hour("2026-10-30 07", 0, 0)],
  hour_axis_ticks: ["40", "20", "0"],
  tables: [{ table: "ods.orders", loads: "612", failed: "11", filtered_rows: "2980", filtered_unknown: "0", width_permyriad: 10_000 }],
  phases: {
    bins: [{ lower_ms: "0", upper_ms: "10", begin_txn: "402", plan: "318", begin_txn_permyriad: 10_000, plan_permyriad: 7_900 }, { lower_ms: "500", upper_ms: "1000", begin_txn: "4", plan: "10", begin_txn_permyriad: 100, plan_permyriad: 250 }, { lower_ms: "5000", upper_ms: null, begin_txn: "0", plan: "0", begin_txn_permyriad: 0, plan_permyriad: 0 }],
    axis_ticks: ["400", "200", "0"],
    begin_txn: { samples: "642", p50_ms: "3", p95_ms: "21" },
    plan: { samples: "642", p50_ms: "6", p95_ms: "128" },
  },
  routine: {
    support: "full", running: "2", scheduling: "1", paused: "1", ended: "1", error_rows: "1284", error_rows_unknown: "0", truncated: false,
    lags: [{ job_id: "2", job_name: "events_rl", db: "dwd", table: "user_events", partition: "1", lag: "240784", state_class: "paused", consumed_message_ms: String(NOW - 2_700_000), width_permyriad: 10_000 }],
    max_running_lag: { job_id: "1", job_name: "orders_rl", db: "ods", table: "orders", partition: "3", lag: "48210", state_class: "running", consumed_message_ms: null, width_permyriad: 8_800 },
    paused_jobs: [{ job_id: "2", name: "events_rl", db: "dwd", table: "user_events", max_lag: "240784", pause_time: "2026-10-30 08:57:04", reason: "current error rows: 1272 is more than max error num: 1000" }],
  },
  limitations: [],
};

describe("load overview (design board I4)", () => {
  it("lays out the server's cards, hours, tables, lags and phases", () => {
    const html = renderToStaticMarkup(<LoadOverviewPanel nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={overview} text={chinese} />);
    expect(html).toContain("已到上限 · 最早 10-28 06:12:40");
    expect(html).toContain("1.70%");
    expect(html).toContain("其中 STREAM_LOAD 12 · INSERT 5");
    expect(html).toContain("142 ms");
    expect(html).toContain("2 / 1");
    expect(html).toContain("240,784");
    // The last hour with loads is selected, not the empty newest one.
    expect(html).toContain("10-30 06:00–07:00");
    expect(html).toContain("失败最多 <span class=\"mono\">ods.orders</span>（6）");
    // Hours before the oldest record are unknown, not buttons with zero.
    expect(html).toContain("早于最早一条，StarRocks 已不可查");
    expect(html.match(/class="load-hour"/g)).toHaveLength(3);
    expect(html).toContain("height:15%");
    expect(html).toContain("2,980 行");
    expect(html).toContain("events_rl · p1");
    expect(html).toContain("45 分钟前");
    expect(html).toContain("取事务 P50 3 ms · P95 21 ms");
    expect(html).toContain("&lt; 10 ms");
    expect(html).toContain("另有 6 次系统任务已隐藏");
    expect(html).toContain("Planned");
    // State in words, not only color; the numbers behind the phase chart.
    expect(html).toContain("events_rl 分区 1，已暂停，积压 240784");
    expect(html).toContain(">已暂停</span>");
    expect(html).toContain("查看分布数据");
    expect(html).toContain("未知：早于 StarRocks 保留的最早一条记录");
    expect(html).toContain("aria-label=\"上一小时\"");
  });

  it("draws partial hours with their held counts and says they may be incomplete", () => {
    const partial = { ...overview, complete_from: "2026-10-30 06:05:00", hours: [hour("2026-10-28 05", 0, 0, "unknown"), hour("2026-10-28 06", 12, 0, "partial"), hour("2026-10-30 06", 31, 6)] };
    const html = renderToStaticMarkup(<LoadOverviewPanel nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={partial} text={chinese} />);
    expect(html).toContain("class=\"load-hour partial\"");
    expect(html).toContain("导入 12，失败 0，可能不完整");
    expect(html).toContain("某类导入在 10-30 06:05:00 之前的记录可能已被 StarRocks 清理");
  });

  it("says when versions or data cannot show a chart", () => {
    const old = renderToStaticMarkup(<LoadOverviewPanel nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, support: "unsupported_version", routine: { ...overview.routine, support: "without_lag", lags: [], max_running_lag: null } }} text={chinese} />);
    expect(old).toContain("导入记录需要 StarRocks 3.3 及以上");
    expect(old).toContain("3.2 没有 OFFSET_LAG");
    expect(old).not.toContain("取事务 / 规划耗时分布");
    const empty = renderToStaticMarkup(<LoadOverviewPanel nowMs={NOW} onOpenHour={noop} onOpenJob={noop} onOpenTable={noop} overview={{ ...overview, hours: [], tables: [], summary: { ...overview.summary, begin_plan_p95_ms: null }, phases: { ...overview.phases, begin_txn: { samples: "3", p50_ms: null, p95_ms: null } } }} text={chinese} />);
    expect(empty).toContain("StarRocks 当前没有保留任何导入记录");
    expect(empty).toContain("没有被过滤的行");
    expect(empty).toContain("样本不足 20 条");
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
