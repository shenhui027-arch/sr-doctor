import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LoadAlertView, LoadAlertsResponse } from "../../../../contracts/generated/doctor-contracts";
import { LoadAlertsPanel, loadAlertTitle, ratePercent } from "./LoadAlertsPanel";
import { loadViewAt } from "./MonitoringWorkspaces";

const chinese = (_english: string, value: string): string => value;
const noop = () => undefined;
const T = 1_793_448_000;

const base = (id: string, state: LoadAlertView["state"], severity: LoadAlertView["severity"], detail: LoadAlertView["detail"]): LoadAlertView => ({
  alert_id: id, rule: detail.rule, state, severity, muted_until_seconds: null, first_seen_seconds: T - 7_200, last_seen_seconds: T, resolved_seconds: state === "resolved" ? T : null,
  detail, engine_version: "3.5.14", rule_version: "history-rules.v3",
});

const failure = base("2", "open", "medium", {
  rule: "failure_rate", database: "ods", table: "orders", kind: "stream_load",
  current: { start_seconds: T - 10_800, end_seconds: T, total: "186", part: "22", rate_ppm: 118_279, collected_hours: 3 },
  baseline: { start_seconds: T - 10_800 - 604_800, end_seconds: T - 10_800, total: "10420", part: "43", rate_ppm: 4_126, collected_hours: 166 },
  delta_ppm: 114_153, ratio_permille: 28_667,
});
const paused = base("1", "open", "high", {
  rule: "routine_paused", job_id: "10276", name: "events_rl", database: "dwd", table: "user_events", observed_seconds: T - 300,
  reason: "ErrorReason{errCode = 102, msg='current error rows is more than max error num'}", max_lag: "240784",
});
const lag = base("3", "acknowledged", "medium", {
  rule: "lag_growing", job_id: "10300", name: "clicks_rl", database: "ods", table: "clicks_rt", ratio_permille: 3_429,
  hours: [18_200, 29_950, 44_100, 62_410].map((value, index) => ({ start_seconds: T - (4 - index) * 3_600, max_lag: String(value), height_permyriad: Math.round((value / 62_410) * 10_000) })),
});

const response = (alerts: LoadAlertView[]): LoadAlertsResponse => ({
  target: "127.0.0.1:9030", alerts, list_limit: 500,
  counts: { open: String(alerts.filter((a) => a.state === "open").length), acknowledged: String(alerts.filter((a) => a.state === "acknowledged").length), resolved: "0", muted: "0" },
  thresholds: { current_hours: 3, baseline_hours: 168, mute_hours: 24, lag_hours: 3, lag_min: "10000", lag_high: "1000000", failure_min_loads: 20, failure_min_baseline_loads: 100, failure_min_ppm: 50_000, failure_high_ppm: 200_000, filter_min_rows: 1_000, filter_min_baseline_rows: 10_000, filter_min_ppm: 10_000, filter_high_ppm: 100_000, calibrated: false },
  limitations: [],
});

