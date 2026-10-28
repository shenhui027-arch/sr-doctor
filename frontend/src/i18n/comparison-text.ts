import type { LocalizedText } from "./locale";

/** Exact display translations for closed comparison copy; IDs and unknown backend text remain verbatim. */
export type ComparisonTextField = "detail" | "limitation" | "reason" | "subject";

type Translation = readonly [english: string, chinese: string];

const catalog: Record<ComparisonTextField, Record<string, Translation>> = {
  detail: {
    "sink missing": ["Sink missing", "缺少 Sink"],
    partial_inputs: ["Both analyses are partial", "两侧分析均不完整"],
    baseline_partial: ["Baseline analysis is partial", "基线分析不完整"],
    candidate_partial: ["Candidate analysis is partial", "候选分析不完整"],
    graph_unavailable: ["Execution graph unavailable", "执行图不可用"],
  },
  limitation: {
    baseline_exact_zero: ["Baseline is exactly zero", "基线恰为 0"],
    baseline_interval_contains_zero: ["Baseline interval contains zero", "基线区间包含 0"],
    candidate_profile_user_external_rerun_required: ["Candidate Profile requires a user-side rerun", "候选 Profile 需要用户侧重跑"],
    candidate_profile_user_external_rerun_is_user_responsibility: ["Candidate Profile rerun is the user's responsibility", "候选 Profile 重跑由用户负责"],
    observed_changes_do_not_prove_causal_improvement: ["Observed changes do not prove causal improvement", "观测变化不能证明因果改善"],
    "causal attribution limited": ["Causal attribution limited", "因果归因受限"],
    raw_connector_profile_bytes_not_returned_by_connected_profile_comparison: ["Connected comparison does not return raw connector Profile bytes", "已连接对比不返回连接器 Profile 原始字节"],
  },
  reason: {
    "metric intervals preserved": ["Metric intervals preserved", "已保留指标区间"],
    "partial topology": ["Partial topology", "拓扑不完整"],
    bounded_metric_delta: ["Values are rounded to display precision, so changes are intervals", "数值经显示精度舍入，变化以区间表示"],
    duplicate_metric_match: ["Several counters share a name and cannot be paired uniquely", "多个计数器同名，无法唯一配对"],
    metric_missing_on_one_side: ["Some metrics exist on one side only", "部分指标仅一侧存在"],
    registry_semantic_mismatch: ["Metric registries differ between the two analyses", "两次分析的指标注册表语义不同"],
    metric_unit_mismatch: ["Metric units differ", "指标单位不同"],
    metric_delta_overflow: ["A metric change exceeds the representable range", "指标变化超出可表示范围"],
    parallelism_shape_mismatch: ["Fragment or instance counts differ, so counters summed over instance reports (CPU, spill) are not compared", "Fragment 数或实例数不同，按实例上报求和的计数器（CPU、落盘）不参与比较"],
    parallelism_shape_unavailable: ["Fragment or instance counts are unknown on one side, so counters summed over instance reports (CPU, spill) are not compared", "至少一侧 Fragment 数或实例数未知，按实例上报求和的计数器（CPU、落盘）不参与比较"],
    finding_algorithm_changed: ["Rule versions differ between the two analyses", "两次分析的规则版本不同"],
    finding_not_comparable: ["Some findings cannot be called resolved or new: the other side has no complete evaluation of the same rule and object", "部分发现无法判定为已解决或新增：另一侧对同一规则和对象没有完整评估"],
    structure_not_comparable: ["Plan structure is not comparable", "计划结构不可比较"],
    baseline_failed: ["Baseline analysis failed", "基线分析失败"],
    candidate_failed: ["Candidate analysis failed", "候选分析失败"],
    baseline_partial: ["Baseline analysis is partial", "基线分析不完整"],
    candidate_partial: ["Candidate analysis is partial", "候选分析不完整"],
  },
  subject: {
    Coordinator: ["Coordinator", "协调节点"],
    "Query elapsed": ["Query elapsed", "查询耗时"],
    Total: ["Total", "总计"],
    "Total scan operators": ["Total scan operators", "全部扫描算子"],
    query: ["Query", "查询"],
  },
};

export function localizedComparisonText(field: ComparisonTextField, value: string, text: LocalizedText): string {
  const translation = catalog[field][value];
  return translation ? text(...translation) : value;
}
