# 源码级基线分析与证据账本

状态：v0.1 起的源码证据账本，之后持续沿用
目的：区分已确认源码事实、已复现行为、产品判断和未验证项。

## 1. 固定快照

| 角色 | 项目 | Commit | 用途 |
|---|---|---|---|
| 官方语义裁判 | `StarRocks/starrocks` `3.5.14` | `23a56ec517db763ea9bc7ee50b19357acdee0303` | ExplainAnalyzer/FE/BE 语义与文档证据 |

固定源码入口：[官方 ExplainAnalyzer.java](https://github.com/StarRocks/starrocks/blob/23a56ec517db763ea9bc7ee50b19357acdee0303/fe/fe-core/src/main/java/com/starrocks/sql/ExplainAnalyzer.java)。链接只用于定位固定快照；若仓库路径随后变化，以 commit 为准。

社区 Profile 工具只观察过可见交互，不作为语义或代码来源（见 [产品边界与基线策略](./00-product-scope-and-baselines.md)）。

## 2. 证据等级

- **Confirmed source fact**：在固定 commit 的文件/调用链中直接确认。
- **Reproduced**：用最小输入或命令在当前环境复现。
- **Claim only**：README/注释声明，未被代码或测试证明。
- **Inconclusive**：受工具链、依赖、真实数据或环境阻塞。

## 3. 官方 ExplainAnalyzer：开源且语义价值最高

`fe/fe-core/src/main/java/com/starrocks/sql/ExplainAnalyzer.java` 带 Apache-2.0 头，公开于 StarRocks 仓库。它在 FE 内部消费 `ProfilingExecPlan` 与 `RuntimeProfile`，能直接访问计划节点、运行时计数器和版本内部语义。

借鉴价值最强的部分：

- 节点与 Profile 的关联方式；
- counter search/merge、sum/max 和 min/max merged metrics 的处理；
- 节点时间、执行时间与展示层之间的语义；
- 缺失计数器、版本变化和 Profile 结构的官方预期。

不能直接作为产品运行时基线的原因：

- 它依赖 FE 内部对象，不是 standalone 文本/JSON parser；
- 外部导出 Profile 可能丢失结构和 counter 上下文；
- 产品必须支持离线输入、安全限制、版本报告和 Partial IR；
- 直接复制 FE 内部实现会把产品绑定到 StarRocks 内部 API。

结论：**它是语义 oracle，不是可直接 fork 的产品底座。**

### 3.1 3.5.14 真实调用与归并语义

CodeGraph 在固定 sparse checkout 上建立了 590 个节点、1,679 条边的可用索引。本轮直接复核了以下链路：

```text
ExplainAnalyzer.analyze(plan, profile, planNodeIds, color)
  -> new ExplainAnalyzer(...)
  -> ExplainAnalyzer.analyze()
  -> fragment / pipeline / operator profile collection
  -> NodeInfo(native operators + subordinate operators)
  -> computeTimeUsage(QueryCumulativeOperatorTime)
  -> graph/text rendering
```

已确认源码事实：

- `ExplainAnalyzer` 的入口是 FE 内部 `ProfilingExecPlan + RuntimeProfile`，而不是原始文本。当 `plan == null` 时仅对 Stream/Routine Load 特殊路径做 `prettyPrint`。
- plan node 从 operator profile 名称中以 `plan_node_id` 正则取出，匹配失败会触发 precondition；因此离线适配器必须显示报告节点 ID 缺失，不能猜测。
- 一个 plan node 可对应多个 pipeline operator，并分开 native 与 subordinate operator；简化成“一行文本=一个节点”会丢失官方结构语义。
- `NodeInfo.computeTimeUsage` 对 native + subordinate 的 `OperatorTotalTime` 求和，并对 Exchange 叠加 `NetworkTime`、对 Scan 叠加 `ScanTime`；分母是 `QueryCumulativeOperatorTime`，不是 query wall time。
- 官方文本高亮的 30%/15% 是节点累计 operator time 占比的展示级别，不是 query health 总分。`isTimeConsumingMetric` 对单项时间 counter 使用 30% 比例，且 averaged counter 优先参考 `__MAX_OF_` counter。
- FE/BE `RuntimeProfile` 都明确以 `TIME_NS` 作为内部时间单位，并保留 child-counter 层级、merge strategy 与 `__MIN_OF_` / `__MAX_OF_` 辅助 counter。这意味着聚合不能仅依据显示字符串或字段名猜测。
- FE/BE 在计算 profile local time 时会将“父 TotalTime - 子 TotalTime 之和”的微小负值 clamp 为 0；这是特定派生量的数值容差，不授权把缺失/非法输入统一转为 0。

本轮冻结前又下钻了 formatter、Thrift strategy 和 output-row 分支，补充确认：

- `RuntimeProfile.java::printCounter` 对 K/M/B count 输出三位小数并附括号原始整数；离线解析应以括号值为 authoritative exact，缩写只用于区间一致性检查。
- 同一方法对 bytes 固定三位小数；`DebugUtil.getByteUint` 虽显示 `KB/MB/GB/TB`，内部常量从 1024 连乘，不是 SI 1000，也不是 `KiB` 词法。
- TIME_NS 在 `<1us`/`us` 档可保留 ns，但 `>=1ms` 先丢 ns、`>=1s` 先丢 us；`printTimeMs` 又在 `>=1m` 丢 ms、`>=1h` 丢秒。因此“字符串完整消费”与“原始 Counter 精确”必须用 parse_state/fidelity 两个字段分开。
- `RuntimeProfile.thrift::TCounterStrategy` 明确给出 `SUM/AVG/SUM_AVG/AVG_SUM`、四种 merge 和 min-max strategy；这些是 source merge 语义，不能压成产品 `sum|max|min`。
- `ExplainAnalyzer` 的 output rows 不是统一读取：final sink 取 PushRowNum，Union 汇总 native PullRowNum，普通节点按 operatorProfiles 源顺序选择 PullRowNum，broadcast Exchange 再除 fragment InstanceNum。产品必须用 VersionPack 的固定有序 recipe，而不是见到同名字段就 sum。
- 官方图遍历的是 `ProfilingExecPlan` fragment/sink/children；children、sinkIds 与 operatorProfiles 的源顺序具有语义。RuntimeProfile 文本缩进没有同等拓扑证明力，不能作为伪 data-flow edge。

上述证据均固定到 `23a56ec517db763ea9bc7ee50b19357acdee0303`；完整 path/symbol 与 golden 落在 [Fixture/VersionPack 契约](./10-fixture-versionpack-and-ruleset-contract.md)。

### 3.2 官方 Analyzer 不能替产品做的事

- 它不提供离线 JSON/Text grammar、部分解析状态和 source ranges。
- 它利用 FE 计划对象弥补导出 Profile 结构；独立文件不一定保留同等信息。
- 它提供时间高亮和详细 counter，不提供跨工作量的健康分、根因图或自动修复。
- 它的内部合并行为与 release 绑定；产品必须用 Version Pack 隔离变化，不能用一套 regex 声称通吃所有版本。

## 4. 社区工具中常见的缺陷类别（作为反例）

观察社区 Profile 工具时，发现以下几类问题反复出现。它们不针对某个项目，而是产品必须用反例测试挡住的错误类型：

### 4.1 数值与单位

| 错误做法 | 典型输入 | 错误结果 | 正确语义 |
|---|---|---|---|
| 非锚定匹配、按“包含”猜单位 | `1m7s` | `7` | `67s` |
| 同上 | `1h30m` | `1800` | `5400s` |
| 把计数后缀当时间单位 | `258.794M` | 当作分钟 | 计数，括号里的原始值为准 |
| 接受前缀垃圾 | `junk500ms` | `0.5` | 拒绝 |
| 先判断 `m` 再判断 `ms` | `500ms` | 500 分钟 | 0.5 秒 |
| 解析失败时默认 0 | `500us` | `0s` | 500 微秒；失败必须是 invalid，不是 0 |
| 纳秒与毫秒阈值错配 | 微秒级算子 | 被放大成秒级并误报 | 维度类型随值一起传递 |

### 4.2 拓扑与诊断

- 拓扑解析失败后静默回退成线性树，界面无法区分真实结构与假定顺序；
- 回退树总是存在，导致更细的诊断分支成为死路径；
- 热点结果没有映射回图节点。

### 4.3 评分

- 从 100 分按严重度固定扣分，再按总耗时扣分，不考虑扫描量、吞吐、查询类型、SLA、并发或历史基线；10 秒扫描 10 TB 与 10 秒扫描 100 MB 得到相同分数。产品不发布这类总分。

### 4.4 安全与隐私

- 把 Profile 里的用户、SQL、库表等字段插入 `innerHTML`，存在 DOM XSS；
- 把完整 Profile 发到公共分享服务并长期保留；
- 服务默认监听所有网卡、任意 CORS、无认证。

### 4.5 工程

- 多套解析器支持的单位集合不同；
- baseline 为 0 时仍显示百分比变化；
- 没有可运行的测试或 CI，README 中的命令和精度宣称无法复现。

## 5. 基线采用映射

| 能力 | 语义裁判 | 可参考 | 必须自研 |
|---|---|---|---|
| Counter/时间/聚合语义 | 官方 ExplainAnalyzer/FE | — | Typed Metric Parser + Registry |
| JSON Profile 体验 | 官方字段语义 | 社区工具的交互习惯 | Adapter、Graph、DTO、安全渲染 |
| Text Profile | 官方输出/语义 | — | Grammar、Partial IR、错误模型 |
| 规则分类 | 官方语义 + 自有需求 | — | Finding/Evidence/Trace/版本契约 |
| Diff | 规范 IR | 社区工具的对比交互 | 结构/指标/不可比语义 |

## 6. 未验证边界

- 生产集群的端到端 Profile/Audit 校准尚未进行（G1 生产只读试点、G2 真实分布校准待验证）。
- 本机验证只在官方 Docker 镜像上进行：3.5.14 固定 digest 夹具，以及 3.1～4.1 各版本线最新补丁的冒烟。
- 没有建立官方 FE 行为的可执行差分 harness；“与官方一致”的结论来自官方 `ANALYZE PROFILE` 黑盒语料对照。
