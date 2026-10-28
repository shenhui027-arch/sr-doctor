import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ExecutionGraph } from "../../../contracts/generated/execution-graph-fixture";
import { sr3514TextMinimal } from "../../../contracts/generated/contract-fixtures";
import { DoctorWorkspace } from "../features/doctor/DoctorWorkspace";
import { ContextBar } from "../features/shell/ContextBar";
import { LocaleProvider } from "./locale";

const render = (node: Parameters<typeof DoctorWorkspace>[0]) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><DoctorWorkspace {...node} /></LocaleProvider>);
const renderContext = (node: Parameters<typeof ContextBar>[0]) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><ContextBar {...node} /></LocaleProvider>);

describe("Doctor workspace", () => {
  it("reads top-down: query summary, findings, one execution graph, node table, original Profile", () => {
    const html = render({ report: sr3514ExecutionGraph, onReport: () => undefined });
    const order = ["查询概要", 'id="findings-title"', "执行结构", 'class="doctor-details"', "Profile 原文"].map((marker) => html.indexOf(marker));
    expect(order.every((position) => position >= 0)).toBe(true);
    expect([...order].sort((left, right) => left - right)).toEqual(order);
    expect(html).toContain("总耗时");
    expect(html).toContain("8.099 s");
    expect(html).toContain("算子累计耗时");
    // No parallel projections or the retired overview/top-5 widgets.
    for (const retired of [">计划结构<", "运行时算子 ·", "Top 5", "Profile 概览"]) expect(html).not.toContain(retired);
    expect(html).not.toContain(sr3514ExecutionGraph.facts_hash);
  });

  it("shows the backend's CPU-sum limitation on the CPU fact and never infers one", () => {
    const cpuCaveat = "同一 BE 上的实例重复计入，可能重复计数，并随实例数增大。不是墙钟时间，也不是单个算子的 CPU 时间。";
    const cpu = sr3514ExecutionGraph.derived_metric_cells.find((cell) => cell.metric_id === "QueryCumulativeCpuTime");
    expect(cpu?.reasons).toEqual(["query_cpu_summed_over_instance_reports_not_wall_or_operator_time"]);
    expect(render({ report: sr3514ExecutionGraph, onReport: () => undefined })).toContain(cpuCaveat);
    // Without the backend reason the browser shows no caveat of its own.
    const withoutReason = { ...sr3514ExecutionGraph, derived_metric_cells: sr3514ExecutionGraph.derived_metric_cells.map((cell) => cell.metric_id === "QueryCumulativeCpuTime" ? { ...cell, reasons: [] } : cell) };
    expect(render({ report: withoutReason, onReport: () => undefined })).not.toContain(cpuCaveat);
  });

  it("does not invent a summary for a retained synthetic report and labels absent values as missing, not 0", () => {
    const html = render({ report: sr3514TextMinimal, onReport: () => undefined });
    expect(html).toContain("无法按节点汇总耗时");
    expect(html).not.toContain("plan-node-link");
    expect(html).toContain('class="metric-state-badge"');
    expect(html).not.toMatch(/<dd>0<\/dd>/);
  });

  it("can reopen the importer without discarding the current report", () => {
    const html = render({ importing: true, onCancelImport: () => undefined, onReport: () => undefined, report: sr3514TextMinimal });
    expect(html).toContain("导入 Profile");
    expect(html).toContain("返回当前分析");
    expect(html).not.toContain("查询概要");
    // The import page carries no empty placeholder panels.
    expect(html).not.toContain("版本与兼容性");
    expect(html).not.toContain("等待分析");
  });
});

describe("Context bar", () => {
  it("shows provenance, version and evidence from the report and returns to the list only for connected Profiles", () => {
    const connected = renderContext({ inputOrigin: "connected_profile_fetch", onReturnToQueries: () => undefined, report: sr3514ExecutionGraph });
    expect(connected).toContain("从数据库拉取");
    expect(renderContext({ inputOrigin: "connected_profile_fetch", observedAtUtc: "2026-10-21T14:02:17Z", report: sr3514ExecutionGraph })).toContain("从数据库拉取 · 14:02:17 UTC");
    expect(connected).toContain("返回查询列表");
    expect(connected).toContain("StarRocks 3.5.14");
    const offline = renderContext({ inputOrigin: "offline_import", onReturnToQueries: () => undefined, report: sr3514TextMinimal });
    expect(offline).toContain("导入的 Profile");
    expect(offline).not.toContain("返回查询列表");
  });

  it("presents a declared version as declared, not detected", () => {
    const report = { ...sr3514TextMinimal, query_summary: null, version: { ...sr3514TextMinimal.version, effective_version: "3.5.14", version_source: "user_declared" as const } };
    const html = renderContext({ inputOrigin: "offline_import", report });
    expect(html).toContain("StarRocks 3.5.14");
    expect(html).not.toContain("从数据库拉取");
  });

  it("states a Level 2 or running Profile up front with the next capture step (v0.6-04)", () => {
    const level2 = render({ report: { ...sr3514ExecutionGraph, profile_context: { ...sr3514ExecutionGraph.profile_context, profile_level: "level_2", merge_state: "unmerged" } }, onReport: () => undefined });
    expect(level2).toContain("Level 2 Profile：无法按计划节点分析");
    expect(level2).toContain("SET pipeline_profile_level = 1");
    const running = render({ report: { ...sr3514ExecutionGraph, profile_context: { ...sr3514ExecutionGraph.profile_context, query_state: "running", snapshot_kind: "unknown" } }, onReport: () => undefined });
    expect(running).toContain("运行中查询的快照：数值为临时值");
    expect(running).toContain("等查询结束后重新获取 Profile");
    // A final, merged Level 1 Profile carries no notice.
    expect(render({ report: sr3514ExecutionGraph, onReport: () => undefined })).not.toContain("profile-state-notice");
  });
});
