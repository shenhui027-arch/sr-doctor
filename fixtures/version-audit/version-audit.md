# StarRocks 多版本源码审计

基线：3.5.14；审计标签：148 个（2.5.0～4.1.7）。本文件由 `scripts/version-audit/audit-starrocks-versions.mjs` 生成。

## 语义函数

同一指纹的连续版本合并为一段；与基线相同标 ✅。

| 函数 | 领域 | 分段 |
|---|---|---|
| `fe.print_counter` | profile_text_format | 2.5.0～3.0.9 `6286e5013d82`<br>3.1.0 `addd397e817d`<br>3.1.1～3.1.3 `7e9029029dfb`<br>3.1.4～4.1.7 ✅ |
| `fe.byte_unit` | profile_text_format | 2.5.0～3.1.0 `72f5c5c95cd3`<br>3.1.1～4.1.7 ✅ |
| `fe.count_unit` | profile_text_format | 2.5.0～3.1.0 `9e92192ec405`<br>3.1.1～4.1.7 ✅ |
| `fe.time_ms` | profile_text_format | 2.5.0～4.1.7 ✅ |
| `fe.pretty_print` | profile_text_structure | 2.5.0～2.5.22 `5f41956d052f`<br>3.0.0～3.1.0 `b0165faf87f1`<br>3.1.1～4.1.7 ✅ |
| `fe.text_formatter` | profile_text_structure | 2.5.0～2.5.22 不存在<br>3.0.0～3.0.5 `4481b141a44a`<br>3.0.6～3.0.9 `987d4fbd2ba2`<br>3.1.0 `2203259d3531`<br>3.1.1～3.1.2 `19573a5091dc`<br>3.1.3～3.2.13 `322266a555ee`<br>3.2.14～3.2.16 ✅<br>3.3.0～3.3.5 `322266a555ee`<br>3.3.6～4.1.7 ✅ |
| `fe.json_formatter` | profile_json_structure | 2.5.0～2.5.22 不存在<br>3.0.0～3.0.9 `c3211c2624ba`<br>3.1.0 `dc31ffacedb1`<br>3.1.1 `6efca8605219`<br>3.1.2～4.1.7 ✅ |
| `fe.merge_isomorphic` | profile_merge | 2.5.0～2.5.6 `ad30e2f9f938`<br>2.5.7～2.5.22 `eccddbf17c6d`<br>3.0.0～3.0.9 `b94fa626bf61`<br>3.1.0～3.1.2 `989a7846b94b`<br>3.1.3～3.2.9 `0fdedb75a71d`<br>3.2.10 `78a8916df39c`<br>3.2.11～3.2.12 `1db982c34e44`<br>3.2.13～3.2.16 `95857d2648f9`<br>3.3.0 `b3426ad7654b`<br>3.3.1～3.3.3 `fb7176e3d89f`<br>3.3.4～3.3.7 `d301fa6b7a94`<br>3.3.8～3.3.11 `8365abecabe8`<br>3.3.12～3.3.22 ✅<br>3.4.0～3.4.1 `8365abecabe8`<br>3.4.2～4.1.7 ✅ |
| `fe.remove_min_max` | profile_merge | 2.5.0～4.1.7 ✅ |
| `fe.compute_time_usage` | time_semantics | 2.5.0～3.0.9 不存在<br>3.1.0 `702939cf0272`<br>3.1.1～3.1.3 `dabedfa3a071`<br>3.1.4～4.1.7 ✅ |
| `fe.search_output_rows` | time_semantics | 2.5.0～3.1.3 不存在<br>3.1.4～4.1.7 ✅ |
| `fe.sum_up_metric` | time_semantics | 2.5.0～3.0.9 不存在<br>3.1.0～3.1.2 `2f7663ff8558`<br>3.1.3 `7276d8e6a69b`<br>3.1.4～4.1.7 ✅ |
| `fe.build_query_profile` | query_summary | 2.5.0～3.1.17 不存在<br>3.2.0～3.2.9 `e82470fa5ea7`<br>3.2.10～3.2.11 `8511d35af7e2`<br>3.2.12～3.2.16 `6fce635a6fec`<br>3.3.0～3.3.1 `9d4efaad2601`<br>3.3.2～3.3.5 `604beccaea8d`<br>3.3.6～3.4.0 `cbc8a00a61b9`<br>3.4.1～4.0.16 ✅<br>4.0.17 `623e8ae0d55b`<br>4.1.0～4.1.6 ✅<br>4.1.7 `623e8ae0d55b` |
| `be.profile_merge` | profile_merge | 2.5.0～2.5.22 `8806bb3a90a0`<br>3.0.0～4.1.7 ✅ |
| `be.counter_strategy` | profile_merge | 2.5.0～2.5.22 不存在<br>3.0.0～3.2.16 `2b8b4eee0766`<br>3.3.0～4.1.7 ✅ |
| `thrift.counter_strategy` | profile_merge | 2.5.0～2.5.22 `7b99171b984c`<br>3.0.0～3.1.7 `8339e75e8d1b`<br>3.1.8～3.1.17 `ed6d2fe91c79`<br>3.2.0～3.2.2 `8339e75e8d1b`<br>3.2.3～3.2.16 `ed6d2fe91c79`<br>3.3.0～4.1.7 ✅ |

