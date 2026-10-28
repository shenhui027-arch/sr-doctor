import { renderToStaticMarkup } from "react-dom/server";
import type { JSX } from "react";
import { describe, expect, it } from "vitest";
import { localizedEnum } from "./enum-label";
import { LocaleProvider, useLocale } from "./locale";

function Probe(): JSX.Element {
  const { locale, text } = useLocale();
  return <span data-locale={locale}>{text("Profile analysis", "Profile 分析")}</span>;
}

describe("LocaleProvider", () => {
  it("defaults to English without browser persistence", () => {
    expect(renderToStaticMarkup(<LocaleProvider><Probe /></LocaleProvider>)).toContain(">Profile analysis<");
  });

  it("renders the Chinese resource without changing evidence values", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><Probe /></LocaleProvider>);
    expect(html).toContain('data-locale="zh"');
    expect(html).toContain("Profile 分析");
  });

  it("localizes the registered info state and preserves unknown values", () => {
    const chinese = (_english: string, value: string) => value;
    expect(localizedEnum("info", chinese)).toBe("信息");
    expect(localizedEnum("custom_state", chinese)).toBe("custom_state");
  });
});
