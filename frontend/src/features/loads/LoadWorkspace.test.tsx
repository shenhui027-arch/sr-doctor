import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LoadErrorLogResponse, LoadRecordsResponse, RoutineLoadJob, RoutineLoadJobsResponse } from "../../../../contracts/generated/doctor-contracts";
import { ErrorLogBody } from "./LoadErrorLogDialog";
import { LoadRecordsPanel } from "./LoadRecordsPanel";
import { RoutineLoadPanel } from "./RoutineLoadPanel";
import { ms } from "./load-text";

const chinese = (_english: string, value: string): string => value;
const noop = () => undefined;
const NOW = 1_793_348_156_695;

const job = (id: string, name: string, state: string, stateClass: RoutineLoadJob["state_class"], lag: string | null): RoutineLoadJob => ({
  job_id: id, name, db: "ods", table: "orders", state, state_class: stateClass, create_time: "2026-10-30 08:15:05", pause_time: stateClass === "paused" ? "2026-10-30 08:57:04" : null, end_time: null,
  source_type: "KAFKA", topic: "orders", current_tasks: "1",
  statistics: { received_bytes: "44291456", received_bytes_rate: "2700000", loaded_rows: "9812004", load_rows_rate: "3120", error_rows: stateClass === "paused" ? "1272" : "12", total_rows: "9812016", unselected_rows: "0", committed_tasks: "18442", aborted_tasks: "3", task_execute_time_ms: "543" },
  max_error_number: "1000",
  partitions: [
    { partition: "0", consumed: "2451002", latest: "2451310", lag: "308", consumed_message_ms: String(NOW - 7_000) },
    { partition: "3", consumed: "2398004", latest: "2446214", lag, consumed_message_ms: String(NOW - 640_000) },
    { partition: "1", consumed: "OFFSET_ZERO", latest: "0", lag: null, consumed_message_ms: null },
  ],
  max_lag: lag, max_lag_partition: lag ? "3" : null,
  reason: stateClass === "paused" ? "ErrorReason{errCode = 102, msg='current error rows: 1272 is more than max error num: 1000'}" : null,
  other_message: null, has_error_log: true,
});

const jobs: RoutineLoadJobsResponse = {
  support: "full", fe_time_zone: "Asia/Shanghai", truncated: false, limitations: [],
  jobs: [job("1", "orders_rl", "RUNNING", "running", "48210"), job("2", "events_rl", "PAUSED", "paused", "240784")],
  summary: {
    support: "full", running: "1", scheduling: "0", paused: "1", ended: "0", error_rows: "1284", error_rows_unknown: "0", truncated: false, paused_jobs: [], max_running_lag: null,
    lags: [{ job_id: "2", job_name: "events_rl", db: "ods", table: "orders", partition: "3", lag: "240784", state_class: "paused", consumed_message_ms: null, width_permyriad: 10_000 }],
  },
};

describe("Routine Load (design board I1)", () => {
  it("counts states, flags big lag and shows the selected job's partitions", () => {
    const html = renderToStaticMarkup(<RoutineLoadPanel jobs={jobs} nowMs={NOW} onOpenLog={noop} text={chinese} />);
    expect(html).toContain("运行中");
    expect(html).toContain("1 / 0");
    expect(html).toContain("240,784");
    expect(html).toContain("ods.orders · p3");
    expect(html).toContain("current error rows: 1272 is more than max error num: 1000");
    expect(html).toContain("积压集中在分区 3");
    expect(html).toContain("OFFSET_ZERO");
    expect(html).toContain("7 秒前");
    expect(html).toContain("11 分钟前");
    expect(html).toContain("max_error_number=1000");
    expect(html).toContain("SR Doctor 只读，不提供暂停、恢复或修改作业");
    expect(html).toContain("Planned");
  });

  it("explains older versions instead of showing empty data", () => {
    expect(renderToStaticMarkup(<RoutineLoadPanel jobs={{ ...jobs, support: "unsupported_version", jobs: [] }} nowMs={NOW} onOpenLog={noop} text={chinese} />)).toContain("需要 StarRocks 3.2 及以上");
    const without = renderToStaticMarkup(<RoutineLoadPanel jobs={{ ...jobs, support: "without_lag" }} nowMs={NOW} onOpenLog={noop} text={chinese} />);
    expect(without).toContain("不可用");
  });

  it("shows unknown error rows as unknown, never as zero", () => {
    const html = renderToStaticMarkup(<RoutineLoadPanel jobs={{ ...jobs, summary: { ...jobs.summary, error_rows: "0", error_rows_unknown: "2" } }} nowMs={NOW} onOpenLog={noop} text={chinese} />);
    expect(html).toContain("2 个作业未报告，未计入");
    expect(html).toContain("累计错误行（作业生命周期）</span><strong>—</strong>");
  });
});

