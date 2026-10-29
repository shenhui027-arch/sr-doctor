import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { HistoryLoadDay, HistoryLoadHeatCell, HistoryLoadResponse } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { AuditHistoryLoadView } from "./AuditHistoryLoad";
import source from "./AuditHistoryLoad.tsx?raw";

const english: LocalizedText = (en) => en;
const chinese: LocalizedText = (_en, zh) => zh;
const monday = 1_792_972_800;

function day(index: number, coverage: HistoryLoadDay["coverage"], executions: string | null, segment = 0): HistoryLoadDay {
  return {
    start_seconds: monday + index * 86400,
    coverage,
    complete_hours: coverage === "complete" ? 24 : 0,
    partial_hours: coverage === "partial" ? 12 : 0,
    not_collected_hours: coverage === "not_collected" ? 24 : 12,
    executions,
    errors: executions === null ? null : "1",
    query_time_sum: null,
    segment,
    executions_permyriad: executions === null ? null : 5000,
    errors_permyriad: executions === null ? null : 10000,
    query_time_permyriad: null,
  };
}

const heatmap: HistoryLoadHeatCell[] = Array.from({ length: 7 * 24 }, (_, index) => ({
  weekday: Math.floor(index / 24),
  hour: index % 24,
  collected_hours: index === 5 ? 0 : 2,
  average_executions: index === 5 ? null : "120",
  level: index === 5 ? null : 3,
}));

const response: HistoryLoadResponse = {
  target: "127.0.0.1:9030",
  start_seconds: monday,
  end_seconds: monday + 14 * 86400,
  days: [day(0, "complete", "1200"), day(1, "not_collected", null), ...Array.from({ length: 5 }, (_, index) => day(index + 2, "complete", "1000")), day(7, "complete", "0", 1), ...Array.from({ length: 5 }, (_, index) => day(index + 8, "complete", "900", 1)), day(13, "partial", "300", 1)],
  segments: [{ engine_versions: ["3.5.13"], rule_versions: ["r1"], first_day_seconds: monday }, { engine_versions: ["3.5.14"], rule_versions: ["r1"], first_day_seconds: monday + 7 * 86400 }],
  heatmap,
  week: [
    { current_day_seconds: monday + 8 * 86400, previous_day_seconds: monday + 86400, current_executions: "900", previous_executions: null, comparison: { state: "not_comparable", reason: "partial_coverage" } },
    { current_day_seconds: monday + 9 * 86400, previous_day_seconds: monday + 2 * 86400, current_executions: "900", previous_executions: "1000", comparison: { state: "not_comparable", reason: "version_or_rule_changed" } },
    { current_day_seconds: monday + 10 * 86400, previous_day_seconds: monday + 3 * 86400, current_executions: "900", previous_executions: "0", comparison: { state: "exact", delta: "900", permille: null } },
    { current_day_seconds: monday + 11 * 86400, previous_day_seconds: monday + 4 * 86400, current_executions: "900", previous_executions: "1000", comparison: { state: "exact", delta: "-100", permille: -100 } },
  ],
  axes: { executions: [{ y_permyriad: 0, value: { kind: "count", count: "0" } }, { y_permyriad: 10000, value: { kind: "count", count: "1200" } }], errors: [], query_time: [] },
  collected_hours: 300,
  expected_hours: 324,
  rows_read: "12000",
  rows_attributed: "11800",
  executions: "11400",
  errors: "13",
  limitations: ["hours_not_collected_are_never_interpolated", "weekday_and_hour_are_utc"],
};

describe("AuditHistoryLoadView", () => {
  it("draws collected, partial and not-collected days distinctly and marks the version change", () => {
    const html = renderToStaticMarkup(<AuditHistoryLoadView response={response} text={english} />);
    expect(html).toContain("Partial · 300 / 324 hours collected");
    expect(html).toContain("11,400");
    expect(html).toContain("Daily executions · 14 days");
    expect(html.match(/class="recent"/g)?.length).toBe(1);
    expect(html.match(/class="gap"/g)?.length).toBe(1);
    // The 0-execution day is a collected real zero: a bar, not a gap.
    expect(html).toContain('class="bar" height="98"');
    expect(html).toContain("3.5.14 · not_comparable");
    expect(html).toContain("audit-history-before");
  });

  it("keeps missing heatmap cells and non-comparable weeks explicit", () => {
    const html = renderToStaticMarkup(<AuditHistoryLoadView response={response} text={english} />);
    expect(html.match(/<td class="missing"/g)?.length).toBe(1);
    expect(html).toContain("Mon 05:00 · not collected");
    expect(html).toContain("partial · not compared");
    expect(html).toContain(">not_comparable<");
    expect(html).toContain("900 · no %");
    expect(html).toContain("−10.0%");
  });

  it("says why there is nothing to show without a history target, in Chinese too", () => {
    const html = renderToStaticMarkup(<AuditHistoryLoadView response={{ ...response, target: null }} text={chinese} />);
    expect(html).toContain("这个集群还没有历史数据");
    expect(html).toContain("G1，待验证");
    expect(source).not.toContain("innerHTML");
  });
});