describe("load alerts (design boards I12-I13)", () => {
  it("lists load alerts and shows the failure-rate evidence from the server", () => {
    const html = renderToStaticMarkup(<LoadAlertsPanel alerts={response([paused, failure, lag])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId="2" text={chinese} />);
    expect(html).toContain("试运行");
    expect(html).toContain("Routine Load 暂停");
    expect(html).toContain("events_rl · dwd.user_events");
    expect(html).toContain("失败率突增 · ods.orders · Stream Load");
    expect(html).toContain("11.8%（基线 0.41%）");
    expect(html).toContain("10,420");
    expect(html).toContain("166 小时已采集");
    expect(html).toContain("比基线高 11.4 个百分点，是基线的 28.7 倍");
    expect(html).toContain("历史库只存次数，不存失败原因");
    expect(html).toContain("在导入历史中查看 ods.orders");
    expect(html).toContain("静默此告警 24 小时");
    expect(html).toContain("标记已确认");
    expect(html).toContain("规则试运行");
    expect(html).toContain("版本与规则");
  });

  it("shows the pause reason in the app and the lag hours", () => {
    const pausedHtml = renderToStaticMarkup(<LoadAlertsPanel alerts={response([paused])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId="1" text={chinese} />);
    expect(pausedHtml).toContain("current error rows is more than max error num");
    expect(pausedHtml).toContain("不代表真实切换时刻");
    expect(pausedHtml).toContain("原因原文只在这里显示，不进通知");
    expect(pausedHtml).toContain("240,784 条消息");
    expect(pausedHtml).toContain("本小时观测到的最大值");
    expect(pausedHtml).toContain("观测状态");
    const lagHtml = renderToStaticMarkup(<LoadAlertsPanel alerts={response([lag])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId="3" text={chinese} />);
    expect(lagHtml).toContain("62,410 条消息");
    expect(lagHtml).toContain("每小时最大分区积压（条消息）");
    expect(lagHtml).toContain("是 3 小时前的 3.4 倍");
    expect(lagHtml).toContain("查看作业的 30 天历史");
  });

  it("groups big counts as text, never through a float", () => {
    const huge = base("9", "open", "high", { ...failure.detail, current: { ...(failure.detail as Extract<LoadAlertView["detail"], { rule: "failure_rate" }>).current, total: "9007199254740993" } } as LoadAlertView["detail"]);
    const html = renderToStaticMarkup(<LoadAlertsPanel alerts={response([huge])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId="9" text={chinese} />);
    expect(html).toContain("9,007,199,254,740,993");
  });

  it("says a missing ratio is unavailable unless the baseline has none", () => {
    const detail = failure.detail as Extract<LoadAlertView["detail"], { rule: "failure_rate" }>;
    const overflow = base("7", "open", "high", { ...detail, ratio_permille: null });
    const html = renderToStaticMarkup(<LoadAlertsPanel alerts={response([overflow])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId="7" text={chinese} />);
    expect(html).toContain("倍数不可用");
    expect(html).not.toContain("基线里没有");
    const zero = base("8", "open", "high", { ...detail, baseline: { ...detail.baseline, part: "0", rate_ppm: 0 }, ratio_permille: null });
    expect(renderToStaticMarkup(<LoadAlertsPanel alerts={response([zero])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId="8" text={chinese} />)).toContain("基线里没有，不计倍数");
  });

  it("offers more of a state when the list holds fewer than its count", () => {
    const many = { ...response([paused]), counts: { open: "501", acknowledged: "0", resolved: "0", muted: "0" } };
    const html = renderToStaticMarkup(<LoadAlertsPanel alerts={many} busy={false} onAction={noop} onMore={noop} onOpenHistory={noop} onSelect={noop} selectedId="1" text={chinese} />);
    expect(html).toContain("显示最近 1 条，共 501 条");
    expect(html).toContain("再显示 500 条");
    // The parent keeps the tab, so a reload after "show more" stays on it.
    const muted = { ...response([paused, { ...lag, alert_id: "4", state: "muted" as const }]), counts: { open: "1", acknowledged: "0", resolved: "0", muted: "1001" } };
    const kept = renderToStaticMarkup(<LoadAlertsPanel alerts={muted} busy={false} onAction={noop} onMore={noop} onOpenHistory={noop} onSelect={noop} selectedId={null} tab="muted" text={chinese} />);
    expect(kept).toContain("显示最近 1 条，共 1001 条");
    expect(kept).not.toContain("events_rl · dwd.user_events");
  });

  it("drops the chosen tab and show-more when the cluster changes", () => {
    const view = { target: "a:9030", tab: "muted" as const, more: { state: "muted" as const, limit: 1000 } };
    expect(loadViewAt(view, "a:9030")).toBe(view);
    const onB = loadViewAt(view, "b:9030");
    expect(onB).toEqual({ target: "b:9030", tab: null, more: null });
    // A to B and back: the view of A is gone, not restored.
    expect(loadViewAt(onB, "a:9030")).toEqual({ target: "a:9030", tab: null, more: null });
  });

  it("names every rule and formats small rates", () => {
    expect(loadAlertTitle({ rule: "routine_stopped", job_id: "1", name: "x", database: "d", table: "t", state: "CANCELLED", observed_seconds: T, reason: null }, chinese)).toBe("Routine Load 已结束（CANCELLED）");
    expect(ratePercent(80)).toBe("0.008%");
    expect(ratePercent(0)).toBe("0%");
    const empty = renderToStaticMarkup(<LoadAlertsPanel alerts={response([])} busy={false} onAction={noop} onOpenHistory={noop} onSelect={noop} selectedId={null} text={chinese} />);
    expect(empty).toContain("这个状态下没有导入告警");
    expect(empty).toContain("导入告警规则（试运行）");
  });
});
