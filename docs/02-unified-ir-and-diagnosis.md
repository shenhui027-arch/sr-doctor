# v0.1 统一 IR、Graph、RuleSet 与 Compare 契约

状态：Approved Phase 0C contract baseline / 实现：已实现，随 v0.7.0 冻结

官方校准：`StarRocks 3.5.14@23a56ec517db763ea9bc7ee50b19357acdee0303`

## 1. 范围与不变量

本契约服务 SR Doctor 的 Offline JSON/Text Profile，以及 Connected Audit/Profile/Advisor/Explain/Candidate Profile/Compare 闭环。Audit 自动聚类、长期采集、Monitor、趋势/SLO、任意 SQL 控制台和自动改写/应用仍不进入 v0.1 wire DTO。连接增量对象由 [连接器与优化验证契约](./11-starrocks-connector-and-optimization-loop.md) 约束。

必须同时成立：

1. 原始事实、规范事实、派生事实和产品诊断分层保存。
2. 正式数值只经过一个 Typed Metric Parser；前端不解析 raw/display string。
3. `missing`、`invalid`、`partial`、`overflow`、`unavailable` 和 exact `0` 不可合并。
4. 版本状态由 `version_source`、`pack_match`、`compatibility` 三个独立字段表达；用户声明不等于检测事实或 supported。
5. VersionPack 只承载 release 语义；产品 RuleSet 独立版本化并声明兼容关系。
6. 图边只能来自 explicit evidence 或唯一可证的 verified derivation；其他关系进入 unresolved，不生成 inferred edge。
7. 内部 `NormalizedIR` 与前端安全 `AnalysisSnapshotReport` 分离；UI 只消费 Report DTO，不依赖 source merge、parser 或 registry 内部结构。
8. Report 是一次同步响应中的确定性值对象。v0.1 无分析 revision/任务持久化；短期 connection session 不进入 Report canonical facts。
9. Report 的 canonical JSON 不含 created_at、随机 UUID 或 request ID；相同输入和算法版本必须得到相同 SHA-256。

## 2. NormalizedIR、AnalysisOutcome 与安全 Report

请求级失败不返回伪 Report；能构造部分事实时返回 HTTP 200 的 Partial Report。内部模型和 wire DTO 不共用一个 struct：

```text
NormalizedIR             backend internal, never serialized directly
├─ schema_version
├─ analysis_receipt
│  ├─ adapter
│  ├─ metric_parser
│  ├─ metric_registry
│  ├─ graph_resolver
│  ├─ version_pack
│  └─ rule_set            each item has id/version/manifest hash
├─ analysis_key          sha256(source_hash + above versions + options)
├─ (facts_hash)          on demand: AdapterOutput::facts_hash() = sha256(canonical facts), not stored (v0.6-03)
├─ input                 InputSummary
├─ version               VersionContext
├─ profile_context       ProfileContext
├─ completeness          CompletenessReport
├─ query_summary         QuerySummary | null
├─ fragments[]
├─ pipelines[]
├─ operator_groups[]
├─ metric_observations[]
├─ graph                 ExecutionGraph
├─ derived_metrics[]
├─ findings[]
├─ rule_evaluations[]
├─ diagnostics[]
├─ provenance_index[]
└─ source_document       bounded raw source + parser indexes

AnalysisOutcome          public API envelope
└─ report                AnalysisSnapshotReport

AnalysisSnapshotReport   canonical safe wire DTO
├─ schema_version
├─ analysis_state        complete | partial
├─ analysis_receipt      public id/version/manifest hash only
├─ analysis_key
├─ facts_hash
├─ input                 InputSummary
├─ version               VersionContext
├─ profile_context       ProfileContext
├─ completeness          CompletenessReport
├─ query_summary         QuerySummary | null
├─ operator_rows[]       stable refs + MetricCell only
├─ graph_view            safe nodes/edges/unresolved/source refs
├─ derived_metric_cells[]
├─ findings[]            public evidence/limitations/rule trace projection
├─ rule_evaluations[]    public applicability/receipt projection
├─ diagnostics[]         sole public diagnostic truth
├─ provenance_index[]    bounded safe projection
└─ source_view           SourceView
```

`AnalysisOutcome` 不重复 `analysis_state` 或 `diagnostics`；其唯一公开真值是 `report.analysis_state` 与 `report.diagnostics`。`NormalizedIR.diagnostics` 经过脱敏、限量和稳定排序后生成 Report diagnostics。`analysis_key` 包含 source hash，用于同输入证据回放，不是可查询的 server-side ID；`facts_hash` 只覆盖版本化 canonical facts，使等价 JSON/Text 可以证明领域事实一致，但不宣称两份完整 Report 字节相同。

