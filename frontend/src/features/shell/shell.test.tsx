import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { Comparison, HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { initialConnectionState } from "../connection/connection-state";
import { CollectionCapsule, collectionLabel } from "../history/history-status";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { LocaleProvider } from "../../i18n/locale";
import { ContextBar } from "./ContextBar";
import { HeaderStatusText } from "./HeaderStatusText";
import { TopBar } from "./TopBar";

const makeHistoryStatus = (overrides: Partial<HistoryStatusResponse> = {}): HistoryStatusResponse => ({
  store: { state: "available", retention_days: 30, file_bytes: "1048576", templates: "10", buckets: "24" },
  target: "127.0.0.1:9030",
  collection: { state: "running", next_run_seconds: 1_800_000_300 },
  watermark_seconds: 1_800_000_000,
  budget: {
    period_seconds: 300,
    slice_seconds: 300,
    lag_seconds: 120,
    slice_row_limit: 1000,
    query_timeout_seconds: 15,
    catch_up_seconds: 86_400,
    production_budget_measured: false,
  },
  coverage: [],
  runs: [],
  remote_collection_enabled: false,
  open_alerts: "3",
  load_revision: "1",
  limitations: [],
  ...overrides,
});

describe("Unknown-enum safety and HeaderStatusText", () => {
  const sampleMap = {
    alpha: ["Alpha label", "甲标签"],
    beta: ["Beta label", "乙标签"],
  } as const;

  it("maps known enum keys and falls back safely for unknown, prototype, or empty values", () => {
    const en = (enText: string) => enText;
    const zh = (_enText: string, zhText: string) => zhText;

    expect(lookupEnumLabel(sampleMap, "alpha", en)).toBe("Alpha label");
    expect(lookupEnumLabel(sampleMap, "alpha", zh)).toBe("甲标签");
    expect(lookupEnumLabel(sampleMap, "brand_new_backend_reason", en)).toBe("brand_new_backend_reason");
    expect(lookupEnumLabel(sampleMap, "brand_new_backend_reason", zh)).toBe("brand_new_backend_reason");
    expect(lookupEnumLabel(sampleMap, "toString", en)).toBe("toString");
    expect(lookupEnumLabel(sampleMap, "", en)).toBe("Unknown");
    expect(lookupEnumLabel(sampleMap, "", zh)).toBe("未知");
    expect(lookupEnumLabel(sampleMap, undefined as unknown as string, zh)).toBe("未知");
  });

  it("handles empty or non-string values in HeaderStatusText without throwing", () => {
    expect(renderToStaticMarkup(<HeaderStatusText value="" />)).toBe("");
    expect(renderToStaticMarkup(<HeaderStatusText value={undefined} />)).toBe("");
    expect(renderToStaticMarkup(<HeaderStatusText value={null} />)).toBe("");
    expect(renderToStaticMarkup(<HeaderStatusText value={42 as unknown as string} />)).toBe("");
    expect(renderToStaticMarkup(<HeaderStatusText value="已连接 · 3.5.14 · 只读" />))
      .toBe('已连接<span class="header-detail"> · 3.5.14 · 只读</span>');
  });

  it("renders TopBar and CollectionCapsule normally when HistoryStatusResponse.collection has an unknown reason or state", () => {
    const unknownStopped = makeHistoryStatus({
      collection: { state: "stopped", reason: "future_cluster_maintenance" as never },
    });
    const unknownState = makeHistoryStatus({
      collection: { state: "future_throttled_mode" as never },
    });

    for (const locale of ["en", "zh"] as const) {
      const label = collectionLabel(
        unknownStopped,
        (enText, zhText) => (locale === "en" ? enText : zhText),
      );
      expect(typeof label).toBe("string");
      expect(label).toBe("future_cluster_maintenance");

      const topBarHtml = renderToStaticMarkup(
        <LocaleProvider initialLocale={locale}>
          <TopBar
            connected={false}
            connection={initialConnectionState}
            dialogOpen={false}
            firstRun={false}
            hasDoctorReport={false}
            historyStatus={unknownStopped}
            onImport={() => undefined}
            onOpenCollection={() => undefined}
            onOpenConnection={() => undefined}
            onOpenSettings={() => undefined}
            onSelectWorkspace={() => undefined}
            openAlerts={3}
            pausedLoadCount={1}
            workspace="Overview"
          />
        </LocaleProvider>,
      );
      expect(topBarHtml).toContain("future_cluster_maintenance");

      const capsuleHtml = renderToStaticMarkup(
        <LocaleProvider initialLocale={locale}>
          <CollectionCapsule onOpen={() => undefined} status={unknownState} />
        </LocaleProvider>,
      );
      expect(capsuleHtml).toContain("future_throttled_mode");
    }
  });
});

describe("TopBar and persistent ContextBar (boards/00-shell)", () => {
  it("renders single-row TopBar with 3 primary items, collapsible Alerts/Loads + More menu, counts, and conditional Demo badge", () => {
    const readyConnection = {
      phase: "ready" as const,
      session: sr3514ConnectedContractGolden.connection,
      message: null,
    };

    const html = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <TopBar
          connected
          connection={readyConnection}
          dialogOpen={false}
          firstRun={false}
          hasDoctorReport
          historyStatus={makeHistoryStatus()}
          onImport={() => undefined}
          onOpenCollection={() => undefined}
          onOpenConnection={() => undefined}
          onOpenSettings={() => undefined}
          onSelectWorkspace={() => undefined}
          openAlerts={3}
          pausedLoadCount={1}
          workspace="Doctor"
        />
      </LocaleProvider>,
    );

    expect(html).toContain('class="app-header"');
    expect(html).toContain("<strong>SR Doctor</strong>");
    expect(html).toContain('aria-current="page" class="nav-link is-active"');
    // Alerts (3) and Loads (1) collapse into More (4) below 1024px.
    expect(html).toContain("nav-link nav-item--collapse-md");
    expect(html).toContain('class="nav-more-wrap"');
    expect(html).toContain("<span>更多</span><span class=\"nav-count nav-count--alert\">4</span>");
    expect(html).toContain('class="conn-pill"');
    expect(html).toContain("btn btn-secondary btn-import-top");
    expect(html).not.toContain("Demo / Mock cluster");

    // Demo mode explicitly shows the Demo / Mock cluster stamp; first-run hides Import Profile and disables Doctor.
    const firstRunDemoHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="en">
        <TopBar
          connected={false}
          connection={initialConnectionState}
          demoMode
          dialogOpen={false}
          firstRun
          hasDoctorReport={false}
          historyStatus={null}
          onImport={() => undefined}
          onOpenCollection={() => undefined}
          onOpenConnection={() => undefined}
          onOpenSettings={() => undefined}
          onSelectWorkspace={() => undefined}
          openAlerts={0}
          pausedLoadCount={0}
          workspace="Queries"
        />
      </LocaleProvider>,
    );
    expect(firstRunDemoHtml).toContain("Demo / Mock cluster");
    expect(firstRunDemoHtml).not.toContain("btn-import-top");
    expect(firstRunDemoHtml).toMatch(/<button[^>]*\bdisabled\b[^>]*><span>Doctor<\/span>/);
  });

  it("mounts ContextBar with anchored Diagnosis | Compare SegTabs in both diagnosis and compare modes", () => {
    const diagnosisHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <ContextBar
          actions={<button type="button">导出报告</button>}
          inputOrigin="connected_profile_fetch"
          mode="diagnosis"
          observedAtUtc="2026-11-19T14:02:17Z"
          onModeChange={() => undefined}
          onReturnToQueries={() => undefined}
          report={sr3514JsonMinimal}
        />
      </LocaleProvider>,
    );

    expect(diagnosisHtml).toContain('class="context-bar"');
    expect(diagnosisHtml).toContain('class="context-anchor"');
    expect(diagnosisHtml).toContain("seg-tabs has-pill seg-tabs--sm");
    expect(diagnosisHtml).toContain("‹ 查询列表");
    expect(diagnosisHtml).toContain("StarRocks");
    expect(diagnosisHtml).toContain("14:02:17 UTC");
    expect(diagnosisHtml).toContain("导出报告");

    const comparison: Comparison = {
      schema_version: "0.1",
      comparison_receipt: { components: [] },
      comparison_key: "cmp-key-1",
      baseline: { outcome: { report: sr3514JsonMinimal } },
      candidate: { outcome: { report: sr3514JsonMinimal } },
      state: "partially_comparable",
      reasons: [],
      compatibility_delta: [],
      structure_delta: { state: "matched", matches: [], added: [], removed: [], changed: [], edge_delta: [], unresolved: [] },
      metric_deltas: [],
      finding_delta: { added: [], resolved: [], persisting: [], changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [] },
      diagnostics: [],
      limitations: [],
    };

    const compareHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <ContextBar
          actions={<button type="button">导出对比报告</button>}
          comparison={comparison}
          mode="compare"
          onModeChange={() => undefined}
        />
      </LocaleProvider>,
    );

    expect(compareHtml).toContain('class="context-anchor"');
    expect(compareHtml).toContain("seg-tabs has-pill seg-tabs--sm");
    expect(compareHtml).toContain("部分可比");
    expect(compareHtml).toContain("导出对比报告");
  });
});
