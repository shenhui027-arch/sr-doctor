import type { LocalizedText } from "./locale";

/**
 * Presentation-only translations for the frozen RuleSet wording.
 *
 * The catalog is intentionally keyed by rule ID and exact backend values.
 * Free-form evidence, Advisor text, and future rules stay unchanged instead
 * of being guessed or reinterpreted by the client.
 */
export type FindingTextField =
  | "applicability"
  | "category"
  | "confidence_reason"
  | "evidence_completeness_reason"
  | "expected_direction"
  | "limitation"
  | "next_step"
  | "recommendation_limitation"
  | "recommendation_title"
  | "risk"
  | "step_detail"
  | "step_title"
  | "validation_step";

type Translation = readonly [english: string, chinese: string];
type FieldCatalog = Partial<Record<FindingTextField, Record<string, Translation>>>;

const common: FieldCatalog = {
  confidence_reason: {
    exact_typed_inputs: ["Exact typed inputs", "输入已类型化且精确"],
    bounded_typed_inputs: ["Bounded typed inputs", "输入已类型化并带边界"],
    threshold_boundary_intersection: ["Threshold-boundary intersection", "与阈值边界相交"],
    threshold_or_zero_boundary_intersection: ["Threshold or zero-boundary intersection", "阈值或零值边界相交"],
  },
  evidence_completeness_reason: {
    exact_typed_inputs: ["Exact typed inputs", "输入已类型化且精确"],
    bounded_typed_inputs: ["Bounded typed inputs", "输入已类型化并带边界"],
    threshold_boundary_intersection: ["Threshold-boundary intersection", "与阈值边界相交"],
    required_evidence_unavailable: ["Required evidence is unavailable", "必需证据不可用"],
    version_or_profile_not_eligible: ["Version or Profile is not eligible", "版本或 Profile 不符合适用条件"],
    matching_operator_group_missing: ["Matching operator group is missing", "缺少匹配的算子组"],
    join_operator_missing: ["Join operator is missing", "缺少 Join 算子"],
    spill_observation_missing: ["Spill observation is missing", "缺少落盘观测"],
    matching_plan_node_missing: ["Matching plan node is missing", "缺少匹配的计划节点"],
    runtime_operator_missing: ["Runtime operator is missing", "缺少运行时算子"],
    join_probe_operator_missing: ["Join probe operator is missing", "缺少 Join 探测算子"],
    broadcast_exchange_missing: ["Broadcast exchange is missing", "缺少广播交换"],
    streaming_preaggregation_missing: ["Streaming pre-aggregation is missing", "缺少流式预聚合"],
  },
  limitation: {
    "operator active time is not query wall time": ["Operator active time is not query wall time", "算子活跃时间不等同于查询墙钟时间"],
    "parallel and asynchronous operators may overlap": ["Parallel and asynchronous operators may overlap", "并行和异步算子可能重叠"],
    "source_merge_state_unknown_not_promoted; source counter stage and MAX-counter selection are verified-derived from the official 3.5.14 stored pipeline Profile build recipe; a missing __MAX_OF_ counter is consumed only under the official redundant base=min=max removal contract": [
      "Node time follows the official 3.5.14 Profile merge recipe (verified-derived): a counter without a printed maximum is used only when StarRocks dropped it because min = max = value",
      "节点耗时按官方 3.5.14 Profile 合并规则推导（已验证推导）：未打印最大值的计数器，只在 StarRocks 因最小值 = 最大值 = 取值而省略时才使用",
    ],
  },
  validation_step: {
    "re-profile after one controlled manual change": ["Re-profile after one controlled manual change", "一次受控人工变更后重新采集 Profile"],
  },
};

