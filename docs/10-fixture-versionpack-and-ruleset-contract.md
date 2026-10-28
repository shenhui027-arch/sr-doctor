# v0.1 Fixture、VersionPack 与 RuleSet 治理契约

状态：Approved Phase 0C governance baseline / Fixture：已实现，随 v0.7.0 冻结

## 1. 目的

用最少充分、可审计的 corpus 证明 3.5.14 Offline Doctor 行为，并为后续多版本留下显式 delta，不把第三方 sample、README 声明或“看起来能解析”当真值。

## 2. 目录与所有权

```text
contracts/
  schema/
  generated/
fixtures/
  manifests/
  synthetic/3.5.14/
  generated/budgets/
  expected/3.5.14/
backend/crates/version-packs/starrocks_3_5_14/
backend/crates/rule-sets/doctor_v0_1/
```

Task 01 首次创建 schema、root package/Cargo workspace、manifest 和 synthetic fixtures；Task 04 只消费并实现，不重定义期望。

Compare 同时由独立的 `contracts/manifests/doctor-comparator-v0.1.json` 治理。它不是 AnalysisReceipt 的第七组件：AnalysisReceipt 仍严格为原有六个分析组件，ComparisonReceipt 仅属于 Comparison 1.1。Comparator semantic hash 排除 evidence/status/golden IDs/两种 hash 字段，outer hash 只排除 outer manifest hash；fixture manifest 链接 comparator outer hash，因此只允许 VersionPack 的 golden/outer 链级联，VersionPack semantic、六组件 AnalysisReceipt、两份 Report 的 facts_hash/analysis_key 不得因此变化。

命名映射固定如下，目录、Cargo package 与 manifest identity 不再混用：

| kind | directory | Cargo package | manifest id |
|---|---|---|---|
| VersionPack | `backend/crates/version-packs/starrocks_3_5_14/` | `starrocks-version-pack-3-5-14` | `starrocks-3.5.14` |
| RuleSet | `backend/crates/rule-sets/doctor_v0_1/` | `doctor-rules-v0-1` | `doctor-v0.1` |

## 3. FixtureManifest

每个 fixture 必须有一条机器可读 manifest：

```text
FixtureManifest
├─ fixture_id
├─ purpose
├─ source_kind            json_profile | text_profile | generated_budget
├─ provenance             synthetic | apache_example_adapted | private_external
├─ source_reference       repo/commit/path or generation recipe
├─ license_status         approved | private_not_committed | blocked
├─ redaction_method       nullable
├─ input_sha256
├─ expected_sha256
├─ declared_version       nullable
├─ expected_version_context
├─ expected_facts[]
├─ expected_absences[]
├─ expected_diagnostics[]
├─ expected_graph_state
├─ applicable_pack/schema/registry/ruleset versions
└─ limitations[]
```

fixture 内容、manifest input hash、expected canonical JSON hash 三者不一致即测试失败。`expected_absences` 用于明确“不可推断”，防止后续为了页面填满而补值/补边。

## 4. Clean-room 与许可证

- 官方 StarRocks 3.5.14 Apache-2.0 源码/文档可作为语义依据；若改编短例，manifest 记录 path/commit 并进入 NOTICE 审查。
- 社区 Profile 工具的 profiles/JSON 不复制进产品 corpus，只用于观察需求，并据此构造“同类错误”的全新最小 synthetic 样本。
- synthetic fixture 使用虚构 query/node/table 名和最小字段，不含用户 SQL、主机、IP、账号或真实数据量。
- 私有脱敏 Profile 默认仓外，只记录 hash、脱敏方法和本地回执；没有授权不得提交。

## 5. v0.1 最小 corpus

