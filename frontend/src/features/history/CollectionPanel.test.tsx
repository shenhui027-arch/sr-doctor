import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { CollectionPanel, collectionLabel } from "./CollectionPanel";

const english = (value: string): string => value;
const status = (overrides: Partial<HistoryStatusResponse> = {}): HistoryStatusResponse => ({
  store: { state: "available", retention_days: 30, file_bytes: "2516582", templates: "12", buckets: "40" },
  target: "127.0.0.1:19030",
  collection: { state: "running", next_run_seconds: 1_800_000_300 },
  watermark_seconds: 1_800_000_000,
  budget: { period_seconds: 300, slice_seconds: 300, lag_seconds: 120, slice_row_limit: 1000, query_timeout_seconds: 15, catch_up_seconds: 86_400, production_budget_measured: false },
  coverage: [
    { hour_start_seconds: 1_799_992_800, state: "not_collected" },
    { hour_start_seconds: 1_799_996_400, state: "partial" },
    { hour_start_seconds: 1_800_000_000, state: "complete" },
  ],
  runs: [
    { started_seconds: 1_800_000_000, slice_start_seconds: 1_799_999_580, slice_end_seconds: 1_799_999_880, state: "partial", reason: "row_limit", rows_read: "1000", attributed: "998", partial_statements: "2", unavailable_statements: "0", duration_ms: "41" },
    { started_seconds: 1_799_999_700, slice_start_seconds: 1_799_999_280, slice_end_seconds: 1_799_999_580, state: "gap", reason: "beyond_catch_up", rows_read: "0", attributed: "0", partial_statements: "0", unavailable_statements: "0", duration_ms: "0" },
  ],
  remote_collection_enabled: false,
  limitations: [],
  ...overrides,
} as HistoryStatusResponse);
const render = (value: HistoryStatusResponse) => renderToStaticMarkup(<CollectionPanel busy={false} onClear={() => undefined} onToggle={() => undefined} status={value} text={english} />);

describe("collection and history panel (design board B)", () => {
  it("renders the DTO's coverage, partial and gap reads without inventing zeros", () => {
    const html = render(status());
    expect(html).toContain("Collecting");
    expect(html).toContain("Pause");
    expect(html).toContain("complete 1 · partial 1 · not collected 1 · gaps are never interpolated");
    expect(html).toContain("Partial · row limit reached");
    expect(html).toContain("Not read · beyond catch-up");
    expect(html).toContain("Not collected: not zero executions");
    expect(html.match(/<li class="(complete|partial|not_collected)"/g)).toHaveLength(3);
  });

  it("leaves the production budget unmeasured until the pilot measures it", () => {
    const html = render(status());
    expect(html).toContain("Not measured yet");
    expect(html.match(/<td class="num muted">—<\/td>/g)).toHaveLength(5);
    expect(render(status({ budget: { ...status().budget, production_budget_measured: true } }))).not.toContain("Not measured yet");
  });

  it("explains the production gate and offers no pause for a stopped collector", () => {
    const gated = status({ collection: { state: "stopped", reason: "production_gate" }, target: "sr.example:9030" });
    const html = render(gated);
    expect(collectionLabel(gated, english)).toBe("Remote collection not enabled yet");
    expect(html).toContain("A production read-only pilot must first confirm TLS verify-full");
    expect(html).not.toMatch(/\bG1\b/);
    expect(html).not.toContain(">Pause<");
    expect(html).not.toContain(">Resume<");
  });

  it("states what the store keeps and never keeps, and hides it when history is off", () => {
    const html = render(status());
    expect(html).toContain("30 days, expired data deleted automatically");
    expect(html).toContain("2.4 MB");
    expect(html).toContain("Never keeps: SQL literals, Profile text, StarRocks credentials.");
    const off = render(status({ store: { state: "disabled" }, collection: { state: "stopped", reason: "history_disabled" } }));
    expect(off).toContain("History is off in this setup (DOCTOR_HISTORY=off); nothing is collected.");
    expect(off).not.toContain("Clear history");
  });

  it("offers resume for a paused collector", () => {
    const html = render(status({ collection: { state: "paused" } }));
    expect(html).toContain("Paused");
    expect(html).toContain(">Resume<");
  });
});
