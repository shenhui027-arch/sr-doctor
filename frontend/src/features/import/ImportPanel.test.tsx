import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider } from "../../i18n/locale";
import { createImportRequestTracker, ImportPanel, runTrackedImport, type ImportClient } from "./ImportPanel";
import { importReducer, initialImportState, type ImportAction } from "./import-state";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("ImportPanel", () => {
  it("renders a card--form with format SegTabs and fixed footer submit button", () => {
    const html = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <ImportPanel maxInputBytes={16 * 1024 * 1024} onReport={() => undefined} />
      </LocaleProvider>,
    );
    expect(html).toContain("card--form");
    expect(html).toContain("离线导入 Profile");
    expect(html).toContain("自动识别");
    expect(html).toContain(">分析 Profile →</button>");
    expect(html).not.toContain("RequestedSourceKind");
  });

  it("aborts a superseded or unmounted offline import so a late reply never overwrites the current view (regression)", async () => {
    const first = deferred<AnalysisSnapshotReport>();
    const second = deferred<AnalysisSnapshotReport>();
    const signals: Array<AbortSignal | undefined> = [];
    let call = 0;

    const api: ImportClient = {
      analyzeForm: () => Promise.reject(new Error("unexpected form call")),
      analyzeJson: (_body, signal) => {
        signals.push(signal);
        call += 1;
        return call === 1 ? first.promise : second.promise;
      },
    };

    const tracker = createImportRequestTracker();
    const reports: string[] = [];
    const actions: ImportAction[] = [];
    const text = (en: string) => en;

    const state1 = importReducer(initialImportState, { type: "paste", text: "Query: slow-1" });
    const state2 = importReducer(initialImportState, { type: "paste", text: "Query: fast-2" });

    const p1 = runTrackedImport({
      api,
      dispatch: (action) => actions.push(action),
      onReport: (report) => reports.push(report.analysis_key),
      state: state1,
      text,
      tracker,
    });

    expect(signals[0]?.aborted).toBe(false);

    // Starting the second request immediately aborts the first AbortController.
    const p2 = runTrackedImport({
      api,
      dispatch: (action) => actions.push(action),
      onReport: (report) => reports.push(report.analysis_key),
      state: state2,
      text,
      tracker,
    });

    expect(signals[0]?.aborted).toBe(true);
    expect(signals[1]?.aborted).toBe(false);

    const report1 = { ...sr3514JsonMinimal, analysis_key: "stale-report-1" };
    const report2 = { ...sr3514JsonMinimal, analysis_key: "fresh-report-2" };

    second.resolve(report2);
    await p2;

    // Now the superseded first request settles late — must be ignored completely.
    first.resolve(report1);
    await p1;

    expect(reports).toEqual(["fresh-report-2"]);

    // Unmount abort: an in-flight third request aborted before settling never calls onReport or dispatches fail.
    const third = deferred<AnalysisSnapshotReport>();
    const unmountApi: ImportClient = {
      analyzeForm: () => Promise.reject(new Error("unexpected")),
      analyzeJson: (_body, signal) => {
        signals.push(signal);
        return third.promise;
      },
    };
    const p3 = runTrackedImport({
      api: unmountApi,
      dispatch: (action) => actions.push(action),
      onReport: (report) => reports.push(report.analysis_key),
      state: state2,
      text,
      tracker,
    });
    expect(signals[2]?.aborted).toBe(false);
    tracker.abort();
    expect(signals[2]?.aborted).toBe(true);
    third.resolve({ ...sr3514JsonMinimal, analysis_key: "after-unmount" });
    await p3;

    expect(reports).toEqual(["fresh-report-2"]);
  });
});
