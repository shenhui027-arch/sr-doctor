import type { LocalizedText } from "./locale";

/** Labels for backend `FindingMeasurement.measurement_id`; unknown ids stay verbatim. */
const labels: Record<string, [string, string]> = {
  node_time_share: ["Share of operator time", "占算子总耗时"],
  node_time: ["Node time", "节点耗时"],
  slowest_over_mean_driver_time: ["Slowest driver vs. mean", "最慢驱动 / 平均"],
  slowest_driver_time: ["Slowest driver", "最慢驱动耗时"],
  mean_driver_time: ["Mean driver", "平均驱动耗时"],
  fastest_driver_time: ["Fastest driver", "最快驱动耗时"],
  probe_output_over_input_rows: ["Probe rows out / in", "探测输出 / 输入行"],
  probe_input_rows: ["Probe input rows", "探测输入行"],
  probe_output_rows: ["Probe output rows", "探测输出行"],
  broadcast_bytes_sent: ["Broadcast bytes sent", "广播发送字节"],
  preaggregation_output_over_input_rows: ["Rows kept by pre-aggregation", "预聚合保留行比例"],
  preaggregation_input_rows: ["Pre-aggregation input rows", "预聚合输入行"],
  preaggregation_output_rows: ["Pre-aggregation output rows", "预聚合输出行"],
  output_over_child_output_rows: ["Output rows / child rows", "输出行 / 子节点行"],
  node_output_rows: ["Node output rows", "节点输出行"],
  child_output_rows: ["Child output rows", "子节点输出行"],
};

export function measurementLabel(id: string, text: LocalizedText): string {
  const label = labels[id];
  return label ? text(...label) : id;
}

/** The rule's trigger condition, phrased as a condition rather than a raw comparator. */
export function measurementThreshold(threshold: string, text: LocalizedText): string {
  return text(`rule: ${threshold}`, `规则：${threshold}`);
}
