import type { LocalizedText } from "./locale";

/** Exact display translations for the frozen 3.5.14 Advisor contract fixture. */
export type AdvisorTextField = "applicability" | "limitation" | "suggested_action" | "validation_step";

type Translation = readonly [english: string, chinese: string];

const catalog: Record<AdvisorTextField, Record<string, Translation>> = {
  applicability: {
    plan_advisor_available: ["Plan Advisor available", "Plan Advisor 可用"],
  },
  limitation: {
    advisor_is_official_experimental_fe_local_volatile: ["Advisor is official, experimental, FE-local, and volatile", "Advisor 为官方、实验性、FE 本地且易变"],
    not_a_product_ruleset_finding: ["Not a product RuleSet finding", "不是产品 RuleSet 发现项"],
  },
  suggested_action: {
    "Review the Advisor guide before a controlled user-side rerun.": ["Review the Advisor guide before a controlled user-side rerun.", "在受控的用户侧重跑前检查 Advisor 指引。"],
  },
  validation_step: {
    "Run EXPLAIN COSTS.": ["Run EXPLAIN COSTS.", "运行 EXPLAIN COSTS。"],
    "Rerun outside the product and fetch a candidate Profile.": ["Rerun outside the product and fetch a candidate Profile.", "在产品外重跑后拉取候选 Profile。"],
  },
};

/** Localize only exact, frozen Advisor copy; unknown server text remains verbatim. */
export function localizedAdvisorText(field: AdvisorTextField, value: string, text: LocalizedText): string {
  const translation = catalog[field][value];
  return translation ? text(...translation) : value;
}
