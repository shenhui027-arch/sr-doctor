import type { LocalizedText } from "./locale";

// Display names only. Values, units and metric identity remain server-owned.
const labels: Record<string, [string, string]> = {
  QueryTotalTime: ["Total query time", "查询总耗时"],
  QueryExecutionWallTime: ["Query elapsed time", "查询实际耗时"],
  QueryCumulativeScanTime: ["Cumulative scan time", "累计扫描耗时"],
  QueryCumulativeNetworkTime: ["Cumulative network time", "累计网络耗时"],
  QueryCumulativeOperatorTime: ["Cumulative operator time", "算子累计耗时"],
  QueryPeakMemoryUsagePerNode: ["Peak memory per node", "单节点峰值内存"],
  WallTime: ["Elapsed time", "实际耗时"],
  QueryPlanningTime: ["Planning time", "规划耗时"],
  QueryCumulativeCpuTime: ["Cumulative CPU time", "累计 CPU 耗时"],
  QueryPeakMemoryUsage: ["Peak query memory", "查询峰值内存"],
  QuerySpillBytes: ["Spill bytes", "溢写数据量"],
  // Meanings follow the official 3.5.14 QueryRuntimeProfile merge.
  QueryAllocatedMemoryUsage: ["Memory allocated (cumulative)", "累计分配内存"],
  QueryDeallocatedMemoryUsage: ["Memory released (cumulative)", "累计释放内存"],
  QuerySumMemoryUsage: ["Peak memory summed over BEs", "各 BE 峰值内存之和"],
  QueryPeakScheduleTime: ["Longest schedule time", "最长调度耗时"],
  ResultDeliverTime: ["Result delivery time", "结果返回耗时"],
  FrontendProfileMergeTime: ["FE Profile merge time", "FE 合并 Profile 耗时"],
  InstanceNum: ["Fragment instances", "Fragment 实例数"],
  Active: ["Active time", "活跃耗时"],
  ActiveTime: ["Active time", "活跃耗时"],
  DriverTotalTime: ["Driver total time", "Driver 总耗时"],
  DriverPrepareTime: ["Driver prepare time", "Driver 准备耗时"],
  PendingTime: ["Pending time", "等待耗时"],
  ScheduleTime: ["Schedule time", "调度耗时"],
  ScheduleCount: ["Schedule count", "调度次数"],
  BytesSent: ["Bytes sent", "发送字节数"],
  OperatorTotalTime: ["Operator active time", "算子活跃耗时"],
  TotalTime: ["Total time", "总耗时"],
  ScanTime: ["Scan time", "扫描耗时"],
  ScanBytes: ["Scanned bytes", "扫描数据量"],
  ScanRows: ["Scanned rows", "扫描行数"],
  RowsReturned: ["Returned rows", "返回行数"],
  RowsRead: ["Read rows", "读取行数"],
  OutputRows: ["Output rows", "输出行数"],
  PeakMemoryUsage: ["Peak memory", "峰值内存"],
  OutputChunkBytes: ["Output chunk bytes", "输出 Chunk 字节数"],
  PullChunkNum: ["Pulled chunks", "拉取 Chunk 数"],
  PullRowNum: ["Pulled rows", "拉取行数"],
  PullTotalTime: ["Pull time", "拉取耗时"],
  PushChunkNum: ["Pushed chunks", "推送 Chunk 数"],
  PushRowNum: ["Pushed rows", "推送行数"],
  PushTotalTime: ["Push time", "推送耗时"],
  NetworkTime: ["Network time", "网络耗时"],
};

export function metricLabel(id: string, text: LocalizedText): string {
  const label = labels[id];
  return label ? text(...label) : id;
}

// Translations of backend metric-cell reason codes. The backend decides which
// cell carries a code; unknown codes stay verbatim.
const reasons: Record<string, [string, string]> = {
  query_cpu_summed_over_instance_reports_not_wall_or_operator_time: [
    "Sum of the per-BE query CPU time reported by every fragment instance; instances on the same BE repeat it, so it may be double-counted and grows with the instance count. Not wall time, and not any single operator's CPU time.",
    "每个 Fragment 实例上报其所在 BE 的查询累计 CPU，FE 对全部实例求和；同一 BE 上的实例重复计入，可能重复计数，并随实例数增大。不是墙钟时间，也不是单个算子的 CPU 时间。",
  ],
  query_spill_summed_over_instance_reports: [
    "Sum of the per-BE query spill bytes reported by every fragment instance; instances on the same BE repeat it, so it may be double-counted.",
    "每个 Fragment 实例上报其所在 BE 的查询落盘字节数，FE 对全部实例求和；同一 BE 上的实例重复计入，可能重复计数。",
  ],
};

export function metricReasonText(code: string, text: LocalizedText): string {
  const reason = reasons[code];
  return reason ? text(...reason) : code;
}
