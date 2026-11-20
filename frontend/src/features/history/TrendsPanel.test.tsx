import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type {
  AuditMetricFact,
  HistorySeriesRequest,
  HistorySeriesResponse,
  HistoryTrendsRequest,
  HistoryTrendsResponse,
  TrendConfidence,
  TrendPoint,
  TrendRow,
  TrendVerdict,
} from "../../../../contracts/generated/doctor-contracts";
import { createTrendRequestTracker } from "./HistoryWorkspace";
import { changeText, confidenceText, evidenceText, severityText, sourceText, TrendChart, TrendsPanel, verdictText } from "./TrendsPanel";

const english = (value: string): string => value;
const ms = (value: number): AuditMetricFact => ({
  metric_id: "query_time",
  input: { state: "text", value: String(value) },
  state: "exact",
  parser_state: "exact",
  canonical_number: { kind: "integer", value: `${value}000000`, scale: null },
  canonical_unit: "ns",
});
const side = (median: number | null, executions: string) => ({
  median: median === null ? null : ms(median),
  mad: median === null ? null : ms(2),
  executions,
  valid_samples: executions,
  hours: median === null ? 0 : 24,
  thin_hours: 0,
});
const row = (key: string, verdict: TrendRow["verdict"], base: number | null, now: number, relative: string | null): TrendRow => ({
  template_key: key,
  kind: key === "a" ? "product_fingerprint" : "official_digest",
  template: `SELECT ${key} FROM t WHERE id = ?`,
  catalog: "default_catalog",
  database: "demo",
  verdict,
  confidence: verdict.state === "regression" ? "high" : null,
  evidence: verdict.state === "not_comparable" ? "unavailable" : "complete",
  baseline: side(base, "120"),
  current: side(now, "40"),
  change: base === null ? null : { direction: now >= base ? "up" : "down", magnitude: ms(Math.abs(now - base)), relative_percent: relative },
  comparability: { engine_version: "3.5.14", rule_version: "sr-doctor-fingerprint.v1" },
  limitations: [],
});
const rule = { k_mad: "3", min_relative_percent: 20, min_baseline_hours: 6, min_hour_samples: 3, min_current_samples: 10, calibrated: false };
const window = { current_start_seconds: 1_800_000_000, baseline_start_seconds: 1_799_395_200, end_seconds: 1_800_086_400 };
const trends: HistoryTrendsResponse = {
  target: "127.0.0.1:9030",
  targets: ["127.0.0.1:9030"],
  window,
  rule,
  truncated: false,
  rows: [
    row("a", { state: "regression", severity: "high", workload: "increased" }, 100, 250, "150.0"),
    row("b", { state: "not_comparable", reason: "engine_version_changed" }, null, 100, null),
    row("c", { state: "unscored", reason: "zero_baseline" }, 0, 50, null),
  ],
  counts: { regression: "1", improvement: "0", unscored: "41", stable: "212", not_comparable: "1" },
  limitations: [],
};
const point = (hour: number, coverage: TrendPoint["coverage"], median: number | null, thin = false): TrendPoint => ({
  hour_start_seconds: 1_800_000_000 + hour * 3600,
  coverage,
  executions: coverage === "not_collected" ? null : "5",
  median: median === null ? null : ms(median),
  valid_samples: "5",
  thin,
  comparable: true,
});
const series: HistorySeriesResponse = {
  target: "127.0.0.1:9030",
  window,
  rule,
  row: trends.rows[0],
  points: [point(0, "complete", 100), point(1, "complete", 104), point(2, "not_collected", null), point(3, "complete", 98, true), point(4, "partial", 250), point(5, "complete", 260)],
  exemplars: [{ query_id: "3b7e19c2-8a10", query_time: ms(1960), event_seconds: 1_800_018_000 }],
};

