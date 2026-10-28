//! Offline current-builder export. stdin: start,end,limit,sort,family-sort then
//! state,user,catalog,database,resource_group,native_digest (UTF-8 hex; '-' = null),
//! then the statement filter (`all` | `exclude_doctor_and_non_query`).
//! stdout: role<TAB>exact wire SQL (with the tool marker) UTF-8 hex. Never
//! execute or persist this output.
use doctor_connector::{
    AUDIT_OVERVIEW_FAMILY_TOP_LIMIT, AuditColumn, AuditSchema, AuditTableName,
    audit_concurrency_statement, audit_coverage_statement, audit_family_statement,
    audit_occurrence_statement, doctor_statement,
};
use doctor_contracts::{
    AuditFamilyRequest, AuditFamilySort, AuditOccurrenceRequest, AuditOccurrenceSort, AuditScope,
    AuditStatementFilter, AuditTerminalState, AuditWindow,
};
use std::io::{self, Read};
fn value(s: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    if s == "-" {
        return Ok(None);
    }
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err("invalid hex".into());
    }
    let bytes = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(String::from_utf8(bytes)?))
}
fn build(input: &str) -> Result<[(&'static str, String); 4], Box<dyn std::error::Error>> {
    if input.len() > 16384 {
        return Err("input cap".into());
    }
    let a: Vec<_> = input.lines().collect();
    if a.len() != 12 {
        return Err("input shape".into());
    }
    let state = match value(a[5])?.as_deref() {
        None => None,
        Some("finished") => Some(AuditTerminalState::Finished),
        Some("error") => Some(AuditTerminalState::Error),
        _ => return Err("state".into()),
    };
    let scope = AuditScope {
        window: AuditWindow {
            start_seconds: a[0].parse()?,
            end_seconds: a[1].parse()?,
        },
        state,
        user: value(a[6])?,
        catalog: value(a[7])?,
        database: value(a[8])?,
        resource_group: value(a[9])?,
        native_digest: value(a[10])?,
        statement_filter: match a[11] {
            "all" => AuditStatementFilter::All,
            "exclude_doctor_and_non_query" => AuditStatementFilter::ExcludeDoctorAndNonQuery,
            _ => return Err("statement filter".into()),
        },
    };
    scope.validate()?;
    let sort = match a[3] {
        "query_time_desc" => AuditOccurrenceSort::QueryTimeDesc,
        "scan_bytes_desc" => AuditOccurrenceSort::ScanBytesDesc,
        "scan_rows_desc" => AuditOccurrenceSort::ScanRowsDesc,
        "cpu_cost_desc" => AuditOccurrenceSort::CpuCostDesc,
        "memory_cost_desc" => AuditOccurrenceSort::MemoryCostDesc,
        "newest_first" => AuditOccurrenceSort::NewestFirst,
        _ => return Err("sort".into()),
    };
    let family_sort = match a[4] {
        "cumulative_query_time_desc" => AuditFamilySort::CumulativeQueryTimeDesc,
        "cumulative_scan_bytes_desc" => AuditFamilySort::CumulativeScanBytesDesc,
        "cumulative_cpu_cost_desc" => AuditFamilySort::CumulativeCpuCostDesc,
        "execution_count_desc" => AuditFamilySort::ExecutionCountDesc,
        _ => return Err("family sort".into()),
    };
    // Preserve official source types, and add the optional source-user column.
    let mut rows: Vec<Vec<Option<String>>> = [
        "queryId",
        "timestamp",
        "state",
        "queryTime",
        "scanBytes",
        "scanRows",
        "cpuCostNs",
        "memCostBytes",
        "returnRows",
        "catalog",
        "db",
        "resourceGroup",
        "digest",
        "stmt",
        "errorCode",
        "pendingTimeMs",
        "user",
        "isQuery",
    ]
    .iter()
    .map(|name| {
        vec![
            Some((*name).into()),
            Some(
                if *name == "timestamp" {
                    "DATETIME"
                } else if *name == "isQuery" {
                    "TINYINT"
                } else if [
                    "queryTime",
                    "scanBytes",
                    "scanRows",
                    "cpuCostNs",
                    "memCostBytes",
                    "returnRows",
                    "pendingTimeMs",
                ]
                .contains(name)
                {
                    "BIGINT"
                } else {
                    "VARCHAR"
                }
                .into(),
            ),
        ]
    })
    .collect();
    let schema = AuditSchema::from_schema_probe_rows(&rows)?;
    rows.clear();
    assert!(schema.has(AuditColumn::User));
    let table = AuditTableName::parse("starrocks_audit_db__.starrocks_audit_tbl__")?;
    let statements = [
        (
            "coverage",
            audit_coverage_statement(&table, &scope, &schema)?,
        ),
        (
            "primary",
            audit_occurrence_statement(
                &table,
                &AuditOccurrenceRequest {
                    scope: scope.clone(),
                    sort,
                    limit: a[2].parse()?,
                },
                &schema,
            )?,
        ),
        (
            "concurrency",
            audit_concurrency_statement(&table, &scope, &schema)?,
        ),
        (
            "family_top",
            audit_family_statement(
                &table,
                &AuditFamilyRequest {
                    scope,
                    sort: family_sort,
                    limit: AUDIT_OVERVIEW_FAMILY_TOP_LIMIT,
                },
                &schema,
            )?,
        ),
    ]
    .map(|(role, sql)| (role, doctor_statement(&sql)));
    Ok(statements)
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().take(16385).read_to_string(&mut input)?;
    for (role, sql) in build(&input)? {
        let hex: String = sql.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        println!("{role}\t{hex}");
    }
    Ok(())
}
fn main() {
    if run().is_err() {
        eprintln!("audit_resource_builder_failed");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_roles_preserve_product_windows_and_limits() {
        let result=build("408000\n3000000\n17\nscan_rows_desc\nexecution_count_desc\n-\n776f726b6c6f61645f75736572\n-\n-\n-\n-\nexclude_doctor_and_non_query\n").unwrap();
        assert_eq!(
            result.each_ref().map(|(role, _)| *role),
            ["coverage", "primary", "concurrency", "family_top"]
        );
        for (role, sql) in &result {
            let start = if *role == "concurrency" {
                404400
            } else {
                408000
            };
            assert!(sql.contains(&format!("WHERE `timestamp` >= FROM_UNIXTIME({start}) AND `timestamp` < FROM_UNIXTIME(3000000)")),"{role}");
            assert!(
                sql.contains("AND `user` = CAST(UNHEX('776F726B6C6F61645F75736572') AS VARCHAR)")
            );
            assert!(sql.starts_with("/* sr-doctor */ WITH "), "{role}");
            assert!(sql.contains("AND `isQuery` = 1 AND (`stmt` IS NULL OR (`stmt` NOT LIKE '/* sr-doctor */ %' AND `stmt` NOT LIKE '/* starrocks-doctor */ %'))"), "{role}");
        }
        assert!(result[1].1.ends_with("LIMIT 17"));
        assert!(
            result[3]
                .1
                .ends_with(&format!("LIMIT {AUDIT_OVERVIEW_FAMILY_TOP_LIMIT}"))
        );
        assert!(build("408000\n3000000\n0\nquery_time_desc\ncumulative_query_time_desc\n-\n-\n-\n-\n-\n-\nall\n").is_err());
        assert!(build("408000\n3000000\n1\nquery_time_desc\ncumulative_query_time_desc\n-\n-\n-\n-\n-\n-\n").is_err());
        assert!(build("408000\n3000000\n1\nquery_time_desc\ncumulative_query_time_desc\n-\n-\n-\n-\n-\n-\nhide\n").is_err());
        let all = build("408000\n3000000\n1\nquery_time_desc\ncumulative_query_time_desc\n-\n-\n-\n-\n-\n-\nall\n").unwrap();
        assert!(all.iter().all(|(_, sql)| !sql.contains("`isQuery`")));
    }
}