`facts_hash` 纳入 effective version/pack、ProfileContext、canonical metric state/value/unit/fidelity/bounds、graph facts、derived facts 和 rule/finding receipts；排除 filename、source hash/kind、SourceView 文本、source locator 和纯 display 字段。任一有损 fidelity、语义或算法 receipt 不同都必须产生不同 facts_hash。

`analysis_state=complete` 仅当 detected version 命中 compatible exact pack、ProfileContext 为 level_1 + final + merged、所有 v0.1 required completeness dimension 为 complete，且无 error/partial diagnostic；其余仍有可信事实的响应一律 partial。该状态是证据完整性，不是 query health。

`SourceView` 是专用安全 DTO，只允许 `plain_text_lines[]`/JSON token、bounded source range 和稳定 source ref；不含 HTML。前端以文本节点渲染，不接收散落于 MetricObservation 的 `raw_text`、CounterStrategy、merge recipe 或内部实体集合。

## 3. InputSummary

```text
InputSummary
├─ requested_kind        auto | json_profile | text_profile
├─ detected_kind         json_profile | text_profile
├─ input_origin          local_file | pasted | connected_profile_fetch
├─ connected_query_ref   safe projection | null
├─ filename              sanitized basename | null
├─ byte_length
├─ line_count            known for text, else null
├─ source_hash           sha256 hex
└─ adapter_evidence[]
```

文件名只作显示和 provenance，不参与 kind/version 检测。原始 Profile 不进入日志、trace、telemetry 或分析 key 之外的持久状态。

`input_origin` 不改变 parser 选择；connected fetch 的 Text Profile 仍进入唯一 Text Adapter。`ConnectedQueryRef` 只含 query ID、cluster/FE fingerprint、时间/状态、digest/数据库/resource group 的安全投影和 source receipt，不含 credential、完整 SQL 或 host user。

## 4. VersionContext：唯一 wire 模型

```text
VersionContext
├─ reported_version      {value, source_ref} | null
├─ user_declared_version request hint | null
├─ effective_version     evidence-backed or hint fallback | null
├─ version_source        detected | user_declared | unknown
├─ selected_pack         {pack_id/version, official release/commit, manifest hash} | null
├─ pack_match            exact_pack | no_pack
├─ compatibility         compatible | partial | incompatible | inconclusive
├─ support_status        supported | parse_only | unsupported | unknown
├─ detection_evidence[]
├─ compatibility_checks[]
│  └─ {check_id, domain, state=matched|missing|unknown|conflict, expected/observed, source_refs[]}
└─ issues[]
```

规则：

- 输入自身 reported version 属于 detected evidence；request hint 单独保留，不能覆盖 reported。二者冲突时 compatibility check=conflict。
- `supported` 仅在 detected evidence 指向 3.5.14、命中 exact pack 且 compatibility=compatible 时成立。
- `user_declared` 可以选择 3.5.14 候选 pack，但最高为 `parse_only`，除非后续又取得独立 detected evidence；不得直接变成 supported。
- known other version + no pack 为 unsupported；无可靠版本事实为 unknown。
- unknown/unsupported 不自动套最近 pack。可解析的通用结构仍以 partial Report 返回，但依赖 3.5.14 的规则 not_applicable。
- 不再保留 `resolution_state/base_pack/delta_packs` 等第二套平行真值。

VersionPack 内容固定为 detection evidence、aliases、dimension/unit、aggregation/merge semantics、structural evidence 和官方 commit；不得包含产品 severity、recommendation 或 RuleSet。

### 4.1 ProfileContext

```text
ProfileContext
├─ profile_level         level_1 | level_2 | unknown
├─ query_state           finished | running | error | unknown
├─ snapshot_kind         final | runtime | unknown
├─ merge_state           merged | unmerged | unknown
├─ evidence_refs[]
└─ limitations[]
```

3.5.14 `pipeline_profile_level=2` 保留原始结构并不满足官方可视化分析前提；Running 只是周期快照。二者必须传播到 compatibility/completeness/rule applicability，图可 partial/unavailable，不能按最终 merged Profile 给确定结论。

## 5. MetricObservation 与 MetricCell

### 5.1 内部领域值