## 注册指标的产生位置

| 指标 | 分段 |
|---|---|
| `QueryExecutionWallTime` | 2.5.0～2.5.22 无<br>3.0.0～4.1.7 有 |
| `QueryCumulativeCpuTime` | 2.5.0～2.5.22 无<br>3.0.0～4.1.7 有 |
| `QueryCumulativeOperatorTime` | 2.5.0～2.5.22 无<br>3.0.0～4.1.7 有 |
| `QueryCumulativeScanTime` | 2.5.0～3.1.2 无<br>3.1.3～4.1.7 有 |
| `QueryCumulativeNetworkTime` | 2.5.0～3.1.2 无<br>3.1.3～4.1.7 有 |
| `QueryPeakMemoryUsagePerNode` | 2.5.0～3.2.9 无<br>3.2.10～3.2.16 有<br>3.3.0～3.3.1 无<br>3.3.2～4.1.7 有 |
| `QueryAllocatedMemoryUsage` | 2.5.0～2.5.22 无<br>3.0.0～4.1.7 有 |
| `QueryDeallocatedMemoryUsage` | 2.5.0～2.5.22 无<br>3.0.0～4.1.7 有 |
| `QuerySumMemoryUsage` | 2.5.0～3.2.9 无<br>3.2.10～3.2.16 有<br>3.3.0～3.3.1 无<br>3.3.2～4.1.7 有 |
| `QueryPeakScheduleTime` | 2.5.0～3.1.2 无<br>3.1.3～4.1.7 有 |
| `ResultDeliverTime` | 2.5.0～3.0.9 无<br>3.1.0～4.1.7 有 |
| `FrontendProfileMergeTime` | 2.5.0～3.1.2 无<br>3.1.3～4.1.7 有 |
| `QuerySpillBytes` | 2.5.0～3.1.1 无<br>3.1.2～4.1.7 有 |
| `OutputChunkBytes` | 2.5.0～3.4.2 无<br>3.4.3～4.1.7 有 |

所有审计版本都存在的指标未列出。基线源码中没有字面量、由其他方式产生的指标：`QueryPlanningTime`、`QueryTotalTime`、`Active`、`OutputRows`。

## 连接侧能力

| 能力 | 分段 |
|---|---|
| `explain_analyzer` | 2.5.0～3.0.9 无<br>3.1.0～4.1.7 有 |
| `query_runtime_profile` | 2.5.0～3.1.17 无<br>3.2.0～4.1.7 有 |
| `json_profile` | 2.5.0～2.5.22 无<br>3.0.0～4.1.7 有 |
| `get_query_profile` | 2.5.0～3.0.1 无<br>3.0.2～4.1.7 有 |
| `show_profilelist` | 2.5.0～3.0.9 无<br>3.1.0～4.1.7 有 |
| `plan_advisor` | 2.5.0～3.3.22 无<br>3.4.0～4.1.7 有 |
| `audit_digest` | 2.5.0～4.1.7 有 |
| `pipeline_profile_level` | 2.5.0～4.1.7 有 |
| `execution_section_named_execution` | 2.5.0～3.0.9 无<br>3.1.0～4.1.7 有 |
| `query_state_profile_string` | 2.5.0～3.0.9 无<br>3.1.0～4.1.7 有 |
| `fragment_instance_ids` | 2.5.0～3.0.9 无<br>3.1.0～4.1.7 有 |
| `bare_operator_profile_name` | 2.5.0～3.1.2 有<br>3.1.3～4.1.7 无 |