| fixture ID | kind | 必须证明 |
|---|---|---|
| `sr3514_text_minimal_exact` | synthetic text | version detected、四类核心 operator 中可出现的最小事实、typed metric/provenance |
| `sr3514_json_minimal_exact` | synthetic JSON | 与 text 等价事实得到相同 canonical subset |
| `sr3514_metric_units` | generated table | duration longest-match、official `us`、μ/µ strict reject、count 1000、bytes 标签按 1024、IEC unit reject、ratio 全部 P0 cases |
| `sr3514_metric_fidelity` | generated + synthetic | Text time 的 us/ms/s/min 截断区间、bytes/count HALF_UP 端点、count parenthetical raw exact；parse exact 不等于 source exact |
| `sr3514_partial_truncated` | synthetic text | snapshot partial；已完成 metric 仍 exact；无静默 0 |
| `sr3514_metric_invalid` | synthetic text | invalid/overflow/missing/zero 独立状态 |
| `sr3514_graph_explicit` | synthetic | explicit nodes/edges 与 source refs |
| `sr3514_graph_verified_derived` | synthetic | 只有唯一 exchange/plan relation生成 verified edge 与 derivation rule ID |
| `sr3514_graph_unavailable` | synthetic | edges empty、unresolved reasons、Operators/Source 可用 |
| `sr3514_multi_operator_plan_node` | synthetic | 一个 plan_node_id 对应多个 pipeline operators，不错误去重 |
| `sr3514_ordered_plan_operators` | synthetic | 双子 Join children、sinkIds、native/subordinate operatorProfiles 的 source_ordinal；canonicalization 不按 ID 改序 |
| `sr3514_counter_strategies` | synthetic JSON | aggregate/merge/min-max source strategy 分阶段保留，不被产品 aggregation 覆盖 |
| `sr3514_output_rows_recipes` | synthetic | sink/Union/general/broadcast exchange 的有序 VersionPack recipe；source/sink 顺序互换、多 sink/跨 fragment；未知 operator output rows unavailable + diagnostic |
| `sr3514_profile_level2` | synthetic | partial/limitations；graph/rules 不冒充 level 1 merged profile |
| `sr3514_running_snapshot` | synthetic | runtime/unmerged 状态传播，最终态规则 not_applicable/suspected |
| `version_user_declared_3514` | synthetic | exact candidate pack + parse_only，不直接 supported |
| `version_unknown` | synthetic | no pack/unknown 或 partial，不套最近 pack |
| `compare_equivalent` | two inputs | 等价事实 delta stable/zero |
| `compare_zero_baseline` | two inputs | absolute delta 存在，percentage undefined_baseline_zero |
| `compare_interval_baseline_contains_zero` | two inputs | absolute interval 可保留，percentage undefined_baseline_interval_contains_zero |
| `compare_lossy_and_algorithm_changed` | two inputs | explicit registry equivalence/approximate bounds/rule evaluator version change 分流 |
| `compare_not_comparable` | two inputs | 两侧独立状态和四类 reason，不当作 0 |
| `rule_operator_time_share` | synthetic | `=15/>15/=30/>30`、区间全低/全高/跨15/跨30、分母 exact0/interval含0/缺失、版本/图降级，官方 node_time/denominator trace |
| `rule_spill_observed` | synthetic | spill exact >0、有损 lower-bound >0、区间含0、exact 0、missing/invalid/unsupported operator cases |
| `budget_at_limit` / `budget_over_limit` | generated | 每个资源限制 boundary/boundary+1，不把巨大文件提交仓库 |

这是一份行为矩阵，不要求每行一个大文件；相同最小 fixture 可由 manifest 的 case table 复用。不得为了测试数量复制近似样本。

## 6. VersionPackManifest

```text
VersionPackManifest
├─ pack_id                starrocks-3.5.14
├─ product_pack_version
├─ upstream_repo
├─ upstream_release
├─ upstream_commit        23a56ec517db763ea9bc7ee50b19357acdee0303
├─ source_evidence[]      path + symbol/line anchor + observation
├─ detection_rules[]
├─ metric_alias_delta[]
├─ dimension/unit definitions[]
├─ aggregation/merge definitions[]
├─ metric_recipe_manifest
│  ├─ extraction/selection/combine recipe id + version
│  ├─ ordered input contract
│  ├─ source CounterStrategy requirements
│  └─ manifest_hash
├─ structural_derivation_rules[]
├─ known_unknowns[]
└─ golden_manifest_hash
```

VersionPack 绝不包含产品 severity、Finding 文案或 recommendation。新 StarRocks 版本必须新增独立 pack/delta/goldens；不得修改 3.5.14 旧 expected 来让新版本通过，也不得用“最近版本”fallback。

3.5.14 manifest 至少锁定以下官方源码锚点；line 只作人工定位，symbol/path + commit 才是稳定身份：

