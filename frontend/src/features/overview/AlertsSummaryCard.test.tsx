import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import { AlertsSummaryCard } from "./AlertsSummaryCard";

describe("AlertsSummaryCard (boards/02-overview)", () => {
  it("renders a v1.1 detail card and marks an unknown alert count with StateStamp instead of 0", () => {
    const unknownHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <AlertsSummaryCard loadPaused={null} onOpen={() => undefined} open={null} />
      </LocaleProvider>,
    );
    expect(unknownHtml).toContain("card--detail");
    expect(unknownHtml).toContain("前往告警处置 →");
    expect(unknownHtml).toContain('data-honest-state="missing"');
    expect(unknownHtml).not.toContain("Routine Load 暂停");

    const activeHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <AlertsSummaryCard loadPaused={1} onOpen={() => undefined} open="3" />
      </LocaleProvider>,
    );
    expect(activeHtml).toContain(">3<");
    expect(activeHtml).toContain("Routine Load 暂停");
    expect(activeHtml).toContain(">1<");
  });
});
