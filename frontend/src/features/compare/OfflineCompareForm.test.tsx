import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Comparison } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider } from "../../i18n/locale";
import { importReducer, initialImportState } from "../import/import-state";
import {
  createComparisonRequestTracker,
  OfflineCompareForm,
  runTrackedComparison,
  type ComparisonClient,
} from "./OfflineCompareForm";

function readyPair(tag: string) {
  const baseline = importReducer(initialImportState, { type: "paste", text: `baseline-${tag}` });
  const candidate = importReducer(initialImportState, { type: "paste", text: `candidate-${tag}` });
  return { baseline, candidate };
}

function fakeComparison(key: string): Comparison {
  return { comparison_key: key } as unknown as Comparison;
}

describe("OfflineCompareForm", () => {
  it("shows only the two Profile inputs with compare and clear before a comparison exists", () => {
    const html = renderToStaticMarkup(<OfflineCompareForm onComparison={vi.fn()} />);

    expect(html).toContain("Compare baseline and candidate Profiles");
    expect(html).toContain("Import the Profiles from before and after your SQL change");
    expect(html).toContain("Baseline Profile");
    expect(html).toContain("Candidate Profile");
    expect(html.indexOf("Baseline Profile")).toBeLessThan(html.indexOf("Candidate Profile"));
    expect(html).toMatch(/disabled=""[^>]*>Compare locally/);
    expect(html).toContain(">Clear both</button>");
    expect(html).not.toMatch(/Verify optimization results|Awaiting|Unknown|Export comparison report|Swap/);
    expect(html).not.toContain("compare-connected-hint");
    expect(html).not.toContain("before and after executions in Queries");
    expect(html).not.toMatch(/analysis_key|localStorage|sessionStorage/);
  });

  it("adds the Queries hint only while a connection session exists", () => {
    const html = renderToStaticMarkup(<OfflineCompareForm connected onComparison={vi.fn()} />);

    expect(html).toContain("compare-connected-hint");
    expect(html).toContain("before and after executions in Queries");
    expect(html).toContain("Reading existing Profiles does not rerun SQL.");
  });

  it("renders the same boundary in Chinese", () => {
    const offline = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><OfflineCompareForm onComparison={vi.fn()} /></LocaleProvider>);
    const connected = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><OfflineCompareForm connected onComparison={vi.fn()} /></LocaleProvider>);

    expect(offline).toContain("对比基线与候选 Profile");
    expect(offline).toContain("同一条 SQL 优化前后的 Profile");
    expect(offline).not.toContain("选择优化前后的两次执行进行对比");
    expect(connected).toContain("选择优化前后的两次执行进行对比");
    expect(connected).toContain("读取已有 Profile 不会重新执行 SQL。");
  });

  it("aborts superseded or cleared requests so a late response never overwrites the current comparison (Fix #3)", async () => {
    const tracker = createComparisonRequestTracker();
    const signals: AbortSignal[] = [];
    const resolvers: Array<(value: Comparison) => void> = [];

    const api: ComparisonClient = {
      compareForm: vi.fn((_body, signal) => {
        if (signal) signals.push(signal);
        return new Promise<Comparison>((resolve) => {
          resolvers.push(resolve);
        });
      }),
    };

    const accepted: string[] = [];
    const dispatchBaseline = vi.fn();
    const dispatchCandidate = vi.fn();

    const firstRun = runTrackedComparison({
      api,
      pair: readyPair("first"),
      dispatchBaseline,
      dispatchCandidate,
      onComparison: (item) => accepted.push(item.comparison_key),
      fallbackMessage: "failed",
      tracker,
    });

    const secondRun = runTrackedComparison({
      api,
      pair: readyPair("second"),
      dispatchBaseline,
      dispatchCandidate,
      onComparison: (item) => accepted.push(item.comparison_key),
      fallbackMessage: "failed",
      tracker,
    });

    expect(signals).toHaveLength(2);
    expect(signals[0].aborted).toBe(true);
    expect(signals[1].aborted).toBe(false);

    // Resolve the second request first, then let the stale first request arrive late.
    resolvers[1](fakeComparison("second-comparison"));
    await secondRun;
    resolvers[0](fakeComparison("stale-first-comparison"));
    await firstRun;

    expect(accepted).toEqual(["second-comparison"]);

    // Clearing or unmounting aborts an in-flight third request and ignores its late answer.
    const thirdRun = runTrackedComparison({
      api,
      pair: readyPair("third"),
      dispatchBaseline,
      dispatchCandidate,
      onComparison: (item) => accepted.push(item.comparison_key),
      fallbackMessage: "failed",
      tracker,
    });
    expect(signals[2].aborted).toBe(false);
    tracker.abort();
    expect(signals[2].aborted).toBe(true);
    resolvers[2](fakeComparison("aborted-third-comparison"));
    await thirdRun;

    expect(accepted).toEqual(["second-comparison"]);
  });
});