`MetricObservation` 只存在于 backend `NormalizedIR`/Compare evaluator，不是 public wire DTO；UI 对应投影只有 5.2 的 `MetricCell`。Report copy 的 `RuleEvaluation.input_observation_refs` 必须按原有顺序投影为唯一的 `MetricCell.metric_cell_ref`；内部 IR/evaluator 保留 observation ID。缺失或歧义容器是 projection error，`metric:{subject}:{metric}` 冲突使用 observation identity 的确定性不透明后缀，禁止首行选择或覆盖。

```text
MetricObservation
├─ observation_id
├─ metric_id              nullable for unknown metric
├─ source_name
├─ subject_ref
├─ dimension             duration | bytes | count | ratio
├─ parse_state           exact | missing | invalid | partial | overflow
├─ canonical_number      CanonicalNumber | null
├─ canonical_unit        ns | byte | item | ratio | ... | null
├─ raw_text              string | null
├─ source_refs[]
├─ consumed_ranges[] / unconsumed_ranges[]
├─ fidelity              exact | rounded | truncated | interval | unknown
├─ bounds                BoundedInterval | null
├─ semantic_state        known | aliased | unknown_metric | incompatible | unavailable
├─ definition_ref        {metric_id, registry_version, pack_id} | null
├─ scope                 query | fragment | pipeline | operator | driver
├─ value_role            reported_base | reported_min | reported_max | derived
├─ strategy_origin       source | registry | unknown
├─ counter_strategy      CounterStrategy | null
├─ source_merge_state    merged | unmerged | unknown
├─ display_value
├─ order_rank            uint32 | null
└─ diagnostics[]

CanonicalNumber
├─ kind                  integer | decimal
├─ value                 base-10 string
└─ scale                 integer, decimal only

BoundedInterval
├─ lower / upper         CanonicalNumber
├─ lower_inclusive       bool
└─ upper_inclusive       bool
```

```text
CounterStrategy
├─ tunit
├─ aggregate_type        sum | avg | sum_avg | avg_sum
├─ merge_type            merge_all | skip_all | skip_first_merge | skip_second_merge
├─ min_max_type          min_max_all | skip_all
└─ display_threshold
```

JSON 不直接使用 IEEE-754 number 承载 counter，避免超过 JavaScript safe integer。`unknown_version` 是 VersionContext 原因，不是 parse state；因版本无法解释的值使用 `semantic_state=incompatible/unavailable`。禁止 `defaulted` 规范值。

`parse_state=exact` 只证明 lexeme 完整消费，不证明原始 Counter 无损。3.5.14 Text Profile 的 bytes 会格式化到三位小数，time 随量级截断到 us/ms/s/min，因此必须携带 fidelity/bounds；原始 counter、括号 raw count 或可证明单射/无损的 formatter 分支（例如 us 带完整 ns remainder）才可 fidelity=exact。partial 不带 canonical number，不进入聚合、规则或数值 Compare。exact zero 表示 parse exact 且 canonical integer `"0"`。

Rounded/truncated source interval 保留真实端点包含性，通常为 `[lower, upper)`；derived 运算必须传播端点，无法精确保留时向外扩成保守闭区间。dimension 不同不得进入同一区间运算；所有计算使用 decimal/rational，不经 f64。为避免除数无限逼近 0，契约中的“interval 含 0”按保守 closure 判断：即使 0 是 open endpoint 也进入 contains-zero/invalid-denominator 分支；只有区间严格远离 0 才可相除。

### 5.2 前端单元格

```text
MetricCell
├─ metric_cell_ref       stable within canonical Report
├─ metric_id
├─ display_text          backend formatted text or em dash
├─ state                 UI projection of parse/semantic state
├─ canonical_number      evidence/export only; UI 不做数学
├─ canonical_unit
├─ fidelity / bounds     BoundedInterval | null
├─ order_rank            uint32 | null
├─ source_refs[]
└─ reasons[]
```

Operators/Compare 列表的 `order_rank` 由后端在同一 collection/metric 内按 canonical number/interval 稳定计算；相同值同 rank，再以稳定 ID 破同。前端排序只使用 rank，不从 display/raw/canonical string 自行转数字。不可比较或区间重叠无法全序的值 rank 为 null，并显示 approximate/unknown。

### 5.3 显示单位规则（v0.6-09，产品决策）

后端写给读者的所有字节、耗时和计数，以及前端从 canonical 数值格式化的 Compare / Audit 数值，遵守同一套规则。后端由 `doctor-core::display` 实现，前端由 `frontend/src/lib/duration-display.ts` 实现。两边都用 `fixtures/expected/display-units.json` 中的同一组向量测试，改规则必须同时改两边和向量。

