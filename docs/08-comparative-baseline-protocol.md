# 多基线三角校验协议

状态：v0.1 起的校验协议，之后持续沿用  
目的：避免围绕一个有缺陷的参考项目反复修补，也避免把多个同源 fork 误当成多份独立证据。

## 1. 证据优先级

1. **Official release truth**：StarRocks 3.5.14 固定源码、对应官方文档、可重现官方输出。
2. **Independent implementation**：至少一个非同源 fork 的同类工具、上游/旁系引擎或成熟库，只用于寻找可选解法和反例。
3. **Product contract**：当外部证据不完整或互相冲突时，以严格 grammar、typed state、provenance、limitations 和 golden fixtures 建立我们自己的可测契约。

README 声明、复制的代码、无测试实现、同源 fork 数量和无法回放的截图不提高证据等级。

## 2. 标准记录

每个问题在开发前建立一行决策记录：

| 字段 | 含义 |
|---|---|
| `problem_id` | 稳定问题 ID |
| `official_ref` | StarRocks tag/commit/path/symbol 与已观察行为 |
| `reference_refs` | 其他项目 commit/path 与证据独立性 |
| `conflicts` | 行为不一致、无法推导或缺失之处 |
| `decision` | `adopt` / `reject` / `own` / `unknown` |
| `product_contract` | 我们对输入、输出、错误和降级的约定 |
| `fixtures` | 正例、反例、边界、缺失、版本和降级样本 |
| `claim_boundary` | 哪些是官方事实，哪些是产品策略 |

## 3. v0.1 首批裁决

| 问题 | 官方 3.5.14 | 社区工具中观察到的做法 | 其他独立观察 | v0.1 裁决 |
|---|---|---|---|---|
| Duration 解析 | 内部 counter 是 typed `TIME_NS`，不提供任意离线文本 grammar | 非锚定匹配、`m` 先于 `ms` 判断，组合时长被误解 | 未找到可直接承诺 StarRocks 导出 grammar 的独立实现 | `own`：整串消费、最长单位优先、整数/Decimal 规范 ns、无静默 0 |
| Topology | FE 持有 plan + profile，一个 plan node 可对应多个 operator，存在 native/subordinate | 依赖特定 JSON 结构假设；失败时回退成线性树 | Apache Doris 当前 `RuntimeProfile` 显式保存 `nodeid` metadata，说明显式节点身份比文本位置猜测更可靠，但不等于 3.5.14 导出契约 | `own`：显式 ID 优先，仅唯一可证时推导，其余 partial/unavailable |
| Operator 聚合 | counter type、merge strategy、min/max 辅助 counter 与 operator 归并为真值 | 从显示字符串提数后直接排序或汇总；多套 parser 与阈值，语义不统一 | Apache Doris 同样保留 counter tree，并在聚合时显式处理缺失 counter，支持“聚合必须有结构/类型”这一方向 | `adopt official semantics + own registry`：按版本注册指标作用域与 sum/max/non-additive |
| Hotspot 高亮 | 节点时间占 QueryCumulativeOperatorTime >30% / >15% 是展示级别 | 固定阈值，且受解析错误与不可达路径影响 | 未找到可直接替代官方 3.5.14 语义的独立 StarRocks Analyzer | `adopt` 官方时间占比作事实排序；不写成健康结论 |
| 总体健康分 | 无本产品所需的 query health 总分 | 100 分固定扣分加总时长扣分，无法区分 10s/10TB 与 10s/100MB | 未发现可将无 SLA/工作量的查询压缩为可解释单分的可靠方案 | `reject`：severity、confidence、evidence completeness、dimensions 分开显示，无条件时 unscored |

## 4. 搜索与停止条件

- 搜索必须固定项目、commit、path 和可见行为，不用“latest”替代证据。
- 同一项目的多个 fork 只算一份证据，不计入多数。
- 在官方语义已明确、一个独立观察已提供可用反例/思路，或连续搜索未找到可靠实现时，停止搜索并进入自研契约，避免无限调研。
- 引入他项目思路不等于引入它的代码；仍需 clean-room、许可证和依赖审查。

## 5. 开发门禁

任一 Parser、Graph、Aggregation、Rule 或 Diff 实现在开始前必须有对应决策行。若 `decision=unknown`，只能实现解析诊断或 unavailable 状态，不得输出确定性 Finding。
