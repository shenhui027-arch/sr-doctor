import { renderToStaticMarkup } from "react-dom/server";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../contracts/generated/connected-contract-fixtures";
import { sr3514JsonMinimal } from "../../../contracts/generated/contract-fixtures";
import { CompareWorkspace } from "../features/compare/CompareWorkspace";
import { RecommendationsPanel } from "../features/recommendations/RecommendationsPanel";
import { LocaleProvider } from "./locale";

function renderChinese(node: ReactNode): string {
  return renderToStaticMarkup(<LocaleProvider initialLocale="zh">{node}</LocaleProvider>);
}

describe("bilingual production workspaces", () => {
  it("renders Recommendations in Chinese without translating protocol or evidence terms", () => {
    const html = renderChinese(<RecommendationsPanel report={sr3514JsonMinimal} />);

    expect(html).toContain("优化建议");
    // Without an Advisor response the folded Advisor block is not rendered at all.
    expect(html).not.toContain("官方 Advisor · 实验性 · FE 本地 · 易变");
    expect(html).not.toContain("official_plan_advisor");

    const withAdvisor = renderChinese(<RecommendationsPanel advisorResponse={sr3514ConnectedContractGolden.advisor_response} report={sr3514JsonMinimal} />);
    expect(withAdvisor).toContain("官方 Advisor · 实验性 · FE 本地 · 易变");
    expect(withAdvisor).toContain("official_plan_advisor");
    expect(withAdvisor).toContain("不执行 SQL · 不自动应用");
  });

  it("renders Compare in Chinese and preserves the raw analysis state", () => {
    const html = renderChinese(<CompareWorkspace comparison={{
      schema_version: "1.1", comparison_receipt: { components: [] }, comparison_key: "zh", baseline: { outcome: { report: sr3514JsonMinimal } }, candidate: { outcome: { report: sr3514JsonMinimal } },
      state: "comparable", reasons: [], compatibility_delta: [], structure_delta: { state: "unchanged", matches: [], added: [], removed: [], changed: [], edge_delta: [], unresolved: [] },
      metric_deltas: [], finding_delta: { added: [], resolved: [], persisting: [], changed: [], incomparable: [], algorithm_changed: [], measurement_changes: [] }, diagnostics: [], limitations: [],
    }} />);

    expect(html).toContain("Profile 对比工作区");
    expect(html).toContain("优化前 · 基线");
    expect(html).toContain("可比");
    expect(html).toContain("分析不完整");
    expect(html).toContain("导出对比报告");
    expect(renderChinese(<CompareWorkspace comparison={null} />)).toBe("");
  });
});