- **耗时**：单位为 ns / μs / ms / s，以 1000 进位。
- **字节**：单位为 B / KB / MB / GB / TB / PB / EB，按 StarRocks 的二进制含义，以 1024 进位，1 MB = 1,048,576 字节。这与 StarRocks Profile 和 ANALYZE PROFILE 的写法一致，同一个值在本产品里和在原始 Profile 里读起来相同。不使用 KiB / MiB 标签，也不按十进制换算。
- **小数位**：最多 3 位，四舍五入（half-up），去掉末尾的 0；四舍五入后等于进位基数时进到下一级单位。
- **计数**：用千分位逗号分组。
- **负数**：用 U+2212 “−”。
- **区间**：
  - 宽度不超过两个显示刻度的区间（即 StarRocks 打印值本身的四舍五入区间），显示为中点值，写法与原文相同；
  - 更宽的区间写成 `a – b`；
  - 精确端点始终保留在 `bounds` 中。
- **原文写法的四舍五入字节**（如 `122.070 KB`）：先换回最接近的整数字节，再按本规则显示，结果数字不变，只去掉末尾的 0。
- **原文写法的四舍五入计数**（如 `1.000M (1000000)`）：保留原文，因为换成整数会夸大精度。
- **执行汇总节点耗时**：与指标单元一致，保留 3 位小数（此前为 2 位）。

## 6. Metric Registry 与聚合

```text
MetricDefinition
├─ metric_id
├─ aliases_by_pack[]
├─ dimension / canonical_unit
├─ valid_scopes[]
├─ applicability
├─ extraction_recipe_id/version
├─ selection_recipe_id/version
├─ combine_recipe_id/version
├─ ordered_source_contract  preserve_source_ordinal | order_irrelevant
├─ source_counter_strategy
├─ analysis_aggregation  sum | max | min | min_max_range | weighted | non_additive
├─ fidelity_policy
├─ known_issues[]
└─ display_policy
```

官方 Counter 有两个合并阶段：BE/FE 按 `aggregate_type + merge_type + min_max_type` 产生 reported base/min/max；产品随后才按 VersionPack 的 extraction/selection/combine recipe 和 `analysis_aggregation` 构造分析事实。两阶段不得压成一个 sum/max 枚举，也不得对已经 merged 的值重复合并。`__MIN_OF_` / `__MAX_OF_` 以独立 observation/value_role 保留。

未知字段默认 non_additive/unavailable。Query wall time、并行 operator active time、ScanTime 和 NetworkTime 不能互相替代。3.5.14 官方 `QueryCumulativeOperatorTime`、`QueryCumulativeScanTime`、`QueryCumulativeNetworkTime` 分别登记，不因 Overview 需要而强行相加。

output rows 也不是字段名直映：final sink、Union、普通节点和 broadcast Exchange 在 3.5.14 ExplainAnalyzer 使用不同 Push/Pull/Instance 选择/组合路径。VersionPack 只登记固定 Rust evaluator 的 versioned recipe ID，不构建通用 DSL；无匹配 recipe 时 unavailable，禁止猜测。

一个 plan node 可映射多个 native/subordinate pipeline operators；`plan_node_id` 不是 Operator 的唯一主键。

## 7. Completeness、Diagnostics 与 Provenance

```text
CompletenessReport
├─ summary               DimensionCompleteness
├─ metrics               DimensionCompleteness
├─ graph                 DimensionCompleteness
├─ version               DimensionCompleteness
└─ provenance            DimensionCompleteness

DimensionCompleteness
├─ state                 complete | partial | unavailable
├─ present_count
├─ expected_count        known denominator | null
└─ reasons[]
```

expected_count 为空时不计算百分比，也不造总体完整度分。

```text
SourceRef
├─ ref_id
├─ source_kind           json_profile | text_profile
├─ locator               JsonLocator {json_pointer, byte_range?}
│                       | TextLocator {line_start/end, byte_start/end}
└─ redaction_state       original_local | redacted

SourceView               public wire, text-only
├─ source_kind
├─ plain_text_lines[]    {line_no, text, source_ref}
├─ json_tokens[]         optional {kind, text, source_ref}
├─ truncation_state      complete | truncated
├─ limitations[]
└─ line_refs[]           {source_ref, line_no}；报告内每个单行文本引用的行号（v0.6-07），取自 adapter 的类型化 TextLocator，不携带行文本

Diagnostic
├─ code
├─ stage                 request | detect | parse | normalize | graph | rule | compare
├─ severity              error | warning | info
├─ recoverability        fatal | partial | informational
├─ message_id
├─ source_refs[]
├─ affected_ids[]
└─ details               allowlisted scalar map
```