| path / symbol | 必须固化的观察 |
|---|---|
| `gensrc/thrift/RuntimeProfile.thrift` / `TCounterStrategy` | `SUM/AVG/SUM_AVG/AVG_SUM`、四类 merge、min-max strategy 是 source semantics，不是产品 aggregation |
| `fe/fe-core/src/main/java/com/starrocks/common/util/RuntimeProfile.java` / `printCounter` | count 括号 raw value、bytes 三位、TIME_NS 分量/截断行为 |
| `fe/fe-core/src/main/java/com/starrocks/common/util/DebugUtil.java` / `getByteUint`、`printTimeMs` | bytes 1024 倍率；>=1m/1h 的 sub-second/sub-minute 丢失边界 |
| `fe/fe-core/src/main/java/com/starrocks/sql/ExplainAnalyzer.java` / output-row setup/search | sink PushRowNum、Union sum PullRowNum、普通节点按 operatorProfiles 源序选择 PullRowNum、broadcast Exchange 除 InstanceNum |
| `fe/fe-core/src/main/java/com/starrocks/sql/ExplainAnalyzer.java` / `computeTimeUsage` | native/subordinate OperatorTotalTime，Exchange NetworkTime，Scan ScanTime，严格 `>30`/`>15` |
| `fe/fe-core/src/main/java/com/starrocks/common/util/ProfilingExecPlan.java` + `ExplainAnalyzer` traversal | 官方图来自有序 exec plan children/sinkIds/fragment；RuntimeProfile indentation 不是替代拓扑 |

所有锚点使用 commit `23a56ec517db763ea9bc7ee50b19357acdee0303`；Task 01 manifest 写入具体完整路径、symbol、观察摘要与 source blob hash。

## 7. RuleSetManifest

```text
RuleSetManifest
├─ ruleset_id/version
├─ compatible_pack_ids[]
├─ ir_schema_range
├─ rules[]
│  ├─ rule_id/version
│  ├─ applicability
│  ├─ typed inputs
│  ├─ formula/threshold source
│  ├─ severity policy
│  ├─ confidence policy
│  ├─ recommendation validation steps
│  └─ golden case IDs[]
└─ manifest_hash
```

`SR3514.OPERATOR_TIME_SHARE` 的 numerator/denominator 和 15/30 display threshold 引用官方 3.5.14 ExplainAnalyzer；将其转成产品 severity/文案属于我们自己的、明确版本化的决策，不宣称官方 query health。

`SR3514.SPILL_OBSERVED` 仅在 registry 中 spill observation 为 exact canonical >0，或有损值 lower bound 明确大于 0 时触发；区间包含 0 时只能 suspected，exact 0 不触发。建议只要求人工验证 memory/operator context。若 evidence chain 或六类案例不完整，规则从 manifest 移除。

## 8. Golden expected 与 canonical hash

- expected 保存 canonical AnalysisSnapshotReport/ComparisonSnapshot 的必要 subset；动态 request ID、nonce、wall time 不进入。JSON/Text 等价性比较单独的 canonical facts subset/facts_hash，不要求 source-specific Report/analysis_key 相同。Compare 14-case table 使用完整 comparator projection input（两侧 attempt、registry、structure、typed cells、findings）和 field-complete expected（receipt/key、四类 delta、reasons/diagnostics/limitations），不是只断言几个结果片段；其 `comparison_key` 用 Comparison 1.1 + comparator semantic receipt + 两侧 attempt key material 重算。
- object keys、enum spelling、number strings、null/empty collection 顺序固定；数组若语义无序必须在后端按稳定 ID 排序。`children/sinkIds/operatorProfiles` 等语义有序数组必须按 source_ordinal 保序，禁止为 hash 改排。
- Rust 输出与 TypeScript generated type round-trip 后 hash 相同。
- Report hash 变化必须能归因到 schema/adapter/parser/registry/graph/pack/ruleset 版本升级；不允许无版本变化的 silent golden update。

## 9. 真实 3.5.14 校准边界

Synthetic goldens 证明契约和已知语义，不证明任意真实 Profile 都兼容。Candidate 发布前若没有合法、脱敏、真实 3.5.14 Profile 完成仓外回放：

- support matrix 可写 `3.5.14 synthetic contract supported`；
- real-input compatibility 必须 `INCONCLUSIVE`；
- 不得写“完美复刻官方”“生产可用”或无证据误差百分比。

## 10. 新版本接入流程

1. 固定官方 release/tag/commit 和相关源码/文档证据；
2. 与 3.5.14 pack 对比 detection、alias、unit、aggregation、merge、graph structural delta；
3. 先查官方，再查独立实现；同源 fork 不计独立证据；
4. 新建 pack 和 corpus，不修改旧 pack 预期；
5. 重放所有旧 fixtures，明确 shared/changed/unsupported；
6. RuleSet 单独声明兼容 pack，必要时升级规则版本；
7. support matrix 只有在 synthetic + 合法真实样本校准后才提升状态。

没有外部可靠解法时，按 strict grammar、typed state、provenance、limitations 和 golden 自研，并在 manifest 标为 `product_decision`。