describe("template trends (design board 02-overview)", () => {
  it("states verdicts and changes without a percentage when the baseline is 0", () => {
    expect(verdictText(trends.rows[0].verdict, english)).toBe("↑ Regression");
    expect(verdictText(trends.rows[1].verdict, english)).toBe("StarRocks version changed · not comparable yet");
    expect(verdictText(trends.rows[2].verdict, english)).toBe("Baseline is 0 · absolute change only");
    expect(changeText(trends.rows[0])).toBe("+150 ms · +150.0%");
    expect(changeText(trends.rows[2])).toBe("+50 ms");
    expect(changeText(trends.rows[1])).toBe("—");
  });

  it("lists severity, confidence and evidence separately and the calibration status", () => {
    const html = renderToStaticMarkup(
      <TrendsPanel baselineHours={168} currentHours={24} onSelect={() => undefined} onWindow={() => undefined} selectedKey="a" series={series} text={english} trends={trends} />,
    );
    expect(html).toContain("Severity high");
    expect(html).toContain("Confidence high");
    expect(html).toContain("Evidence unavailable");
    expect(html).toContain("Rule on trial");
    expect(html).toContain("212 within their baseline band · 41 with too few samples · 1 not comparable");
    // The typed state stays reachable as a tooltip; the visible text is plain language.
    expect(html).toContain('title="not_comparable · engine_version_changed"');
    expect(html).not.toMatch(/>(unscored|not_comparable) ·/);
    expect(html).not.toMatch(/computed by the server|G2/);
    expect(html).not.toMatch(/health score|\/\s*100\b/i);
    // Without a connected session to this target the pull action is withheld.
    expect(html).toContain("Connect to this cluster to pull");
    expect(html).not.toContain("Pull Profile and diagnose");
    const connected = renderToStaticMarkup(
      <TrendsPanel baselineHours={168} currentHours={24} onPullProfile={() => undefined} onSelect={() => undefined} onWindow={() => undefined} selectedKey="a" series={series} text={english} trends={trends} />,
    );
    expect(connected).toContain("Pull Profile and diagnose");
  });

  it("breaks the line at a not-collected hour and marks thin and partial hours", () => {
    const html = renderToStaticMarkup(<TrendChart series={series} text={english} />);
    expect(html.match(/<polyline/g)).toHaveLength(2);
    expect(html).toContain("trend-gap not_collected");
    expect(html).toContain("trend-gap partial");
    expect(html).toContain("trend-dot thin");
    expect(html.match(/<circle/g)).toHaveLength(5);
  });

  it("puts the list first and keeps rule details and extra executions folded (audit V1-18, V1-19)", () => {
    const many = {
      ...series,
      exemplars: [0, 1, 2, 3, 4].map((index) => ({ query_id: `q-${index}`, query_time: ms(100 + index), event_seconds: 1_800_018_000 + index })),
    };
    const html = renderToStaticMarkup(
      <TrendsPanel baselineHours={168} currentHours={24} onSelect={() => undefined} onWindow={() => undefined} selectedKey="a" series={many} text={english} trends={trends} />,
    );
    expect(html.indexOf('id="trend-list"')).toBeLessThan(html.indexOf('id="trend-detail"'));
    expect(html).toContain('<details class="disclosure history-details"><summary>How this is judged</summary>');
    expect(html.match(/<li><code>q-/g)).toHaveLength(3);
    expect(html).toContain(">Show all 5<");
    expect(html).toContain("Last 6 hours (UTC)");
  });

  it("falls back safely to the raw code when backend trend enums are unknown (correctness fix #3)", () => {
    const unknownRow: TrendRow = {
      ...trends.rows[0],
      kind: "future_digest_kind",
      verdict: { state: "regression", severity: "extreme" as TrendRow["verdict"] extends { severity: infer S } ? S : never, workload: "increased" } as TrendVerdict,
      confidence: "certain" as unknown as TrendConfidence,
      evidence: "provisional" as unknown as TrendRow["evidence"],
    };
    expect(severityText(unknownRow, english)).toBe("extreme");
    expect(confidenceText(unknownRow, english)).toBe("certain");
    expect(evidenceText(unknownRow, english)).toBe("provisional");
    expect(sourceText(unknownRow, english)).toBe("future_digest_kind");
    expect(verdictText({ state: "not_comparable", reason: "cluster_topology_changed" as never }, english)).toBe("cluster_topology_changed");
    expect(verdictText({ state: "unscored", reason: "new_unscored_reason" as never }, english)).toBe("new_unscored_reason");
    expect(verdictText({ state: "brand_new_verdict" } as unknown as TrendVerdict, english)).toBe("brand_new_verdict");
  });

  it("aborts superseded and unmounted trend fetches so late responses never overwrite the view (correctness fix #2)", async () => {
    const trendResolvers: Array<(value: HistoryTrendsResponse) => void> = [];
    const trendSignals: AbortSignal[] = [];
    const seriesResolvers: Array<(value: HistorySeriesResponse) => void> = [];
    const seriesSignals: AbortSignal[] = [];

    const tracker = createTrendRequestTracker({
      readHistoryTrends(_req: HistoryTrendsRequest, signal?: AbortSignal) {
        if (signal) trendSignals.push(signal);
        return new Promise<HistoryTrendsResponse>((resolve) => {
          trendResolvers.push(resolve);
        });
      },
      readHistorySeries(_req: HistorySeriesRequest, signal?: AbortSignal) {
        if (signal) seriesSignals.push(signal);
        return new Promise<HistorySeriesResponse>((resolve) => {
          seriesResolvers.push(resolve);
        });
      },
    });

    const acceptedTargets: string[] = [];
    const firstTrend = tracker.fetchTrends(
      { target: "first:9030", current_hours: 24, baseline_hours: 168, limit: 100 },
      (res) => acceptedTargets.push(res.target ?? ""),
      () => undefined,
    );
    const secondTrend = tracker.fetchTrends(
      { target: "second:9030", current_hours: 24, baseline_hours: 168, limit: 100 },
      (res) => acceptedTargets.push(res.target ?? ""),
      () => undefined,
    );

    expect(trendSignals[0]?.aborted).toBe(true);
    expect(trendSignals[1]?.aborted).toBe(false);

    // Resolve the superseded first response AFTER the second request already started.
    trendResolvers[0]({ ...trends, target: "first:9030" });
    trendResolvers[1]({ ...trends, target: "second:9030" });
    await Promise.all([firstTrend, secondTrend]);
    expect(acceptedTargets).toEqual(["second:9030"]);

    // Start an in-flight series request, then abort on unmount.
    const acceptedSeries: string[] = [];
    const pendingSeries = tracker.fetchSeries(
      { target: "second:9030", template_key: "a", current_hours: 24, baseline_hours: 168, points: 48 },
      (res) => acceptedSeries.push(res.row.template_key),
      () => undefined,
    );
    expect(seriesSignals[0]?.aborted).toBe(false);
    tracker.abortAll();
    expect(seriesSignals[0]?.aborted).toBe(true);
    seriesResolvers[0](series);
    await pendingSeries;
    expect(acceptedSeries).toEqual([]);
  });
});