`details` 不含完整原文、SQL、栈或任意反序列化对象。SourceView 仅为同一响应的安全只读文本/JSON token 模型，不包含 HTML。

## 8. Fragment、Pipeline、OperatorGroup 与 ExecutionGraph

```text
OperatorGroup
├─ operator_group_id     deterministic IR ID
├─ plan_node_ref         nullable, not unique across groups
├─ fragment_ref / pipeline_ref
├─ native_operator_refs[]       OrderedOperatorRef
├─ subordinate_operator_refs[]  OrderedOperatorRef
├─ canonical_type / raw_labels[]
├─ metric_observation_refs[]
└─ source_refs[]

OrderedOperatorRef
├─ operator_ref
├─ source_ordinal        uint32
├─ source_role           native | subordinate
└─ source_refs[]

ExecutionGraph
├─ state                 complete | partial | unavailable
├─ topology_source       execution_topology | embedded_plan | verified_profile_relation | none
├─ root_node_ids[]
├─ nodes[]               ExecutionGraphNode
├─ edges[]               ExecutionEdge
├─ unconnected_node_ids[]
├─ unresolved_relations[]
├─ source_refs[]
└─ completeness

ExecutionGraphNode
├─ node_id
├─ entity_kind           plan_node | operator_group | pipeline | fragment | sink
├─ plan_node_id / fragment_id / pipeline_id  nullable source identities
├─ plan_node_type / display_name             nullable
├─ native_operator_refs[] / subordinate_operator_refs[]  OrderedOperatorRef
├─ metric_observation_refs[]
├─ execution_state       nullable
├─ source_quality        exact | partial
└─ source_refs[]

ExecutionEdge
├─ edge_id
├─ from / to
├─ relation              plan_child | fragment_sink | exchange_link | dependency | verified_data_flow
├─ direction_semantics   upstream_to_downstream | parent_to_child | dependency_precedes
├─ source                explicit | verified_derived
├─ quality               exact | partial
├─ source_ordinal        uint32 | null
├─ input_role            probe | build | child | sink | source | null; only with evidence
├─ derivation_recipe     {id, version} | null
├─ restates_edge_id      fragment_sink 与反向 plan_child 重复时指向后者 | null
└─ evidence_refs[]
```

运行时 Topology 的 `fragment_id`（v0.6）：以 rootId、带 `sinkIds` 的节点与 EXCHANGE 子节点为界切分子树，再由原生非 `*_SINK` 算子把每段绑定到 Profile 的 `Fragment N`；证据缺失时为 null，矛盾时为 null 并给出 `fragment_attribution_conflict`。

Fragment、Pipeline、OperatorGroup 和 PlanNode 是不同实体；一个 plan node 可对应多个 native/subordinate operator group，Exchange 也可能跨 fragment 归并。不得用 singular operator_type 覆盖 plan node，也不得用 RuntimeProfile indentation 假装官方 `ProfilingExecPlan` topology。

`ProfilingExecPlan.children`、fragment `sinkIds`、`operatorProfiles` 是语义有序集合。对应 refs 使用 `{ref, source_ordinal}`，ExecutionEdge 保存 child/sink ordinal；input_role 只有源码/输入明确区分 probe/build 等角色时填写。canonicalization 对语义有序数组按 source_ordinal 保序，禁止为了 hash 按 ID 重排。

`verified_derived` 只有在输入事实和 versioned recipe 唯一确定关系/方向时产生。plan child 不自动等于 data flow；歧义进入 unresolved。graph unavailable 时 edges 必为空，OperatorGroups/Source/Diagnostics 仍可用。v0.1 不做 fuzzy topology 或通用 property graph。

v0.1 首批 operator catalog 只保证 SCAN、JOIN、EXCHANGE、AGGREGATE 的规范字段；其他 operator 保留 canonical `unknown/other`、raw label、typed metrics 和 provenance，不因此整份失败。SORT/PIPELINE/CBO 专项域后置，不预建完整模型。

## 9. DerivedMetric

```text
DerivedMetric
├─ metric_id / formula_version
├─ subject_ref
├─ inputs[]              MetricObservation refs
├─ state                 computed | unavailable | invalid_denominator | incomparable
├─ canonical_number/unit | null
├─ fidelity / bounds     BoundedInterval | null
├─ assumptions[]
└─ evidence_refs[]
```

