# 产品边界与基线策略

状态：v0.1 起建立的产品边界与基线策略，之后持续沿用；当前功能范围以 [README](../README.md) 为准（v1.0 已扩展到 SQL 质量监控，支持 StarRocks 3.1 及以后）。
适用范围：架构评审、语义依据、clean-room 与许可证审查

## 0. 语义基线

唯一的语义真值是官方 StarRocks 源码，按固定提交使用：

| 来源 | 固定提交 | 用途 |
|---|---|---|
| `StarRocks/starrocks` `3.5.14` | `23a56ec517db763ea9bc7ee50b19357acdee0303` | 官方语义与黄金样本校准基线 |

其他版本 = 3.5.14 基线 + 逐版本源码审计得到的差异，见 [Fixture、VersionPack 与 RuleSet 治理](./10-fixture-versionpack-and-ruleset-contract.md)。不得用 `main`、`latest` 或仓库名称代替提交；每个行为契约都记录 `repo + commit + path + 观察行为 + adopt/reject/unknown + 产品用例`。

## 1. 我们要做什么

产品包含两个能力域：

1. **Profile 诊断**：离线接收已有 JSON/Text Profile；连接模式浏览 Queries、拉取已有 Profile、在用户显式操作下执行受限的单条 `EXPLAIN COSTS`，输出规范化 IR、完整度、版本上下文、证据化建议与可证执行结构。用户在外部完成复跑后，可把 candidate Profile 与 baseline 对比。
2. **SQL 质量监控（v1.0）**：以 Audit Log 为事实层，对 SQL 模板做脱敏、指纹、稳健基线、回归发现、应用内告警和治理闭环。

只读连接有两个用户显式触发的语句例外：受限单条 `EXPLAIN COSTS`（只看计划），以及“运行时 Top”的单条 `SELECT`/`WITH ... SELECT`。后者固定超时、丢弃结果行，只按同次 Query ID 拉取 Profile。产品不执行 `EXPLAIN ANALYZE`、多语句或任何 DDL/DML/INSERT/LOAD/ADMIN/Advisor 修改，不自动安装或配置 AuditLoader，也不自动修改 SQL 或参数。

目标不是再做一个 StarRocks 管理后台，而是建立可解释、可回归、版本感知、默认低侵入的 SQL 质量治理工具。

## 2. 职责分层

### 2.1 官方 StarRocks：语义真值

官方源码最适合回答：

- Runtime Counter 的类型与单位；
- Plan Node 与 pipeline operator 的归并；
- native/subordinate operator 的区分；
- `sum`、`max`、`__MIN_OF_`、`__MAX_OF_` 的含义；
- cumulative operator/scan/network time 与 wall time 的差异；
- 哪些字段随 release 新增、重命名或改变语义。

官方 `ExplainAnalyzer` 运行在 FE 内部，输入是 `ProfilingExecPlan + RuntimeProfile` 对象，不是任意离线文本。因此产品需要独立的 Adapter；普通 `EXPLAIN` 的展示与解析也不能声称“复制一个类”就完成离线兼容。

### 2.2 社区工具：只观察可见行为

设计时观察过社区 Profile 工具的页面信息架构与交互习惯（例如查询列表、执行结构图上的节点定位、两份 Profile 的对比心智）。只借鉴可见的交互习惯，不复用任何第三方的代码、解析规则、阈值、文案或素材。

观察中发现、产品明确不采用的做法：

- 在前端猜测单位或数量语义，或维护多套时间/字节解析器；
- 用 `innerHTML` 直接渲染 Profile 字段；
- 把 Profile 或 SQL 发到公共分享服务；
- 无 schema、无版本包、无错误状态的静默容错；
- 拓扑解析失败时伪造线性树；
- 固定扣分式的总分与未经校准的阈值；
- 默认监听所有网卡、任意 CORS、无认证的服务边界；
- 把未经复现的宣称或无测试的行为当作验收依据。

### 2.3 产品自研层：契约与治理

以下能力由产品自研：

- JSON/Text Adapter；
- 只读连接的 Query 浏览、已有 Profile 拉取与受限 `EXPLAIN COSTS` 边界；
- Typed Metric Parser、Metric Registry 和 Aggregation Registry；
- Normalized IR、完整度、VersionPack、Compatibility Report；
- 与 VersionPack 分开版本化的 RuleSet、Finding/Evidence/Rule Trace、可证 Graph、IR Diff；
- 安全、隐私、资源预算和回归体系。

## 3. 能力与策略

| 能力 | 官方 StarRocks | 产品策略 |
|---|---|---|
| Runtime Profile 语义 | 最强，但依赖 FE 内部对象 | 官方 release 为 oracle，自研离线 Adapter |
| Text Profile 解析 | 没有独立的离线 Parser | 自研契约与实现，单一 Typed Metric Parser |
| Node/Metric 聚合 | 权威 | 逐指标登记语义与作用域 |
| Topology | 权威对象关系 | 显式来源、完整度和 degraded，不伪造边 |
| 证据化建议 | 以分析和高亮为主，不是产品评分 | 自研 Rule Contract，只给风险、预期方向和人工验证步骤 |
| 综合评分 | 无 | 不发布固定总分 |
| Diff | 非产品交互 | 在 IR 层自研 |
| 测试证据 | 官方工程测试 | 自建黄金语料与本地 CI |
| 安全/隐私 | 服务内部边界 | 默认 loopback、无外传、严格渲染 |

## 4. Clean-room 实施规则

1. 官方源码按固定提交使用；正式兼容按 StarRocks release 建 Version Pack。
2. 分析者记录输入、可见行为、源码位置、错误案例和产品差异；实现者只依据产品契约编码。
3. 不复制参考源码文件、同构目录、大段注释、规则文案和阈值。
4. 外部 Fixture 必须先过许可证、来源和脱敏审查；优先自造最小样本。
5. 每项借鉴记录 `source_ref`、`concept`、`decision`、`product_contract`、`golden_case`。
6. 不依赖任何参考快照，产品也能构建、测试和生成第三方清单。
7. 任何“与官方语义一致”的声明都要有黄金语料、自动化差异和可复现报告；新版本追加跨版本回归。

## 5. 许可证边界

- 官方 StarRocks 源码为 Apache-2.0；直接复制会带来许可证、NOTICE 和衍生作品义务。行为契约式的 clean-room 实现仍保留来源记录。
- 第三方项目只观察可见行为，不复制代码或素材。
- 本产品以 Apache-2.0 发布；第三方依赖的许可证见 NOTICE 与发布包中的 SBOM。

## 6. 裁决

- **官方 StarRocks：ADOPT AS SEMANTIC ORACLE**，按 release 固定，不作为产品运行时依赖。
- **社区工具：只观察可见交互**，不 fork、不复用代码。
- **未冻结契约就开写 Analyzer：NO-GO**；先冻结 wire/grammar/manifest/expected matrix，再建立可执行的 schema/types/fixtures/tests。

实现门禁见 [基线采用裁决](./05-baseline-adoption-decision.md) 与 [v0.1 验收矩阵](./07-v0.1-acceptance-matrix.md)。
