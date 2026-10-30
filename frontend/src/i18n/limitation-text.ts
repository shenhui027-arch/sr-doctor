import type { LocalizedText } from "./locale";

// Readable text for the closed set of limitation codes the backend attaches to
// Queries / Audit / Profile-list / running / fetch responses. The backend
// decides which codes apply; the browser only translates them. Unregistered
// codes are returned as null so the caller can keep them in technical details
// instead of showing a raw identifier in the main view.
const limitations: Record<string, [string, string]> = {
  active_at_participants_are_deterministic_bounded_not_a_full_sql_list: ["Concurrent queries are a bounded, deterministic sample, not the full list.", "并发查询是有上限的确定性样本，不是完整列表。"],
  advisor_rows_not_normalized_in_v0_1: ["Plan Advisor rows are shown as returned; this version does not normalize them.", "Plan Advisor 结果按原样展示，本版本不做规范化。"],
  audit_loader_ingestion_async_completeness_unknown: ["AuditLoader writes asynchronously, so the newest queries may not be listed yet.", "AuditLoader 异步写入，最新的查询可能还没有出现。"],
  audit_rows_not_normalized_in_v0_1: ["Audit rows are shown as returned; this version does not normalize them.", "审计行按原样展示，本版本不做规范化。"],
  audit_schema_unavailable: ["The audit table structure could not be read.", "无法读取审计表结构。"],
  audit_timestamp_normalization_partial: ["Some audit timestamps could not be converted to UTC.", "部分审计时间无法换算为 UTC。"],
  audit_timestamp_time_zone_unverified: ["The time zone of audit timestamps was not verified.", "审计时间的时区未经核实。"],
  audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch: ["UTC times are interpreted in this read-only session's time zone, not taken from the plugin's own clock.", "UTC 时间按本只读会话的时区换算，不是插件自身记录的时间点。"],
  bucket_quantiles_are_never_combined_across_buckets: ["Percentiles are exact within each bucket and are never combined across buckets.", "分位数在每一段内精确计算，段与段之间不合并。"],
  cpu_cost_sum_is_not_elapsed_time: ["Cumulative CPU adds up executions; it is not elapsed time.", "CPU 累计是多次执行相加，不是经过的时间。"],
  dimension_rankings_are_exact_top_n_per_key: ["Each ranking is the exact top values for its own key, ranked over every value.", "每种排序都是在全部取值上按该指标排出的精确 Top。"],
  empty_buckets_are_exact_zero_counts_for_this_read: ["A bucket with no audit rows shows an exact zero for this read.", "没有审计记录的时段在本次读取中是精确的 0。"],
  memory_cost_max_is_one_execution_not_concurrent_total: ["Peak memory is the largest single execution, not concurrent total memory.", "内存峰值是单次执行的最大值，不是并发总内存。"],
  starrocks_percentile_disc_returns_the_sorted_value_at_index_ceil_p_n_minus_1: ["Percentiles use StarRocks PERCENTILE_DISC: an observed value at sorted position ⌈p·(n−1)⌉, never interpolated.", "分位数用 StarRocks PERCENTILE_DISC：取排序后第 ⌈p·(n−1)⌉ 位的实际值，不插值。"],
  candidate_profile_user_external_rerun_is_user_responsibility: ["The candidate query was rerun outside this tool; matching it to the baseline is up to you.", "候选查询是在本工具之外重跑的，是否与基线对应需要你自行确认。"],
  candidate_user_external_rerun: ["The candidate query was rerun outside this tool.", "候选查询是在本工具之外重跑的。"],
  canonical_cohort_excludes_invalid_query_ids_and_conflicting_base_projections: ["Executions with invalid or conflicting query IDs are left out.", "查询 ID 无效或互相冲突的执行不计入。"],
  cluster_identity_is_session_scoped_not_cross_connection: ["The cluster identity holds only for this connection.", "集群标识只在本次连接内有效。"],
  coverage_and_top_are_separate_non_atomic_reads: ["Coverage and the ranking are read separately and may differ slightly.", "覆盖率与排行分两次读取，可能略有出入。"],
  cumulative_query_time_is_not_wall_clock_time: ["Cumulative query time adds up executions; it is not elapsed time.", "累计查询时间是多次执行相加，不是经过的时间。"],
  exact_native_digest_is_not_semantic_sql_clustering: ["A query family is an exact StarRocks digest match, not a grouping of similar SQL.", "查询族按 StarRocks digest 精确匹配，不是相似 SQL 的聚类。"],
  full_window_aggregation_precedes_top_limit: ["Totals cover the whole time window before the top limit is applied.", "汇总覆盖整个时间窗口，之后才截取前 N 名。"],
  historical_terminal_intervals_are_approximate_not_live_or_blocking: ["Concurrency is estimated from finished queries; it is not live and does not show blocking.", "并发度由已结束的查询估算，不是实时数据，也不表示阻塞关系。"],
  list_observed_state_and_profile_lifecycle_are_independent: ["A query's listed state and whether its Profile still exists are independent.", "列表中的查询状态与其 Profile 是否仍存在相互独立。"],
  no_match_in_bounded_profile_evidence_does_not_prove_profile_absence: ["No Profile was found among the checked candidates; one may still exist.", "在检查过的候选中没有找到 Profile，但它仍可能存在。"],
  observed_changes_do_not_prove_causal_improvement: ["A measured change does not prove that your edit caused it.", "观察到的变化不能证明是你的修改造成的。"],
  official_interpretation_not_raw_profile: ["This is StarRocks' own interpretation, not the raw Profile.", "这是 StarRocks 官方的解读，不是原始 Profile。"],
  hours_not_collected_are_never_interpolated: ["Hours that were not collected are left empty, never interpolated.", "未采集的小时留空，不插值。"],
  only_executions_attributed_to_a_template_are_stored: ["History keeps only executions that could be attributed to a SQL template.", "历史库只保存能归入 SQL 模板的执行。"],
  quantiles_are_not_combined_from_hourly_aggregates: ["Percentiles are not combined from hourly aggregates, so the 30-day view shows none.", "分位数不能由小时聚合合成，30 天视图不提供分位。"],
  version_or_rule_changes_split_comparisons: ["A StarRocks version or rule change starts a new segment; segments are not compared.", "StarRocks 版本或规则变化会开始新的分段，分段之间不比较。"],
  weekday_and_hour_are_utc: ["Weekdays and hours are in UTC.", "星期与小时均按 UTC。"],
  overview_coverage_primary_concurrency_and_family_top_are_separate_non_atomic_reads: ["Overview sections are read separately and may differ slightly.", "概览各部分分开读取，可能略有出入。"],
  plan_advisor_not_in_release: ["This StarRocks release has no Plan Advisor (added in 3.4).", "当前 StarRocks 版本没有 Plan Advisor（3.4 起提供）。"],
  detected_version_not_registered_for_explain_costs: ["The plan graph is drawn only for StarRocks 3.5.14; other releases show the plan text.", "计划图只对 StarRocks 3.5.14 绘制；其他版本显示计划原文。"],
  detected_version_not_registered_for_explain_logical: ["The LOGICAL plan is parsed only for StarRocks 3.5.14; other releases show the plan text.", "LOGICAL 计划只对 StarRocks 3.5.14 解析；其他版本显示计划原文。"],
  detected_version_not_registered_for_explain_verbose: ["The VERBOSE plan is parsed only for StarRocks 3.5.14; other releases show the plan text.", "VERBOSE 计划只对 StarRocks 3.5.14 解析；其他版本显示计划原文。"],
  profile_cache_availability_is_observed_at_fetch_time: ["Whether a Profile is still cached is known only when you fetch it.", "Profile 是否仍在缓存中，只有在拉取时才能确定。"],
  profile_evidence_and_audit_detail_are_non_atomic_reads: ["Profile evidence and audit details are read separately and may differ slightly.", "Profile 证据与审计详情分开读取，可能略有出入。"],
  profile_list_duration_not_normalized_in_v0_1: ["Profile list durations are shown as returned.", "Profile 列表中的耗时按原样展示。"],
  profile_list_observation_time_unknown: ["When the Profile list entries were recorded is unknown.", "Profile 列表条目的记录时间未知。"],
  profile_list_rows_rejected_by_shape: ["Some Profile list rows had an unexpected shape and were left out.", "部分 Profile 列表行格式异常，未采用。"],
  profile_list_source_at_remote_limit: ["StarRocks returned the maximum number of Profiles; older ones are not listed.", "StarRocks 返回的 Profile 已达上限，更早的未列出。"],
  profile_list_time_zone_unverified: ["The time zone of Profile list times was not verified.", "Profile 列表时间的时区未经核实。"],
  profile_observation_time_unknown: ["When this Profile was recorded is unknown.", "该 Profile 的记录时间未知。"],
  profile_raw_text_omitted_by_contract: ["The raw Profile text is not returned here.", "此处不返回原始 Profile 文本。"],
  query_status_from_discovery_not_fresh_probe: ["Query status comes from the list, not a fresh check.", "查询状态来自列表，不是重新检查的结果。"],
  query_status_from_verified_standard_execution: ["Query status comes from the verified execution.", "查询状态来自已核实的执行。"],
  query_text_omitted_by_contract: ["The SQL text is not returned here.", "此处不返回 SQL 文本。"],
  raw_connector_profile_bytes_not_returned_by_connected_fetch_response: ["The raw Profile bytes stay on the server.", "原始 Profile 字节留在服务端，不返回。"],
  raw_connector_profile_bytes_not_returned_by_connected_profile_comparison: ["The raw Profile bytes stay on the server.", "原始 Profile 字节留在服务端，不返回。"],
  result_rows_joined_with_newline: ["Result rows are joined line by line.", "结果行按行拼接。"],
  running_profile_candidates_locally_truncated: ["The running query list was cut to the display limit.", "运行中查询列表已按显示上限截断。"],
  running_snapshot_requires_current_session_generation: ["A running snapshot needs the current connection; reconnecting invalidates it.", "运行中快照依赖当前连接，重新连接后失效。"],
  runtime_query_timeout_10_seconds: ["The controlled run stops after 10 seconds.", "受控执行 10 秒后停止。"],
  runtime_result_rows_are_discarded: ["The controlled run discards its result rows.", "受控执行会丢弃结果行。"],
  runtime_sql_only_bounded_location_excerpt_returned_not_persisted: ["Only a short SQL excerpt is returned for locating issues; nothing is stored.", "只返回用于定位的 SQL 片段，不做保存。"],
  session_time_zone_unavailable: ["The session time zone could not be read.", "无法读取会话时区。"],
  source_nonnegative_count_is_not_core_parser_validity_count: ["The count of non-negative values is not a count of valid values.", "非负值计数不等于有效值计数。"],
  time_zone_probe: ["Time zone was probed from the database.", "时区由数据库探测得到。"],
  verified_standard_attempt: ["Read with a verified standard query.", "通过已核实的标准查询读取。"],
};

export function limitationText(code: string, text: LocalizedText): string | null {
  const label = limitations[code];
  return label ? text(...label) : null;
}

/** One-line form for compact facts: readable sentences, unregistered codes kept verbatim. */
export function limitationSummary(codes: readonly string[], text: LocalizedText): string {
  return codes.map((code) => limitationText(code, text) ?? code).join(" ");
}