分母 exact 0 或 bounded interval 含 0 时由公式返回 `invalid_denominator`；unknown/no bounds 为 unavailable。禁止统一 `max(denominator,1)`。非零 bounded division 用保守 rational interval 并传播端点包含性，不经 f64。v0.1 只实现被已验收 Finding 或 Compare 使用的派生指标，不预建全套评分维度。

## 10. RuleSet、RuleEvaluation 与 Finding

RuleSet 独立于 VersionPack：

```text
RuleSetManifest
├─ rule_set_id / version / manifest_hash
├─ ir_schema_range
├─ compatible_pack_ids[]
├─ registry_requirements[]
└─ rules[]               RuleDefinition

RuleDefinition
├─ rule_id / version / category
├─ evaluator_id / version
├─ subject_kinds[]
├─ applicability         pack/profile/graph/operator/required metrics
├─ typed_input_bindings[]
├─ thresholds[]          typed value/unit/origin/evidence ref
├─ severity_policy / confidence_policy
├─ limitations[]
└─ recommendation_template
   ├─ action_kind / suggested_action
   ├─ risk / expected_direction[]
   └─ validation_steps

RuleEvaluation
├─ rule_id / rule_version
├─ status                triggered | suspected | not_triggered | not_applicable | suppressed
├─ subject_ref
├─ applicability_checks[]
├─ input_observation_refs[]
├─ comparisons[]
├─ threshold_receipts[]
├─ missing_prerequisites[]
├─ evidence_refs[]
├─ limitations[]
└─ finding_id            null unless triggered/suspected

Finding
├─ finding_id
├─ rule_id / rule_version
├─ subject_ref
├─ category
├─ severity              critical | severe | high | moderate | mild | info
├─ confidence            high | medium | low
├─ confidence_reasons[]
├─ evidence[]
├─ limitations[]
├─ recommendation
├─ related_finding_refs[] only with shared evidence
├─ rule_trace_ref
└─ measurements[] measurement_id / display_text / threshold_text / operator_type / input_refs
```

`measurements` 由规则求值器从它比较过的同一组类型化输入算出（倾斜比、扇出比、广播字节、预聚合保留率、耗时占比等），文本由后端格式化，区间输入给区间；`input_refs` 是公开 metric cell 引用。前端原样展示，不重算；它不进入 facts 哈希（v0.6-04）。

severity 表示潜在影响，confidence 表示证据确定性，二者不得绑定。recommendation 只含证据约束的人工 action、风险、验证步骤和预期方向，不包含自动执行结果。来源必须是 `product_rule` 或安全投影的 `starrocks_plan_advisor`；后者不能伪装成本产品推理。

v0.1 RuleSet 首批候选只保留能用 3.5.14 golden 完整证明的少量规则：

1. `SR3514.OPERATOR_TIME_SHARE`：按官方 ExplainAnalyzer 计算 `node_time / QueryCumulativeOperatorTime`。node_time 汇总 source-ordered native/subordinate `OperatorTotalTime`，Exchange 另取 native `NetworkTime`，Scan 另取 native `ScanTime`；阈值严格为 `>30%` 与 `>15%` display attention，等于 30 落入第二档、等于 15 不触发。分母缺失、exact 0、interval 含 0、semantic unavailable、必要 metric parse 非 exact 或 unknown fidelity 时 not_applicable。对可证 ratio interval 统一三值判断：全集满足 bucket=triggered，全集不满足=not_triggered，与 15/30 任一边界相交=suspected；不得用 representative 决定。产品 severity 是独立产品决策，不称 query health，并披露并行/异步时间与 wall time 不同的 limitation。
2. `SR3514.SPILL_OBSERVED`：只在已登记 `QuerySpillBytes`/operator spill observation 满足“fidelity exact 且 canonical >0”或“有损值 lower bound >0”时 triggered；有损区间包含 0 时 suspected，exact 0 为 not_triggered，缺失/invalid 不当作 0。建议只给人工内存/算子验证步骤，不自动改配置。

如果任一规则无法建立 official source -> registry -> typed input -> golden expected 的完整链路，则从 v0.1 manifest 移除，不以规则数量补位。

除两条已冻结 runtime 规则外，v0.1 Recommendation Center 可投影官方 Plan Advisor join/pre-aggregation guide。scan/filter/storage 与 exchange/network 只有在 3.5.14 registry、typed applicability 和六类 golden 完整时才升级为产品 Finding；否则只能显示 `investigation`，不能输出具体 SQL/参数修改。