const ruleSpecific: Record<string, FieldCatalog> = {
  "SR3514.SCAN_TIME_SHARE": {
    category: {
      operator_time_distribution: ["Operator time distribution", "算子时间分布"],
    },
    risk: {
      "scan time share needs manual investigation; it does not prove pruning or I/O root cause": ["Scan time share needs manual investigation; it does not prove a pruning or I/O root cause", "扫描时间占比需要人工调查；它不能证明裁剪或 I/O 根因"],
    },
    expected_direction: {
      "reduce selected scan time share after one controlled manual change": ["Reduce selected scan time share after one controlled manual change", "一次受控人工变更后降低所选扫描算子的时间占比"],
    },
    validation_step: {
      "inspect predicates, partition pruning, index, rows, bytes and I/O context": ["Inspect predicates, partition pruning, index, rows, bytes and I/O context", "检查谓词、分区裁剪、索引、行数、字节数与 I/O 上下文"],
    },
  },
  "SR3514.JOIN_TIME_SHARE": {
    category: { operator_time_distribution: ["Operator time distribution", "算子时间分布"] },
    risk: { "join time share needs manual investigation; it does not prove a join root cause": ["Join time share needs manual investigation; it does not prove a join root cause", "Join 时间占比需要人工调查；它不能证明 Join 根因"] },
    expected_direction: { "reduce selected join time share after one controlled manual change": ["Reduce selected join time share after one controlled manual change", "一次受控人工变更后降低所选 Join 算子的时间占比"] },
    validation_step: { "inspect join conditions, build/probe, distribution, rows, memory and spill context": ["Inspect join conditions, build/probe, distribution, rows, memory and spill context", "检查 Join 条件、Build/Probe、分布方式、行数、内存与落盘上下文"] },
  },
  "SR3514.AGGREGATE_TIME_SHARE": {
    category: { operator_time_distribution: ["Operator time distribution", "算子时间分布"] },
    risk: { "aggregate time share needs manual investigation; it does not prove an aggregation root cause": ["Aggregate time share needs manual investigation; it does not prove an aggregation root cause", "聚合时间占比需要人工调查；它不能证明聚合根因"] },
    expected_direction: { "reduce selected aggregate time share after one controlled manual change": ["Reduce selected aggregate time share after one controlled manual change", "一次受控人工变更后降低所选聚合算子的时间占比"] },
    validation_step: { "inspect grouping, stages, input/output rows, memory and spill context": ["Inspect grouping, stages, input/output rows, memory and spill context", "检查分组、聚合阶段、输入/输出行数、内存与落盘上下文"] },
  },
  "SR3514.EXCHANGE_NETWORK_TIME_SHARE": {
    category: { operator_time_distribution: ["Operator time distribution", "算子时间分布"] },
    risk: { "exchange time share needs manual investigation; it does not prove a network root cause": ["Exchange time share needs manual investigation; it does not prove a network root cause", "Exchange 时间占比需要人工调查；它不能证明网络根因"] },
    expected_direction: { "reduce selected exchange time share after one controlled manual change": ["Reduce selected exchange time share after one controlled manual change", "一次受控人工变更后降低所选 Exchange 算子的时间占比"] },
    validation_step: { "inspect distribution, transferred rows/bytes, upstream/downstream and network context": ["Inspect distribution, transferred rows/bytes, upstream/downstream and network context", "检查分布方式、传输行数/字节数、上下游与网络上下文"] },
  },
  "SR3514.SPILL_OBSERVED": {
    category: {
      spill_observation: ["Spill observation", "落盘观测"],
    },
    limitation: {
      "spill presence alone does not prove the root cause": ["Spill presence alone does not prove the root cause", "仅有落盘现象不足以证明根因"],
    },
    risk: {
      "spill presence alone does not prove the root cause": ["Spill presence alone does not prove the root cause", "仅有落盘现象不足以证明根因"],
    },
    expected_direction: {
      "reduce spill observations after one controlled manual change": ["Reduce spill observations after one controlled manual change", "一次受控人工变更后降低落盘观测"],
    },
    validation_step: {
      "inspect memory and operator context": ["Inspect memory and operator context", "检查内存与算子上下文"],
    },
  },
  "SR3514.ROW_FLOW_AMPLIFICATION": {
    category: { runtime_row_flow_amplification: ["Runtime row-flow amplification", "运行时行流放大"] },
    limitation: { "row amplification is an investigation signal and does not prove a join root cause": ["Row amplification is an investigation signal and does not prove a join root cause", "行流放大只是调查信号，不能证明 Join 根因"] },
    risk: { "observed row amplification does not prove bad SQL or statistics": ["Observed row amplification does not prove bad SQL or statistics", "观察到的行流放大不能证明 SQL 或统计信息存在问题"] },
    expected_direction: { "reduce observed row amplification after one controlled manual change": ["Reduce observed row amplification after one controlled manual change", "一次受控人工变更后降低观察到的行流放大"] },
    validation_step: { "inspect join condition, cardinality and distribution": ["Inspect join condition, cardinality and distribution", "检查 Join 条件、基数与分布方式"] },
  },
  "SR3514.NODE_TIME_SHARE": {
    category: { operator_time_distribution: ["Operator time distribution", "算子时间分布"] },
    risk: { "plan node time share needs manual investigation; it does not prove a root cause": ["Plan node time share needs manual investigation; it does not prove a root cause", "计划节点时间占比需要人工调查；它不能证明根因"] },
    expected_direction: { "reduce selected plan node time share after one controlled manual change": ["Reduce selected plan node time share after one controlled manual change", "一次受控人工变更后降低所选计划节点的时间占比"] },
    validation_step: { "inspect the operator, its input/output rows and upstream/downstream context": ["Inspect the operator, its input/output rows and upstream/downstream context", "检查该算子、其输入/输出行数与上下游上下文"] },
  },
  "SR3514.OPERATOR_TIME_SKEW": {
    category: { runtime_skew: ["Runtime skew", "运行时倾斜"] },
    limitation: {
      "merged MIN/MAX describe drivers or instances, not tablets or keys; skew does not prove its cause": ["Merged MIN/MAX describe drivers or instances, not tablets or keys; skew does not prove its cause", "合并后的 MIN/MAX 描述的是 driver 或实例，而非 tablet 或键；倾斜本身不能证明成因"],
      "a single driver prints no MIN/MAX and is never skewed": ["A single driver prints no MIN/MAX and is never skewed", "单个 driver 不打印 MIN/MAX，不会判为倾斜"],
    },
    risk: { "driver time skew needs manual investigation; it does not prove data skew on a specific key": ["Driver time skew needs manual investigation; it does not prove data skew on a specific key", "driver 耗时倾斜需要人工调查；它不能证明某个具体键上的数据倾斜"] },
    expected_direction: { "reduce the slowest-driver to mean ratio after one controlled manual change": ["Reduce the slowest-driver to mean ratio after one controlled manual change", "一次受控人工变更后降低最慢 driver 与平均值之比"] },
    validation_step: { "inspect bucket/partition distribution, join/group keys and per-driver input rows": ["Inspect bucket/partition distribution, join/group keys and per-driver input rows", "检查分桶/分区分布、Join/分组键与各 driver 的输入行数"] },
  },
  "SR3514.JOIN_PROBE_FANOUT": {
    category: { join_fanout: ["Join fan-out", "Join 扇出"] },
    limitation: {
      "fan-out is an investigation signal; a many-to-many join may be intended": ["Fan-out is an investigation signal; a many-to-many join may be intended", "扇出只是调查信号；多对多 Join 可能是有意为之"],
      "runtime filters and LIMIT can change probe rows": ["Runtime filters and LIMIT can change probe rows", "Runtime Filter 与 LIMIT 会改变探测行数"],
    },
    risk: { "join fan-out does not prove a wrong join condition": ["Join fan-out does not prove a wrong join condition", "Join 扇出不能证明 Join 条件有误"] },
    expected_direction: { "reduce probe output per input row after one controlled manual change": ["Reduce probe output per input row after one controlled manual change", "一次受控人工变更后降低每个探测输入行对应的输出行数"] },
    validation_step: { "inspect join keys, key duplication and the join condition": ["Inspect join keys, key duplication and the join condition", "检查 Join 键、键重复度与 Join 条件"] },
  },
  "SR3514.BROADCAST_EXCHANGE_BYTES": {
    category: { exchange_volume: ["Broadcast volume", "广播数据量"] },
    limitation: {
      "local pass-through bytes (BytesPassThrough) are not network bytes and are not counted": ["Local pass-through bytes (BytesPassThrough) are not network bytes and are not counted", "本地直通字节（BytesPassThrough）不是网络字节，不计入"],
      "BytesSent is printed rounded; the rule uses its precision interval": ["BytesSent is printed rounded; the rule uses its precision interval", "BytesSent 以舍入形式打印；规则使用其精度区间"],
    },
    risk: { "a large broadcast does not prove that a shuffle join would be faster": ["A large broadcast does not prove that a shuffle join would be faster", "广播量大不能证明改用 Shuffle Join 会更快"] },
    expected_direction: { "reduce broadcast bytes after one controlled manual change": ["Reduce broadcast bytes after one controlled manual change", "一次受控人工变更后降低广播字节数"] },
    validation_step: { "inspect build-side size, statistics, broadcast_row_limit and join hints": ["Inspect build-side size, statistics, broadcast_row_limit and join hints", "检查 Build 侧大小、统计信息、broadcast_row_limit 与 Join 提示"] },
  },
  "SR3514.STREAMING_PREAGG_INEFFECTIVE": {
    category: { aggregation_efficiency: ["Ineffective pre-aggregation", "预聚合失效"] },
    limitation: {
      "an adaptive pass-through may be the right choice for high-cardinality keys": ["An adaptive pass-through may be the right choice for high-cardinality keys", "对高基数键，自适应直通可能是正确选择"],
      "the rule does not see AggComputeTime or upstream BytesSent": ["The rule does not see AggComputeTime or upstream BytesSent", "本规则不读取 AggComputeTime 或上游 BytesSent"],
    },
    risk: { "an ineffective pre-aggregation does not prove that forcing it would be faster": ["An ineffective pre-aggregation does not prove that forcing it would be faster", "预聚合失效不能证明强制预聚合会更快"] },
    expected_direction: { "reduce rows leaving the pre-aggregation after one controlled manual change": ["Reduce rows leaving the pre-aggregation after one controlled manual change", "一次受控人工变更后降低预聚合输出的行数"] },
    validation_step: { "inspect GROUP BY key cardinality and streaming_preaggregation_mode": ["Inspect GROUP BY key cardinality and streaming_preaggregation_mode", "检查 GROUP BY 键基数与 streaming_preaggregation_mode"] },
  },
};