## 审计事件字段

- **2.5.0～2.5.16**：24 个字段
- **2.5.17～2.5.22**：新增 IsForwardToLeader；删除 无
- **3.0.0～3.0.8**：新增 BigQueryLogCPUSecondThreshold、BigQueryLogScanBytesThreshold、BigQueryLogScanRowsThreshold；删除 IsForwardToLeader
- **3.0.9**：新增 IsForwardToLeader；删除 无
- **3.1.0～3.1.1**：新增 无；删除 IsForwardToLeader
- **3.1.2**：新增 SpilledBytes；删除 无
- **3.1.3～3.1.5**：新增 CandidateMVs、HitMvs；删除 无
- **3.1.6～3.1.17**：新增 IsForwardToLeader；删除 无
- **3.2.0**：新增 无；删除 IsForwardToLeader
- **3.2.1～3.2.16**：新增 IsForwardToLeader；删除 无
- **3.3.0～3.3.3**：新增 Warehouse；删除 无
- **3.3.4～3.3.12**：新增 Slots；删除 无
- **3.3.13～3.3.20**：新增 QueryFEAllocatedMemory；删除 无
- **3.3.21～3.3.22**：新增 Command、PreparedStmtId；删除 无
- **3.4.0**：新增 无；删除 QueryFEAllocatedMemory、Command、PreparedStmtId
- **3.4.1～3.4.2**：新增 Features、PredictMemBytes；删除 无
- **3.4.3～3.4.5**：新增 QueryFEAllocatedMemory；删除 无
- **3.4.6～3.4.9**：新增 TransmittedBytes；删除 无
- **3.4.10**：新增 Command、PreparedStmtId；删除 无
- **3.5.0～3.5.1**：新增 无；删除 TransmittedBytes、Command、PreparedStmtId
- **3.5.2～3.5.6**：新增 TransmittedBytes；删除 无
- **3.5.7～3.5.9**：新增 QuerySource；删除 无
- **3.5.10～3.5.11**：新增 Command、PreparedStmtId；删除 无
- **3.5.12～3.5.16**：新增 writeClientTimeMs；删除 无
- **3.5.17～3.5.21**：新增 QueriedRelations；删除 无
- **4.0.0～4.0.1**：新增 CNGroup、SessionId、CustomQueryId；删除 writeClientTimeMs、Command、PreparedStmtId、QueriedRelations
- **4.0.2～4.0.3**：新增 Command、PreparedStmtId、CacheHitRatio；删除 无
- **4.0.4～4.0.9**：新增 writeClientTimeMs；删除 无
- **4.0.10～4.0.17**：新增 QueriedRelations；删除 无
- **4.1.0**：新增 无；删除 QueriedRelations
- **4.1.1～4.1.4**：新增 CustomSessionName、QueriedRelations；删除 无
- **4.1.6～4.1.7**：新增 ErrorMessage；删除 无

## 触及监控文件的提交

| 区间 | 全部提交 | 触及监控文件 |
|---|---|---|
| 2.5.22 → 3.0.9 | 5290 | 93 |
| 3.0.9 → 3.1.17 | 6754 | 120 |
| 3.1.17 → 3.2.16 | 6179 | 125 |
| 3.2.16 → 3.3.22 | 6755 | 138 |
| 3.3.22 → 3.4.10 | 4570 | 96 |
| 3.4.10 → 3.5.14 | 3123 | 71 |
| 3.5.14 → 3.5.21 | 521 | 12 |
| 3.5.21 → 4.0.17 | 3980 | 69 |
| 4.0.17 → 4.1.7 | 4164 | 76 |

逐条提交见 `version-audit.json` 的 `intervals[].commits`。