## 10.1 Connected plan evidence

```text
ExplainSnapshot
├─ schema_version / explain_receipt
├─ plan_key / source_hash
├─ detected_version / pack_match
├─ explain_level         costs
├─ plan_fingerprint
├─ environment_fingerprint
│  ├─ catalog / database
│  ├─ session_allowlist_hash
│  ├─ metadata_fingerprint_state
│  └─ statistics_fingerprint_state
├─ nodes[] / edges[] / registered_properties[]
├─ diagnostics[] / limitations[]
└─ provenance_index[]

CorrelationEvidence
├─ state                 verified | partial | unavailable
├─ basis[]
├─ baseline_refs[] / candidate_refs[]
└─ limitations[]

OptimizationExperimentContext
├─ change_summary        user supplied, bounded and redacted
├─ controlled_dimensions[]
├─ baseline_profile_ref
├─ candidate_explain_ref optional, plan-only
├─ candidate_profile_ref optional, obtained after user-side rerun
├─ rerun_responsibility  external_user
└─ comparison_conditions[] matched | mismatched | unknown
```

Explain 与 Profile 不是同一 DTO。`environment_fingerprint` 缺失或变化会降低 PlanComparison/causal verdict，而不会阻止用户查看结构差异。缺少 verified correlation 时禁止按名称、文本顺序或近似 ID 猜 operator 映射。`OptimizationExperimentContext` 记录的是用户侧复跑的控制条件与证据关联，不是产品执行 SQL 的 receipt；产品 API 不产生 execution receipt。

## 11. ComparisonSnapshot

```text
AnalysisAttempt
├─ outcome              AnalysisOutcome | null
└─ error                ErrorSummary | null

ComparisonSnapshot
├─ schema_version=1.1 / comparison_receipt
├─ comparison_key       deterministic hash
├─ baseline             AnalysisAttempt
├─ candidate            AnalysisAttempt
├─ state                comparable | partially_comparable | not_comparable
├─ reasons[]
├─ compatibility_delta
├─ structure_delta
├─ metric_deltas[]
├─ finding_delta
├─ validation_context
│  ├─ query_identity / digest
│  ├─ version / data_window / session / resource_group / cluster
│  └─ each state matched | mismatched | unknown
├─ diagnostics[]
└─ limitations[]

StructureDelta
├─ state
├─ matches[]             baseline/candidate refs + exact match basis
├─ added[] / removed[] / changed[]
├─ edge_delta[]
└─ unresolved[]

MetricDelta
├─ metric_id / subject_match
├─ state                comparable | not_comparable
├─ baseline/candidate metric_cell_refs
├─ absolute_delta       CanonicalNumber | null
├─ percentage_state     computed | undefined_baseline_zero | undefined_baseline_interval_contains_zero | missing | invalid | semantic_mismatch | approximate
├─ percentage_value     CanonicalNumber | null
├─ fidelity              exact | interval | null
├─ bounds                absolute-delta BoundedInterval | null
├─ percentage_bounds     percentage-delta BoundedInterval | null
├─ order_rank           uint32 | null
└─ limitations[]

FindingDelta
├─ added[] / resolved[] / persisting[] / changed[]
├─ incomparable[]
├─ algorithm_changed[]
├─ partial_evidence[]
└─ measurement_changes[] {finding_entry, measurement_id, baseline_display_text?, candidate_display_text?, baseline_threshold_text?, candidate_threshold_text?}
```

`measurement_changes`（v0.6-07）按 `measurement_id` 配对 added/resolved/persisting/changed 各条目两侧 `Finding.measurements` 的展示文本，原样复制、不相减；一侧没有该发现或该度量时为 null，不补 0；两侧规则条件分别给出（同一规则不同严重度档位时二者不同）。它是展示投影，不改变 finding 分流、comparison key 或比较器语义清单。

Comparator `0.6.0` 只在另一侧存在唯一的同 `rule_id`、同 `subject_ref`、同 `rule_version` 的 `not_triggered` 评估，其证据为 `complete` 或 `partial` 且 `missing_prerequisites` 为空，并且 StarRocks 版本、VersionPack、metric registry 可比时，才把缺席的 finding 判为 `resolved` 或 `added`。finding 本身的 `partial` 证据不阻止判定；任一侧证据 `unavailable`、未评估、多条匹配评估、`suppressed`、`not_applicable`、不支持的 Level 2／运行中快照仍不可比。这是产品决策，不是官方优化效果判定。