// Ruleset 0.3.0 structured recommendations, keyed by the exact backend text.
const recommendationCopy: Record<string, FieldCatalog> = {
  "SR3514.SCAN_TIME_SHARE": {
    recommendation_limitation: {
      "partition definitions and table statistics are not read; pruning conclusions rest on the Profile counters": ["Partition definitions and table statistics are not read; pruning conclusions rest on the Profile counters", "未读取分区定义与表统计信息；分区裁剪结论只依据 Profile 中的计数器"],
    },
    recommendation_title: {
      "Reduce what the scan reads": ["Reduce what the scan reads", "减少扫描读取量"],
    },
    next_step: {
      "Check partition pruning and predicate pushdown on this table before changing the query": ["Check partition pruning and predicate pushdown on this table before changing the query", "先检查该表的分区裁剪与谓词下推，再考虑改写查询"],
    },
    step_title: {
      "Confirm filters reach the partition and bucket columns": ["Confirm filters reach the partition and bucket columns", "确认过滤条件作用在分区列和分桶列上"],
      "Compare rows read with rows the query keeps": ["Compare rows read with rows the query keeps", "对比扫描输出行数与查询保留的行数"],
    },
    step_detail: {
      "Check that WHERE constrains the partition column directly, without wrapping it in a function or cast": ["Check that WHERE constrains the partition column directly, without wrapping it in a function or cast", "检查 WHERE 是否直接约束分区列，避免对分区列做函数或类型转换包裹"],
      "A scan that outputs far more rows than its consumer keeps points at a filter applied after the scan": ["A scan that outputs far more rows than its consumer keeps points at a filter applied after the scan", "扫描输出远多于下游保留的行数，说明过滤发生在扫描之后"],
    },
  },
  "SR3514.JOIN_TIME_SHARE": {
    recommendation_limitation: {
      "join keys, table statistics and join hints are not read": ["Join keys, table statistics and join hints are not read", "未读取关联键、表统计信息与 Join Hint"],
    },
    recommendation_title: {
      "Reduce the rows entering this join": ["Reduce the rows entering this join", "减少进入该 Join 的行数"],
    },
    next_step: {
      "Filter the larger input as early as possible upstream before tuning the join itself": ["Filter the larger input as early as possible upstream before tuning the join itself", "优先在上游尽早过滤较大的一侧输入，再调整 Join 本身"],
    },
    step_title: {
      "Check filters on the larger input": ["Check filters on the larger input", "检查较大一侧输入的过滤条件"],
      "Compare join input and output rows": ["Compare join input and output rows", "对比 Join 的输入与输出行数"],
      "Review the distribution mode": ["Review the distribution mode", "复核 Join 分布方式"],
    },
    step_detail: {
      "Confirm predicates on the larger side are pushed below the join": ["Confirm predicates on the larger side are pushed below the join", "确认较大一侧的谓词已下推到 Join 之下"],
      "Output far larger than input means the join condition multiplies rows": ["Output far larger than input means the join condition multiplies rows", "输出远大于输入，说明关联条件放大了行数"],
      "Broadcasting a large build side to every instance costs memory and network; a shuffle may fit better": ["Broadcasting a large build side to every instance costs memory and network; a shuffle may fit better", "构建侧较大时广播到每个实例会占用内存与网络，Shuffle 可能更合适"],
    },
  },
  "SR3514.AGGREGATE_TIME_SHARE": {
    recommendation_limitation: {
      "grouping key cardinality is not read": ["Grouping key cardinality is not read", "未读取分组键基数"],
    },
    recommendation_title: {
      "Reduce the rows and groups this aggregation handles": ["Reduce the rows and groups this aggregation handles", "减少聚合处理的行数与分组"],
    },
    next_step: {
      "Filter before aggregating, then check that every grouping key is needed": ["Filter before aggregating, then check that every grouping key is needed", "先在聚合前过滤，再确认每个分组键都是必要的"],
    },
    step_title: {
      "Check the aggregation input": ["Check the aggregation input", "检查聚合输入"],
      "Check the number of groups": ["Check the number of groups", "检查分组数量"],
    },
    step_detail: {
      "Confirm filters run before the aggregation rather than in HAVING": ["Confirm filters run before the aggregation rather than in HAVING", "确认过滤发生在聚合之前，而不是在 HAVING 中"],
      "Output close to input means the grouping keys barely reduce the data": ["Output close to input means the grouping keys barely reduce the data", "输出接近输入，说明分组键几乎没有压缩数据"],
    },
  },
  "SR3514.EXCHANGE_NETWORK_TIME_SHARE": {
    recommendation_limitation: {
      "cluster topology and network load are not read": ["Cluster topology and network load are not read", "未读取集群拓扑与网络负载"],
    },
    recommendation_title: {
      "Reduce the data this exchange sends": ["Reduce the data this exchange sends", "减少该交换发送的数据量"],
    },
    next_step: {
      "Drop rows and columns before the exchange, then review its distribution": ["Drop rows and columns before the exchange, then review its distribution", "先在交换前去掉不需要的行和列，再复核分布方式"],
    },
    step_title: {
      "Check what is sent": ["Check what is sent", "检查发送的数据"],
      "Review the distribution": ["Review the distribution", "复核分布方式"],
    },
    step_detail: {
      "Compare the rows and bytes sent with what the consumer actually needs": ["Compare the rows and bytes sent with what the consumer actually needs", "对比发送的行数、字节数与下游实际需要的数据"],
      "A broadcast copies data to every instance; a shuffle or colocate join may move less": ["A broadcast copies data to every instance; a shuffle or colocate join may move less", "广播会把数据复制到每个实例；Shuffle 或 Colocate Join 可能传输更少"],
    },
  },
  "SR3514.NODE_TIME_SHARE": {
    recommendation_limitation: {
      "the time share names no cause; the operator's internal counters are not analysed": ["The time share names no cause; the operator's internal counters are not analysed", "耗时占比不说明原因；未分析该算子内部的计数器"],
    },
    recommendation_title: {
      "Find why this operator takes the most time": ["Find why this operator takes the most time", "查明该算子耗时最多的原因"],
    },
    next_step: {
      "Compare its input and output rows with the operators around it": ["Compare its input and output rows with the operators around it", "对比它与上下游算子的输入、输出行数"],
    },
    step_title: {
      "Compare input and output rows": ["Compare input and output rows", "对比输入与输出行数"],
      "Check its place in the plan": ["Check its place in the plan", "检查它在计划中的位置"],
    },
    step_detail: {
      "A large input or unexpected growth points at the plan upstream": ["A large input or unexpected growth points at the plan upstream", "输入过大或行数异常增长，问题多在上游计划"],
      "Use the raw structure view to see which fragment and pipeline it runs in": ["Use the raw structure view to see which fragment and pipeline it runs in", "在原始结构视图查看它所在的 Fragment 与 Pipeline"],
    },
  },
  "SR3514.SPILL_OBSERVED": {
    recommendation_limitation: {
      "memory limits and concurrent load are not read": ["Memory limits and concurrent load are not read", "未读取内存上限与并发负载"],
    },
    recommendation_title: {
      "Find what spilled to disk": ["Find what spilled to disk", "查明落盘的原因"],
    },
    next_step: {
      "Check the memory of the spilling operators before changing memory limits": ["Check the memory of the spilling operators before changing memory limits", "先检查落盘算子的内存，再考虑调整内存上限"],
    },
    step_title: {
      "Find the spilling operators": ["Find the spilling operators", "找出落盘的算子"],
      "Reduce what those operators hold": ["Reduce what those operators hold", "减少这些算子持有的数据"],
    },
    step_detail: {
      "Join, aggregate and sort operators spill when their state exceeds the memory limit": ["Join, aggregate and sort operators spill when their state exceeds the memory limit", "Join、聚合、排序算子的状态超出内存上限时会落盘"],
      "Drop rows or columns entering the spilling operator first": ["Drop rows or columns entering the spilling operator first", "优先减少进入落盘算子的行或列"],
    },
  },
  "SR3514.ROW_FLOW_AMPLIFICATION": {
    recommendation_limitation: {
      "key uniqueness is not checked by the product": ["Key uniqueness is not checked by the product", "产品不检查关联键唯一性"],
    },
    recommendation_title: {
      "Check why this join multiplies rows": ["Check why this join multiplies rows", "查明该 Join 放大行数的原因"],
    },
    next_step: {
      "Confirm that join keys the query treats as unique really are unique": ["Confirm that join keys the query treats as unique really are unique", "确认查询视为唯一的关联键确实唯一"],
    },
    step_title: {
      "Compare output with input rows": ["Compare output with input rows", "对比输出与输入行数"],
      "Check key uniqueness": ["Check key uniqueness", "检查关联键唯一性"],
    },
    step_detail: {
      "Output many times its inputs means duplicate keys on at least one side": ["Output many times its inputs means duplicate keys on at least one side", "输出是输入的数倍，说明至少一侧存在重复键"],
      "Count duplicate join keys on each side with a separate read-only query": ["Count duplicate join keys on each side with a separate read-only query", "用单独的只读查询统计两侧关联键的重复数"],
    },
  },
  "SR3514.OPERATOR_TIME_SKEW": {
    recommendation_limitation: {
      "per-key distribution is not read; MIN/MAX describe instances, not keys": ["Per-key distribution is not read; MIN/MAX describe instances, not keys", "未读取按键的分布；MIN/MAX 描述的是实例而非键"],
    },
    recommendation_title: {
      "Balance the work across instances": ["Balance the work across instances", "均衡各实例的工作量"],
    },
    next_step: {
      "Find the key value that sends most rows to the slowest instance": ["Find the key value that sends most rows to the slowest instance", "找出把大部分行送到最慢实例的键值"],
    },
    step_title: {
      "Compare the slowest and the average instance": ["Compare the slowest and the average instance", "对比最慢实例与平均值"],
      "Check the distribution key": ["Check the distribution key", "检查分布键"],
    },
    step_detail: {
      "A maximum far above the mean means a few instances do most of the work": ["A maximum far above the mean means a few instances do most of the work", "最大值远高于平均值，说明少数实例承担了大部分工作"],
      "Look for a hot value such as NULL or a default in the shuffle or bucket key": ["Look for a hot value such as NULL or a default in the shuffle or bucket key", "检查 Shuffle 键或分桶键中是否有 NULL、默认值等热点值"],
    },
  },
  "SR3514.JOIN_PROBE_FANOUT": {
    recommendation_limitation: {
      "build-side key duplication is not read": ["Build-side key duplication is not read", "未读取构建侧键的重复情况"],
    },
    recommendation_title: {
      "Reduce the rows the join probe produces": ["Reduce the rows the join probe produces", "减少 Join 探测产生的行数"],
    },
    next_step: {
      "Check whether the build side holds duplicate keys the query does not need": ["Check whether the build side holds duplicate keys the query does not need", "确认构建侧是否有查询不需要的重复键"],
    },
    step_title: {
      "Compare probe output with probe input": ["Compare probe output with probe input", "对比探测输出与输入行数"],
      "Check duplicate build keys": ["Check duplicate build keys", "检查构建侧重复键"],
    },
    step_detail: {
      "Each probe row matching many build rows multiplies the output": ["Each probe row matching many build rows multiplies the output", "每行探测数据匹配多行构建数据，会使输出成倍增加"],
      "Count duplicate join keys on the build side with a separate read-only query": ["Count duplicate join keys on the build side with a separate read-only query", "用单独的只读查询统计构建侧关联键的重复数"],
    },
  },
  "SR3514.BROADCAST_EXCHANGE_BYTES": {
    recommendation_limitation: {
      "broadcast_row_limit and table statistics are not read": ["Broadcast_row_limit and table statistics are not read", "未读取 broadcast_row_limit 与表统计信息"],
    },
    recommendation_title: {
      "Avoid broadcasting a large input": ["Avoid broadcasting a large input", "避免广播大输入"],
    },
    next_step: {
      "Shrink the broadcast side first; if it stays large, consider a shuffle join": ["Shrink the broadcast side first; if it stays large, consider a shuffle join", "先缩小被广播的一侧；仍然很大时再考虑 Shuffle Join"],
    },
    step_title: {
      "Check the broadcast size": ["Check the broadcast size", "检查广播数据量"],
      "Filter or project the broadcast side": ["Filter or project the broadcast side", "过滤或裁剪被广播的一侧"],
    },
    step_detail: {
      "Every instance receives the full broadcast input": ["Every instance receives the full broadcast input", "每个实例都会收到完整的广播数据"],
      "Drop unused rows and columns before the join": ["Drop unused rows and columns before the join", "在 Join 前去掉不需要的行和列"],
    },
  },
  "SR3514.STREAMING_PREAGG_INEFFECTIVE": {
    recommendation_limitation: {
      "grouping key cardinality is not read": ["Grouping key cardinality is not read", "未读取分组键基数"],
    },
    recommendation_title: {
      "Review the streaming pre-aggregation": ["Review the streaming pre-aggregation", "复核流式预聚合"],
    },
    next_step: {
      "Check whether the grouping keys repeat enough for pre-aggregation to help": ["Check whether the grouping keys repeat enough for pre-aggregation to help", "确认分组键的重复程度是否足以让预聚合起作用"],
    },
    step_title: {
      "Compare pre-aggregation output with input": ["Compare pre-aggregation output with input", "对比预聚合输出与输入行数"],
      "Check grouping key cardinality": ["Check grouping key cardinality", "检查分组键基数"],
    },
    step_detail: {
      "Output close to input means the keys barely reduce data before the shuffle": ["Output close to input means the keys barely reduce data before the shuffle", "输出接近输入，说明分组键在 Shuffle 前几乎没有压缩数据"],
      "Count distinct grouping keys with a separate read-only query; nearly unique keys cannot be pre-aggregated": ["Count distinct grouping keys with a separate read-only query; nearly unique keys cannot be pre-aggregated", "用单独的只读查询统计分组键的去重数；接近唯一的键无法预聚合"],
    },
  },
};
for (const [ruleId, catalog] of Object.entries(recommendationCopy)) Object.assign(ruleSpecific[ruleId], catalog);

/** Localize only frozen RuleSet copy; unknown values remain verbatim. */
export function localizedFindingText(ruleId: string | null | undefined, field: FindingTextField, value: string, text: LocalizedText): string {
  if (ruleId && !ruleSpecific[ruleId]) return value;
  const translation = (ruleId ? ruleSpecific[ruleId]?.[field]?.[value] : undefined) ?? common[field]?.[value];
  return translation ? text(...translation) : value;
}

