import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { monitorGeneration } from "./HistoryWorkspace";
import workspaceSource from "./HistoryWorkspace.tsx?raw";
import indexHtml from "../../../index.html?raw";
import { monitorView } from "./monitor-read";
import { blockedStore, HistoryOffState, RefreshBar, TargetPicker } from "./MonitorStates";

const english = (value: string): string => value;
const noop = () => undefined;

describe("monitoring reads (audit V1-13, V1-14)", () => {
  it("never shows data read for another target or window", () => {
    const state = { scope: "a:9030|168|24", data: ["rows of a"], error: null, updatedSeconds: 1_800_000_000, loading: false };
    expect(monitorView(state, "a:9030|168|24", true).data).toEqual(["rows of a"]);
    expect(monitorView(state, "b:9030|168|24", true)).toEqual({ data: null, error: null, updatedSeconds: null, loading: true });
  });

  it("keeps the last result when a refresh fails and says how old it is", () => {
    const view = monitorView({ scope: "a", data: ["rows"], error: "history_trends_failed", updatedSeconds: 1_800_000_000, loading: false }, "a", true);
    expect(view.data).toEqual(["rows"]);
    const html = renderToStaticMarkup(<RefreshBar error={view.error} loading={false} onRefresh={noop} text={english} updatedSeconds={view.updatedSeconds} />);
    expect(html).toContain("Refresh failed (history_trends_failed); showing data from 08:00:00 UTC");
    expect(html).toContain(">Refresh<");
  });

  it("re-reads when the collector stores a slice or the alert count moves", () => {
    const status = { watermark_seconds: 1_800_000_000, open_alerts: "1" } as Parameters<typeof monitorGeneration>[0];
    expect(monitorGeneration(status)).not.toBe(monitorGeneration({ ...status!, watermark_seconds: 1_800_000_300 }));
    expect(monitorGeneration(status)).not.toBe(monitorGeneration({ ...status!, open_alerts: "2" }));
    expect(monitorGeneration(null)).toBe("");
  });

  it("names the cluster and offers a choice only when several are known", () => {
    expect(renderToStaticMarkup(<TargetPicker target="a:9030" targets={["a:9030"]} onTarget={noop} text={english} />)).toContain("<strong>a:9030</strong>");
    expect(renderToStaticMarkup(<TargetPicker target="a:9030" targets={["a:9030", "b:9030"]} onTarget={noop} text={english} />)).toContain("<option value=\"b:9030\">b:9030</option>");
  });
});

describe("history off (audit V1-17)", () => {
  it("stops at the store state or a history_disabled answer", () => {
    expect(blockedStore({ state: "disabled" }, null)).toBe("disabled");
    expect(blockedStore(undefined, "history_disabled")).toBe("disabled");
    expect(blockedStore({ state: "unavailable" }, null)).toBe("unavailable");
    expect(blockedStore({ state: "available", retention_days: 30, file_bytes: null, templates: "0", buckets: "0" }, "history_trends_failed")).toBeNull();
  });

  it("explains how to turn history on and what still works", () => {
    const html = renderToStaticMarkup(<HistoryOffState onOpenQueries={noop} onRetry={noop} store="disabled" text={english} />);
    expect(html).toContain("History monitoring is off in this setup");
    expect(html).toContain("without DOCTOR_HISTORY=off");
    expect(html).toContain("Profile import, diagnosis and Compare work without it.");
    expect(html).toContain(">Check again<");
    expect(html).toContain(">Go to queries<");
    expect(html).not.toContain("history_disabled");
  });
});

describe("clearing history (audit V1-16)", () => {
  it("names alerts, governance items and pinned snapshots in the confirmation", () => {
    expect(workspaceSource).toContain("全部告警、治理项及其处理记录和钉住的基线快照，且无法恢复");
    expect(workspaceSource).toContain("every alert, governance item with its record, and pinned baseline snapshot");
  });
});

describe("page icon (audit health note)", () => {
  it("is inline, so the local API is never asked for /favicon.ico", () => {
    expect(indexHtml).toContain("<link rel=\"icon\" href=\"data:image/svg+xml,");
  });
});

describe("unopenable history store (task 21 reasons)", () => {
  it("tells a manual migration decision apart from a folder problem and shows the code", () => {
    const manual = renderToStaticMarkup(<HistoryOffState detail={{ state: "unavailable", reason: "migration_requires_manual_resolution", code: "history_store:duplicate_active_governance_items:manual_resolution_required" }} onRetry={noop} store="unavailable" text={english} />);
    expect(manual).toContain("one template has two open governance items");
    expect(manual).toContain("<code>history_store:duplicate_active_governance_items:manual_resolution_required</code>");
    const folder = renderToStaticMarkup(<HistoryOffState detail={{ state: "unavailable", reason: "io_or_permission", code: "history_store:io" }} onRetry={noop} store="unavailable" text={english} />);
    expect(folder).toContain("exists and is writable");
    expect(folder).not.toContain("governance items");
    // An older backend sends no reason: the generic advice still applies.
    expect(renderToStaticMarkup(<HistoryOffState detail={{ state: "unavailable" }} onRetry={noop} store="unavailable" text={english} />)).toContain("prints the error code");
  });
});
