import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import { OfflineCompareForm } from "./OfflineCompareForm";

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

    expect(html).toContain('class="compare-connected-hint"');
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
});
