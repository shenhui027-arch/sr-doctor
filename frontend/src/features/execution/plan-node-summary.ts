import type { ExecutionSummary, PlanNodeSummary } from "../../../../contracts/generated/doctor-contracts";

/** Backend basis points order the rows; nodes without an exact share follow in DTO order. */
export function rankedSummaries(summary: ExecutionSummary): PlanNodeSummary[] {
  return summary.nodes
    .map((node, index) => ({ node, index }))
    .sort((left, right) => (right.node.time_share_basis_points ?? -1) - (left.node.time_share_basis_points ?? -1) || left.index - right.index)
    .map(({ node }) => node);
}

/** Why per-node time is not summarized, from the backend's reason codes. */
export function summaryUnavailableText(summary: ExecutionSummary, text: (english: string, chinese: string) => string): string {
  if (summary.reasons.includes("profile_not_merged")) return text("Per-node time needs a merged Profile (pipeline_profile_level=1). The graph still shows the plan structure.", "按节点汇总耗时需要合并后的 Profile（pipeline_profile_level=1）。执行图仍显示计划结构。");
  if (summary.reasons.includes("runtime_operators_unavailable")) return text("This input has no runtime operator metrics; only the plan structure is shown.", "该输入不含运行时算子指标，只显示计划结构。");
  if (summary.reasons.some((reason) => reason === "profile_running_snapshot" || reason === "profile_runtime_snapshot")) return text("The query was still running when this Profile was taken; its counters are provisional, so per-node time is not summarized. Analyze the final Profile after the query finishes.", "采集该 Profile 时查询仍在运行，计数器只是中间值，因此不按节点汇总耗时。请在查询结束后分析最终 Profile。");
  if (summary.reasons.includes("profile_error")) return text("The query failed; its Profile covers only part of the execution, so per-node time is not summarized.", "查询执行失败，Profile 只覆盖部分执行过程，因此不按节点汇总耗时。");
  if (summary.reasons.some((reason) => reason.startsWith("runtime_profile_") || reason === "runtime_operator_running")) return text("StarRocks marked this Profile as incomplete (instances missing, still running, or not identical after merging), so per-node time is not summarized.", "StarRocks 标记该 Profile 不完整（有实例缺失、仍在运行或合并后不一致），因此不按节点汇总耗时。");
  return text("Per-node time is unavailable for this input.", "该输入无法按节点汇总耗时。");
}
