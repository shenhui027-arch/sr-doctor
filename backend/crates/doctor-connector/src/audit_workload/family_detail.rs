//! One bounded parent-scope read: exact distribution plus at most three roles.
use super::*;
use doctor_contracts::{AuditFamilyExecutionDistribution, AuditMetricState};
use doctor_core::{
    AUDIT_FAMILY_HISTOGRAM_BINS, AuditFamilyDistributionSource, MAX_AUDIT_QUERY_TIME_MS,
    normalize_audit_metric, project_audit_family_distribution,
};

pub const AUDIT_FAMILY_PROFILE_CANDIDATE_LIMIT: usize = 500;
const COUNT_COLUMNS: usize = 13;
const OCCURRENCE_COLUMNS: usize = 18;
const WIRE_COLUMNS: usize = 1 + COUNT_COLUMNS + OCCURRENCE_COLUMNS;

/// Server-owned binding from a returned family, never a client SQL fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilyTarget {
    pub native_digest: String,
    pub catalog: String,
    pub database: String,
}

impl AuditFamilyTarget {
    pub fn family_scope(&self, parent: &AuditScope) -> Result<AuditScope, AuditSqlBuildError> {
        parent.validate().map_err(AuditSqlBuildError::Request)?;
        if [
            (&parent.native_digest, &self.native_digest),
            (&parent.catalog, &self.catalog),
            (&parent.database, &self.database),
        ]
        .iter()
        .any(|(filter, target)| filter.as_ref().is_some_and(|value| value != *target))
        {
            return Err(AuditSqlBuildError::FamilyTarget);
        }
        let mut scope = parent.clone();
        scope.native_digest = Some(self.native_digest.clone());
        scope.catalog = Some(self.catalog.clone());
        scope.database = Some(self.database.clone());
        scope
            .validate()
            .map_err(|_| AuditSqlBuildError::FamilyTarget)?;
        Ok(scope)
    }

    fn predicate(&self) -> String {
        format!(
            "`catalog_name` = {} AND `database_name` = {} AND `digest` = {}",
            utf8_hex_literal(&self.catalog),
            utf8_hex_literal(&self.database),
            utf8_hex_literal(&self.native_digest)
        )
    }
}

fn validate_candidates(candidates: &[String]) -> Result<(), AuditSqlBuildError> {
    if candidates.len() > AUDIT_FAMILY_PROFILE_CANDIDATE_LIMIT {
        return Err(AuditSqlBuildError::FamilyCandidateBudget);
    }
    let mut unique = BTreeSet::new();
    for id in candidates {
        let canonical = crate::canonical_query_id(id).ok_or(AuditSqlBuildError::QueryId)?;
        if &canonical != id || !unique.insert(canonical) {
            return Err(AuditSqlBuildError::QueryId);
        }
    }
    Ok(())
}

fn family_events_cte(
    table: &AuditTableName,
    parent: &AuditScope,
    target: &AuditFamilyTarget,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    validate_occurrence_scope(parent, schema)?;
    target.family_scope(parent)?;
    // Deliberately do not push target into audit_window: cross-family duplicate
    // conflicts must be rejected by the parent-scope agreement first.
    Ok(format!(
        "{}, family_events AS (SELECT * FROM occurrence_events WHERE {})",
        canonical_occurrence_events_cte(table, parent, schema),
        target.predicate()
    ))
}