`partial_evidence` 的元素格式为 `rule_id:subject_ref`，仅列出 `resolved`／`added` 中 finding 或唯一负向评估证据为 `partial` 的条目，顺序与对应判定遍历顺序一致。后端始终输出数组（没有则为 `[]`）；读取旧 Comparison 时缺省为空，生成的 TypeScript 因兼容旧 Comparison 将该读取字段标为可选，旧回执不因此变成 `0.6.0` 回执。前端按此字段显示“证据部分”，不得从整体 `analysis_state=partial` 推断。两侧均为 `complete` 的判定不加该标注。

`AnalysisAttempt` 必须且只能满足二选一：有 outcome、无 error 时状态由 `outcome.report.analysis_state` 唯一推导为 complete/partial；无 outcome、有 error 时推导为 failed。wire 不再保存可矛盾的 attempt state。两侧 attempt 独立；外层请求合法时单侧 failed 仍可进入 HTTP 200 Comparison，失败侧只有脱敏 ErrorSummary。结构只按稳定 ID/明确映射/版本化 exact recipe 匹配，v0.1 不做 fuzzy node match。

Profile/Profile 才能形成 runtime metric/finding delta。Explain/Explain 使用独立 `PlanComparison`，只报告结构/登记属性 changed/unchanged。Profile/Explain 通过 `CorrelationEvidence` 提供结构关联；所有 runtime delta 必须 `not_comparable_missing_runtime_profile`。即使两侧 Profile 完整，`validation_context` 有 mismatched/unknown 时也只能说 observed change，不能宣称某次修改造成改善。

`SCHEMA_VERSION=1.0` 只表示 AnalysisSnapshotReport/AnalysisAttempt/Capabilities；Comparison 独立使用 `COMPARISON_SCHEMA_VERSION=1.1`。ComparisonReceipt 在 1.1 必须由本地 comparator 组装，恰含一个 `doctor-comparator-v0.1@0.1.0#<semantic_manifest_hash>`，请求体不能提供或覆盖它。

MetricDelta 的组合矩阵固定：absolute exact 只能是 `absolute_delta!=null + fidelity=exact + bounds=null`；absolute interval 只能是 `absolute_delta=null + fidelity=interval + bounds!=null`；无 absolute 值时三者都为 null。`bounds` 永远是 absolute-delta interval，不能复用为 percentage interval。percentage computed 只能有 `percentage_value`，approximate 只能有 `percentage_bounds`，其余 percentage state 两者均为 null；两类 bounds 可同时存在。

baseline exact 0 时保留 absolute delta，percentage 必须 `undefined_baseline_zero`。baseline bounded interval 含 0 时可保留可证 absolute interval，但 percentage 必须 `undefined_baseline_interval_contains_zero`，不得生成数值百分比。baseline interval 排除 0 且两侧有界时 percentage 为 approximate + conservative `percentage_bounds`。missing/invalid/partial/unavailable 不当作 0。有损 text value 传播 approximate/interval；不同 registry/pack 只有显式等价定义才可比。Finding 只有 rule/evaluator version 相同且 subject key 稳定时可判 added/resolved；否则进入 algorithm_changed/incomparable。

v0.1 有界 percentage 只在 baseline closure 严格为正时计算：lower 为 `candidate.lower / baseline.upper - 1`，upper 为 `candidate.upper / baseline.lower - 1`。输入端点恰可表示时包含性取对应输入端点的 conjunction；向外 floor/ceil 舍入时以保守闭端点表示。baseline closure 含 0（包括 open 端点）固定为 `undefined_baseline_interval_contains_zero`，不得给 `percentage_bounds`。

## 12. Canonical 序列化与兼容

- schema 使用 semantic major/minor；breaking wire change 升 major。
- key 排序、枚举 spelling、decimal/integer string、null/empty collection 策略固定后再计算 report/comparison hash；无序数组按稳定 ID 排序，`children/sinkIds/operatorProfiles` 等语义有序数组按 source_ordinal 保序。
- Reader 可忽略未知扩展字段；v0.1 DTO 不承诺代理或 round-trip 后仍保留这些未知字段，core 不把未知字段自动升级为已知指标。
- schema、adapter、parser、registry、graph、pack、ruleset 任一行为变化都必须升级对应版本并重放旧 corpus。
- 前端只消费生成类型；Rust/TypeScript 对同一 fixture 的 round-trip canonical JSON 和 hash 必须一致。

本契约冻结只表示可以开始 Task 01 scaffold/schema/golden，不表示 Analyzer 或 v0.1 Candidate 已可用。