const records: LoadRecordsResponse = {
  support: "full", fe_time_zone: "UTC", matched: "2", hidden_system: "6", at_retention_limit: true, earliest_create_time: "2026-10-30 06:12:40", limitations: [],
  summary: { loads: "1000", failed: "17", loaded_rows: "48200000", filtered_rows: "3204", loaded_rows_unknown: "0", filtered_rows_unknown: "0", filtered_share_permyriad: 1, begin_plan_p95_ms: "142" },
  tables: [{ table: "ods.orders", loads: "612", failed: "11", filtered_rows: "2980", filtered_unknown: "0" }],
  records: [
    { job_id: "12", label: "orders_0915", db: "ods", table: "orders", kind: "STREAM_LOAD", state: "FINISHED", state_class: "finished", user: "etl_writer", client_ip: "10.2.8.14", create_time: "2026-10-30 09:15:02", finish_time: "2026-10-30 09:15:05", loaded_rows: "48002", filtered_rows: "6", filtered_share_permyriad: 1, unselected_rows: "0", begin_txn_ms: "3", plan_ms: "4", receive_data_ms: "2102", total_seconds: "3", error_message: null, has_error_log: true, profile_id: "4f4a7a79-8693-5fe0-68c3-f71b70ab9e82", system: false },
    { job_id: "11", label: "orders_0612", db: "ods", table: "orders", kind: "STREAM_LOAD", state: "CANCELLED", state_class: "cancelled", user: null, client_ip: null, create_time: "2026-10-30 06:12:40", finish_time: null, loaded_rows: null, filtered_rows: null, filtered_share_permyriad: null, unselected_rows: null, begin_txn_ms: null, plan_ms: null, receive_data_ms: null, total_seconds: null, error_message: "Label [orders_0612] has already been used.", has_error_log: false, profile_id: null, system: false },
  ],
};

describe("load records (design board I2)", () => {
  it("shows the retention window, phases in ms, totals in seconds and unknowns as dashes", () => {
    const html = renderToStaticMarkup(<LoadRecordsPanel filters={{ kind: null, outcome: null, table: "", include_system: false, hour: null }} onFilters={noop} onOpenLog={noop} records={records} text={chinese} />);
    expect(html).toContain("当前能看到的最早一条是 <strong>10-30 06:12:40</strong> · UTC（已到上限，更早的已不可查）");
    expect(html).toContain("另有 6 次系统任务已隐藏");
    expect(html).toContain("142 ms");
    expect(html).toContain("3 ms · 4 ms · 2.102 s");
    expect(html).toContain("3 s");
    expect(html).toContain("过滤 6（0.01%）");
    expect(html).toContain("Label [orders_0612] has already been used.");
    expect(html).toContain("— · — · —");
    expect(html).toContain(">错误日志<");
    expect(html).toContain("Profile · Planned");
    expect(html).toContain("10.2.8.14");
  });

  it("keeps unknown row counts out of the share", () => {
    const html = renderToStaticMarkup(<LoadRecordsPanel filters={{ kind: null, outcome: null, table: "lr.t", include_system: false, hour: "2026-10-30 06" }} onFilters={noop} onOpenLog={noop} records={{ ...records, summary: { ...records.summary, filtered_rows_unknown: "3", filtered_share_permyriad: null } }} text={chinese} />);
    expect(html).toContain("过滤行 3 条、导入行 0 条未知，未计入");
    expect(html).not.toContain("/ 48.2M · ");
    expect(html).toContain("创建于 10-30 06:00–07:00 · UTC");
    expect(html).toContain("库表包含 <span class=\"mono\">lr.t</span>");
    expect(html).toContain("aria-label=\"清除库表筛选\"");
  });

  it("explains that load records need 3.3", () => {
    expect(renderToStaticMarkup(<LoadRecordsPanel filters={{ kind: null, outcome: null, table: "", include_system: false, hour: null }} onFilters={noop} onOpenLog={noop} records={{ ...records, support: "unsupported_version", records: [] }} text={chinese} />)).toContain("需要 StarRocks 3.3 及以上");
  });

  it("formats milliseconds through the shared duration display", () => {
    expect(ms("0")).toBe("0 ms");
    expect(ms("999")).toBe("999 ms");
    expect(ms("2102")).toBe("2.102 s");
    expect(ms(null)).toBe("—");
    expect(ms("-1")).toBe("—");
  });
});

describe("error log (design board I3)", () => {
  const log: LoadErrorLogResponse = {
    job_id: "12", state: "available", truncated: false, limitations: [],
    categories: [{ category: "too_long", column: "name", target_type: "VARCHAR(5)", count: 2 }, { category: "null_in_non_nullable", column: "v", target_type: null, count: 1 }],
    entries: [
      { index: 1, category: "too_long", column: "name", target_type: "VARCHAR(5)", reason: "The field (name = name, pos = 1) is out of range. Type: VARCHAR(5), Value length: 11, Value: toolongname", row: "3\ttoolongname\t3\t<b>x</b>", row_converted: false, truncated: false },
      { index: 2, category: "null_in_non_nullable", column: "v", target_type: null, reason: "NULL value in non-nullable column 'v'", row: "[4, 'nul', NULL]", row_converted: true, truncated: true },
    ],
  };

  it("shows categories over the sample and every row as text, never markup", () => {
    const html = renderToStaticMarkup(<ErrorLogBody log={log} text={chinese} />);
    expect(html).toContain("按样本统计，不代表全部被过滤行");
    expect(html).toContain("字符串超长");
    expect(html).toContain("name · VARCHAR(5)");
    expect(html).toContain("转换后的值");
    expect(html).toContain("已截断");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
    expect(html).not.toContain("<b>x</b>");
  });

  it("says when the BE already cleaned up the file", () => {
    const html = renderToStaticMarkup(<ErrorLogBody log={{ ...log, state: "purged", entries: [], categories: [] }} text={chinese} />);
    expect(html).toContain("错误日志已被 StarRocks 清理");
    expect(html).toContain("48 小时");
  });

  it("does not call an unreachable BE a cleaned-up file", () => {
    const html = renderToStaticMarkup(<ErrorLogBody log={{ ...log, state: "unavailable", entries: [], categories: [] }} text={chinese} />);
    expect(html).toContain("文件可能仍在");
    expect(html).not.toContain("已被 StarRocks 清理");
  });
});