pub fn audit_family_detail_statement(
    table: &AuditTableName,
    parent: &AuditScope,
    target: &AuditFamilyTarget,
    candidates: &[String],
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    validate_candidates(candidates)?;
    let cte = family_events_cte(table, parent, target, schema)?;
    let eligible = format!("`query_time` >= 0 AND `query_time` <= {MAX_AUDIT_QUERY_TIME_MS}");
    let profile_candidates = if candidates.is_empty() {
        "FALSE".into()
    } else {
        format!(
            "LOWER(`query_id`) IN ({})",
            candidates
                .iter()
                .map(|id| utf8_hex_literal(id))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let count_if = |predicate: &str| format!("COUNT(CASE WHEN {predicate} THEN 1 END)");
    let mut counts = vec![
        "COUNT(*)".into(),
        count_if(&eligible),
        count_if("`query_time` IS NULL"),
        count_if("`query_time` < 0"),
        count_if(&format!("`query_time` > {MAX_AUDIT_QUERY_TIME_MS}")),
    ];
    counts.extend(AUDIT_FAMILY_HISTOGRAM_BINS.iter().map(|bin| {
        count_if(&format!(
            "`query_time` >= {} AND `query_time` < {}",
            bin.lower_ms, bin.upper_ms
        ))
    }));
    let null_counts = vec!["NULL"; COUNT_COLUMNS].join(", ");
    let null_occurrence = vec!["NULL"; OCCURRENCE_COLUMNS].join(", ");
    let statement = format!(
        "{cte}, recent AS (SELECT * FROM family_events ORDER BY `event_time` DESC, `query_id` ASC LIMIT 1), slowest AS (SELECT * FROM family_events WHERE {eligible} ORDER BY `query_time` DESC, `event_time` DESC, `query_id` ASC LIMIT 1), profile_known AS (SELECT * FROM family_events WHERE `state` = {finished} AND {profile_candidates} ORDER BY `event_time` DESC, `query_id` ASC LIMIT 1) SELECT 'summary' AS `row_kind`, {counts}, {null_occurrence} FROM family_events UNION ALL SELECT 'recent', {null_counts}, {OCCURRENCE_PROJECTION} FROM recent UNION ALL SELECT 'slowest', {null_counts}, {OCCURRENCE_PROJECTION} FROM slowest UNION ALL SELECT 'profile_known', {null_counts}, {OCCURRENCE_PROJECTION} FROM profile_known",
        finished = utf8_hex_literal("EOF"),
        counts = counts.join(", ")
    );
    if statement.len() > crate::MAX_PLAN_SQL_BYTES {
        return Err(AuditSqlBuildError::StatementBudget);
    }
    Ok(statement)
}

/// Explicit subsequent lookup retains the parent scope and re-proves the bound
/// family. stmt is selected only here, never in the detail statement.
pub fn audit_family_detail_sql_lookup_statement(
    table: &AuditTableName,
    parent: &AuditScope,
    target: &AuditFamilyTarget,
    query_id: &str,
    schema: &AuditSchema,
) -> Result<String, AuditSqlBuildError> {
    if !valid_query_id(query_id) {
        return Err(AuditSqlBuildError::QueryId);
    }
    schema.require(&[AuditColumn::Statement])?;
    let cte = family_events_cte(table, parent, target, schema)?;
    let query_id = utf8_hex_literal(&query_id.to_ascii_lowercase());
    Ok(format!(
        "{cte} SELECT `queryId` AS `query_id`, `timestamp` AS `event_time`, `stmt` AS `statement` FROM {table}{predicate} AND LOWER(`queryId`) = {query_id} AND EXISTS (SELECT 1 FROM family_events WHERE `query_id` = {query_id}) ORDER BY `timestamp` DESC LIMIT {AUDIT_SQL_LOOKUP_LIMIT}",
        table = table.quoted(),
        predicate = scope_predicate(parent)
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilyDetailSource {
    pub distribution: AuditFamilyExecutionDistribution,
    pub recent: Option<AuditOccurrenceSourceRow>,
    pub slowest: Option<AuditOccurrenceSourceRow>,
    pub profile_known: Option<AuditOccurrenceSourceRow>,
}

/// Reject malformed envelopes before any session can register a new generation.
/// Metrics still go through the sole core typed parser, including slowest eligibility.
pub fn decode_family_detail_source_rows(
    rows: Vec<Vec<Option<String>>>,
    parent: &AuditScope,
    target: &AuditFamilyTarget,
    candidates: &[String],
    schema: &AuditSchema,
) -> Result<AuditFamilyDetailSource, AuditSqlBuildError> {
    validate_candidates(candidates)?;
    target.family_scope(parent)?;
    if rows.is_empty() || rows.len() > 4 {
        return Err(AuditSqlBuildError::FamilyDetailResponse);
    }
    let bad = || AuditSqlBuildError::FamilyDetailResponse;
    let mut summary = None;
    let mut recent = None;
    let mut slowest = None;
    let mut profile_known = None;
    let mut unique = BTreeMap::new();
    for row in rows {
        if row.len() != WIRE_COLUMNS {
            return Err(row_shape(WIRE_COLUMNS, row.len()));
        }
        if row[0].as_deref() == Some("summary") {
            if summary.is_some() || row[14..].iter().any(Option::is_some) {
                return Err(bad());
            }
            let count = |index: usize| row[index].clone().ok_or_else(bad);
            let bucket_counts = (6..14)
                .map(count)
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| bad())?;
            summary = Some(AuditFamilyDistributionSource {
                population_count: count(1)?,
                eligible_count: count(2)?,
                sql_null_count: count(3)?,
                negative_count: count(4)?,
                overflow_count: count(5)?,
                bucket_counts,
            });
            continue;
        }
        if row[1..14].iter().any(Option::is_some) {
            return Err(bad());
        }
        let occurrence = decode_occurrence_source_row(row[14..].to_vec(), schema)?;
        let id = occurrence
            .query_id
            .as_deref()
            .and_then(crate::canonical_query_id)
            .ok_or_else(bad)?;
        let epoch = occurrence
            .event_epoch_seconds
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(bad)?;
        if !(parent.window.start_seconds..parent.window.end_seconds).contains(&epoch)
            || occurrence.event_time.as_ref().is_none_or(String::is_empty)
            || occurrence.catalog.as_ref() != Some(&target.catalog)
            || occurrence.database.as_ref() != Some(&target.database)
            || occurrence.native_digest.as_ref() != Some(&target.native_digest)
            || parent.state.as_ref().is_some_and(|state| {
                occurrence.state.as_deref()
                    != Some(match state {
                        AuditTerminalState::Finished => "EOF",
                        AuditTerminalState::Error => "ERR",
                    })
            })
            || parent
                .resource_group
                .as_ref()
                .is_some_and(|group| occurrence.resource_group.as_ref() != Some(group))
        {
            return Err(bad());
        }
        if unique
            .insert(id.clone(), occurrence.clone())
            .is_some_and(|prior| prior != occurrence)
        {
            return Err(bad());
        }
        let slot = match row[0].as_deref() {
            Some("recent") => &mut recent,
            Some("slowest") => {
                let input = &occurrence
                    .metrics
                    .iter()
                    .find(|(metric, _)| *metric == AuditMetricId::QueryTime)
                    .ok_or_else(bad)?
                    .1;
                let fact = normalize_audit_metric(AuditMetricId::QueryTime, input.clone());
                if !matches!(
                    fact.state,
                    AuditMetricState::Exact | AuditMetricState::PrecisionUnsafe
                ) {
                    return Err(bad());
                }
                &mut slowest
            }
            Some("profile_known") => {
                if occurrence.state.as_deref() != Some("EOF") || !candidates.contains(&id) {
                    return Err(bad());
                }
                &mut profile_known
            }
            _ => return Err(bad()),
        };
        if slot.replace(occurrence).is_some() {
            return Err(bad());
        }
    }
    let summary = summary.ok_or_else(bad)?;
    let distribution = project_audit_family_distribution(&summary).map_err(|_| bad())?;
    let empty = summary.population_count == "0";
    if unique.len() as u64 > summary.population_count.parse::<u64>().map_err(|_| bad())?
        || recent.is_none() != empty
        || (summary.eligible_count == "0") != slowest.is_none()
        || (empty && profile_known.is_some())
    {
        return Err(bad());
    }
    // A role is a member of this same exact population, not an unrelated row.
    // Count each UUID once even when it fills multiple roles. The unique parser
    // supplies state/ns; no raw metric grammar is reimplemented here.
    let mut represented = [0u64; 11];
    let mut times = BTreeMap::new();
    for (id, occurrence) in &unique {
        let (category, ns) = query_time_category(occurrence)?;
        represented[category] += 1;
        if let Some(ns) = ns {
            times.insert(id, ns);
        }
    }
    let available = [
        &summary.sql_null_count,
        &summary.negative_count,
        &summary.overflow_count,
    ]
    .into_iter()
    .chain(summary.bucket_counts.iter());
    for (represented, available) in represented.into_iter().zip(available) {
        if represented > available.parse::<u64>().map_err(|_| bad())? {
            return Err(bad());
        }
    }
    if let Some(slowest) = &slowest {
        let (category, ns) = query_time_category(slowest)?;
        if times.values().any(|other| Some(*other) > ns)
            || summary
                .bucket_counts
                .iter()
                .skip(category - 2)
                .any(|count| count != "0")
        {
            return Err(bad());
        }
    }
    Ok(AuditFamilyDetailSource {
        distribution,
        recent,
        slowest,
        profile_known,
    })
}

fn query_time_category(
    occurrence: &AuditOccurrenceSourceRow,
) -> Result<(usize, Option<i64>), AuditSqlBuildError> {
    let bad = || AuditSqlBuildError::FamilyDetailResponse;
    let input = &occurrence
        .metrics
        .iter()
        .find(|(metric, _)| *metric == AuditMetricId::QueryTime)
        .ok_or_else(bad)?
        .1;
    let fact = normalize_audit_metric(AuditMetricId::QueryTime, input.clone());
    match fact.state {
        AuditMetricState::Missing => Ok((0, None)),
        AuditMetricState::NegativeSentinel => Ok((1, None)),
        AuditMetricState::Overflow => Ok((2, None)),
        AuditMetricState::Exact | AuditMetricState::PrecisionUnsafe => {
            let ns = fact
                .canonical_number
                .ok_or_else(bad)?
                .value
                .parse::<i64>()
                .map_err(|_| bad())?;
            let ms = ns / 1_000_000;
            let bin = AUDIT_FAMILY_HISTOGRAM_BINS
                .iter()
                .position(|bin| ms >= bin.lower_ms && ms < bin.upper_ms)
                .ok_or_else(bad)?;
            Ok((3 + bin, Some(ns)))
        }
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent() -> AuditScope {
        AuditScope {
            window: AuditWindow {
                start_seconds: 1,
                end_seconds: 86401,
            },
            state: None,
            user: None,
            native_digest: None,
            catalog: None,
            database: None,
            resource_group: None,
            statement_filter: doctor_contracts::AuditStatementFilter::All,
        }
    }
    fn target() -> AuditFamilyTarget {
        AuditFamilyTarget {
            native_digest: "AbC123".into(),
            catalog: "default_catalog".into(),
            database: "db".into(),
        }
    }
    fn table() -> AuditTableName {
        AuditTableName::parse("audit.events").unwrap()
    }
    fn id() -> String {
        "abcdef01-0000-0000-0000-000000000001".into()
    }
    fn summary(population: &str, eligible: &str) -> Vec<Option<String>> {
        let mut row = vec![Some("summary".into())];
        row.extend(
            [
                population, eligible, "0", "0", "0", eligible, "0", "0", "0", "0", "0", "0", "0",
            ]
            .map(|s| Some(s.into())),
        );
        row.extend(vec![None; 18]);
        row
    }
    fn occurrence(role: &str, time: Option<&str>) -> Vec<Option<String>> {
        let mut row = vec![Some(role.into())];
        row.extend(vec![None; 13]);
        row.extend([
            Some(id()),
            Some("1970-01-01 00:00:20".into()),
            Some("20".into()),
            Some("EOF".into()),
            time.map(String::from),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some("0".into()),
            Some(target().catalog),
            Some(target().database),
            None,
            Some(target().native_digest),
            Some("".into()),
            Some("1".into()),
            Some("0".into()),
            Some("1".into()),
        ]);
        row
    }
    fn decode(
        rows: Vec<Vec<Option<String>>>,
    ) -> Result<AuditFamilyDetailSource, AuditSqlBuildError> {
        decode_family_detail_source_rows(
            rows,
            &parent(),
            &target(),
            &[id()],
            &AuditSchema::official_3_5_14(),
        )
    }

    #[test]
    fn family_detail_sql_keeps_parent_canonical_before_family_and_limits_only_roles() {
        let sql = audit_family_detail_statement(
            &table(),
            &parent(),
            &target(),
            &[id()],
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        let (parent_cte, family) = sql.split_once(", family_events AS ").unwrap();
        assert!(parent_cte.contains("SELECT LOWER(`queryId`) AS `query_id`"));
        assert!(parent_cte.contains("GROUP BY `query_id` HAVING"));
        assert!(parent_cte.contains("COUNT(DISTINCT `digest`) <= 1"));
        assert!(!parent_cte.contains(&utf8_hex_literal(&target().native_digest)));
        assert!(
            parent_cte
                .contains("`timestamp` >= FROM_UNIXTIME(1) AND `timestamp` < FROM_UNIXTIME(86401)")
        );
        assert!(!parent_cte.contains("LIMIT"));
        assert!(family.starts_with("(SELECT * FROM occurrence_events WHERE"));
        assert_eq!(sql.matches("LIMIT 1").count(), 3);
        assert_eq!(sql.matches("UNION ALL SELECT").count(), 3);
        assert!(sql.contains("COUNT(*)"));
        assert!(!sql.contains("`stmt`"));
        for bin in AUDIT_FAMILY_HISTOGRAM_BINS {
            assert!(sql.contains(&format!(
                "COUNT(CASE WHEN `query_time` >= {} AND `query_time` < {} THEN 1 END)",
                bin.lower_ms, bin.upper_ms
            )));
        }
        assert!(sql.contains("`query_time` DESC, `event_time` DESC, `query_id` ASC LIMIT 1"));
        assert!(sql.contains("LOWER(`query_id`) IN ("));
    }

    #[test]
    fn family_detail_candidate_target_schema_and_statement_budgets_are_closed() {
        let schema = AuditSchema::official_3_5_14();
        let candidates: Vec<_> = (0..500)
            .map(|n| format!("abcdef01-0000-0000-0000-{n:012x}"))
            .collect();
        let sql =
            audit_family_detail_statement(&table(), &parent(), &target(), &candidates, &schema)
                .unwrap();
        assert!(sql.len() < crate::MAX_PLAN_SQL_BYTES);
        let mut too_many = candidates;
        too_many.push(id());
        assert_eq!(
            audit_family_detail_statement(&table(), &parent(), &target(), &too_many, &schema),
            Err(AuditSqlBuildError::FamilyCandidateBudget)
        );
        for ids in [
            vec![id(), id()],
            vec![id().to_uppercase()],
            vec!["' OR 1=1".into()],
        ] {
            assert_eq!(
                audit_family_detail_statement(&table(), &parent(), &target(), &ids, &schema),
                Err(AuditSqlBuildError::QueryId)
            );
        }
        let mut scope = parent();
        scope.native_digest = Some("ffff".into());
        assert_eq!(
            audit_family_detail_statement(&table(), &scope, &target(), &[], &schema),
            Err(AuditSqlBuildError::FamilyTarget)
        );
        let mut bad_target = target();
        bad_target.catalog = "bad\0catalog".into();
        assert_eq!(
            audit_family_detail_statement(&table(), &parent(), &bad_target, &[], &schema),
            Err(AuditSqlBuildError::FamilyTarget)
        );
        for source_type in ["DECIMAL(38,0)", "VARCHAR", "UNKNOWN"] {
            let mut bad_schema = schema.clone();
            bad_schema
                .source_types
                .insert(AuditColumn::QueryTime, source_type.into());
            assert_eq!(
                audit_family_detail_statement(&table(), &parent(), &target(), &[], &bad_schema),
                Err(AuditSqlBuildError::SchemaTypeUnsupported)
            );
        }
    }

    #[test]
    fn family_detail_lookup_reproves_parent_conflicts_and_keeps_ambiguity_limit() {
        let sql = audit_family_detail_sql_lookup_statement(
            &table(),
            &parent(),
            &target(),
            &id().to_uppercase(),
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        assert!(sql.contains("SELECT LOWER(`queryId`) AS `query_id`"));
        assert!(sql.contains("AND LOWER(`queryId`) ="));
        assert!(sql.contains("EXISTS (SELECT 1 FROM family_events WHERE `query_id` ="));
        assert!(sql.contains(&utf8_hex_literal(&id())));
        assert!(sql.ends_with("ORDER BY `timestamp` DESC LIMIT 2"));
        assert_eq!(sql.matches("`timestamp` >= FROM_UNIXTIME(1)").count(), 2);
        assert!(!sql.contains("MAX(`stmt`)"));
    }

    #[test]
    fn family_detail_decodes_full_population_and_same_occurrence_can_fill_three_roles() {
        let source = decode(vec![
            occurrence("slowest", Some("9")),
            summary("501", "501"),
            occurrence("profile_known", Some("9")),
            occurrence("recent", Some("9")),
        ])
        .unwrap();
        assert_eq!(source.recent, source.slowest);
        assert_eq!(source.recent, source.profile_known);
        let AuditFamilyExecutionDistribution::Exact {
            population_count, ..
        } = source.distribution;
        assert_eq!(population_count, "501");
    }

    #[test]
    fn family_detail_empty_and_unavailable_metrics_do_not_become_zero() {
        let source = decode(vec![summary("0", "0")]).unwrap();
        assert!(
            source.recent.is_none() && source.slowest.is_none() && source.profile_known.is_none()
        );
        for (time, excluded) in [(None, 3), (Some("-1"), 4), (Some("9223372036855"), 5)] {
            let mut counts = summary("1", "0");
            counts[excluded] = Some("1".into());
            let source = decode(vec![counts, occurrence("recent", time)]).unwrap();
            assert!(source.slowest.is_none());
            assert!(source.recent.is_some());
        }
    }

    #[test]
    fn family_detail_rejects_missing_duplicate_or_malformed_summary_and_role_shapes() {
        for rows in [
            vec![],
            vec![occurrence("recent", Some("9"))],
            vec![summary("0", "0"), summary("0", "0")],
            vec![summary("1", "1")],
            vec![summary("0", "0"), occurrence("recent", Some("9"))],
            vec![summary("1", "1"), occurrence("recent", Some("9"))],
            vec![
                summary("1", "1"),
                occurrence("recent", Some("9")),
                occurrence("slowest", Some("8")),
            ],
        ] {
            assert!(decode(rows).is_err());
        }
        for column in 1..14 {
            let mut row = summary("0", "0");
            row[column] = Some("00".into());
            assert!(decode(vec![row]).is_err());
        }
        let mut row = summary("0", "0");
        row[14] = Some(id());
        assert!(decode(vec![row]).is_err());
        let mut row = summary("0", "0");
        row.pop();
        assert!(decode(vec![row]).is_err());
    }

    #[test]
    fn family_detail_rejects_wrong_window_family_or_unproven_profile() {
        for (column, value) in [
            (16, "86401"),
            (24, "other_catalog"),
            (25, "other_db"),
            (27, "abcd"),
            (14, "not-uuid"),
        ] {
            let mut row = occurrence("recent", Some("9"));
            row[column] = Some(value.into());
            assert!(
                decode(vec![
                    summary("1", "1"),
                    row,
                    occurrence("slowest", Some("9"))
                ])
                .is_err()
            );
        }
        let mut wrong_state = occurrence("profile_known", Some("9"));
        wrong_state[17] = Some("ERR".into());
        assert!(
            decode(vec![
                summary("1", "1"),
                occurrence("recent", Some("9")),
                occurrence("slowest", Some("9")),
                wrong_state
            ])
            .is_err()
        );
        assert!(
            decode_family_detail_source_rows(
                vec![
                    summary("1", "1"),
                    occurrence("recent", Some("9")),
                    occurrence("slowest", Some("9")),
                    occurrence("profile_known", Some("9"))
                ],
                &parent(),
                &target(),
                &[],
                &AuditSchema::official_3_5_14()
            )
            .is_err()
        );
    }

    #[test]
    fn family_detail_fixed_3514_constant_oracle_is_decoded_by_the_real_pipeline() {
        // Captured from the unchanged builder embedded in constant-only CTEs.
        // This proves SQL/decoder agreement, not a live Audit/API/Candidate run.
        let tsv = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/synthetic/audit-family-detail-sql-3514.tsv"
        ));
        let rows = tsv
            .lines()
            .map(|line| {
                line.split('\t')
                    .map(|cell| {
                        if cell == "NULL" {
                            None
                        } else {
                            Some(cell.to_owned())
                        }
                    })
                    .collect()
            })
            .collect();
        let candidates = vec!["abcdef01-0000-0000-0000-000000000007".into()];
        let source = decode_family_detail_source_rows(
            rows,
            &parent(),
            &target(),
            &candidates,
            &AuditSchema::official_3_5_14(),
        )
        .unwrap();
        let AuditFamilyExecutionDistribution::Exact {
            population_count,
            eligible_count,
            exclusions,
            buckets,
            ..
        } = source.distribution;
        assert_eq!(population_count, "518");
        assert_eq!(eligible_count, "515");
        assert_eq!(
            (
                exclusions.sql_null_count.as_str(),
                exclusions.negative_count.as_str(),
                exclusions.overflow_count.as_str()
            ),
            ("1", "1", "1")
        );
        assert_eq!(
            buckets.iter().map(|b| b.count.as_str()).collect::<Vec<_>>(),
            ["501", "2", "2", "2", "2", "2", "2", "2"]
        );
        assert_eq!(
            source.recent.unwrap().query_id.as_deref(),
            Some("abcdef01-0000-0000-0000-000000000001")
        );
        assert_eq!(
            source.slowest.unwrap().query_id.as_deref(),
            Some("abcdef01-0000-0000-0000-000000000207")
        );
        assert_eq!(
            source.profile_known.unwrap().query_id,
            Some(candidates[0].clone())
        );
    }

    #[test]
    fn family_detail_representatives_must_fit_population_bins_and_slowest_bin() {
        assert!(
            decode(vec![
                summary("1", "1"),
                occurrence("recent", Some("60000")),
                occurrence("slowest", Some("60000"))
            ])
            .is_err()
        );
        let mut counts = summary("2", "2");
        counts[6] = Some("1".into());
        counts[7] = Some("1".into());
        // Even when the returned 9ms row fits a nonzero bucket, it is not the
        // slowest if another, strictly higher bucket has a population member.
        assert!(
            decode(vec![
                counts,
                occurrence("recent", Some("9")),
                occurrence("slowest", Some("9"))
            ])
            .is_err()
        );
        let mut counts = summary("2", "1");
        counts[3] = Some("1".into());
        let mut profile = occurrence("profile_known", Some("9"));
        profile[14] = Some("abcdef01-0000-0000-0000-000000000002".into());
        assert!(
            decode_family_detail_source_rows(
                vec![
                    counts,
                    occurrence("recent", Some("9")),
                    occurrence("slowest", Some("9")),
                    profile
                ],
                &parent(),
                &target(),
                &[id(), "abcdef01-0000-0000-0000-000000000002".into()],
                &AuditSchema::official_3_5_14()
            )
            .is_err()
        );
    }

    #[test]
    fn family_detail_checks_observable_parent_filters_in_response() {
        for filtered in [
            AuditScope {
                state: Some(AuditTerminalState::Error),
                ..parent()
            },
            AuditScope {
                resource_group: Some("selected-group".into()),
                ..parent()
            },
        ] {
            assert!(
                decode_family_detail_source_rows(
                    vec![
                        summary("1", "1"),
                        occurrence("recent", Some("9")),
                        occurrence("slowest", Some("9"))
                    ],
                    &filtered,
                    &target(),
                    &[id()],
                    &AuditSchema::official_3_5_14()
                )
                .is_err()
            );
        }
        let filtered = AuditScope {
            state: Some(AuditTerminalState::Error),
            resource_group: Some("selected-group".into()),
            ..parent()
        };
        let mut recent = occurrence("recent", Some("9"));
        recent[17] = Some("ERR".into());
        recent[26] = Some("selected-group".into());
        let mut slowest = recent.clone();
        slowest[0] = Some("slowest".into());
        assert!(
            decode_family_detail_source_rows(
                vec![summary("1", "1"), recent, slowest],
                &filtered,
                &target(),
                &[],
                &AuditSchema::official_3_5_14()
            )
            .is_ok()
        );
    }
}
