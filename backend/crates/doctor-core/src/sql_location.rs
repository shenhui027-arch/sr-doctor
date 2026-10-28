//! A deliberately small lexical locator, not a SQL parser or rewrite engine.
use doctor_contracts::{
    AnalysisSnapshotReport, GraphNode, GraphSqlFactKind as Kind, GraphSqlLocation as Location,
};

#[derive(Clone, Debug)]
struct Token {
    value: String,
    start: usize,
    end: usize,
    identifier: bool,
}

fn lex(sql: &str) -> Option<Vec<Token>> {
    if sql.len() > 65536
        || sql.contains('\\')
        || sql.contains('"')
        || sql.contains("...")
        || sql.contains('…')
        || sql
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return None;
    }
    let b = sql.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"--") || b[i] == b'#' {
            if b[i] == b'-' && b.get(i + 2).is_some_and(|c| !c.is_ascii_whitespace()) {
                return None;
            }
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"/*") {
            if matches!(b.get(i + 2), Some(b'!' | b'+')) {
                return None;
            }
            i += 2;
            while i + 1 < b.len() && &b[i..i + 2] != b"*/" {
                i += 1;
            }
            if i + 1 >= b.len() {
                return None;
            }
            i += 2;
            continue;
        }
        let start = i;
        let identifier;
        if matches!(b[i], b'\'' | b'`') {
            let quote = b[i];
            identifier = quote == b'`';
            i += 1;
            loop {
                if i >= b.len() {
                    return None;
                }
                if b[i] == quote {
                    i += 1;
                    if b.get(i) == Some(&quote) {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] >= 128 {
            identifier = true;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128) {
                i += 1;
            }
        } else {
            identifier = false;
            i += 1;
        }
        // Preserve identifier case: session identifier folding is not inferred.
        out.push(Token {
            value: sql[start..i].to_owned(),
            start,
            end: i,
            identifier,
        });
        if out.len() > 8192 {
            return None;
        }
    }
    Some(out)
}
fn keyword(token: &Token, word: &str) -> bool {
    token.identifier && token.value.eq_ignore_ascii_case(word)
}
fn name(token: &Token) -> &str {
    token
        .value
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .unwrap_or(&token.value)
}
fn same(a: &[Token], b: &[Token]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.value == b.value)
}

struct Shape {
    tokens: Vec<Token>,
    tables: Vec<(usize, usize)>,
    predicates: Vec<(Kind, usize, usize)>,
}
fn shape(sql: &str) -> Option<Shape> {
    let mut t = lex(sql)?;
    if t.last().is_some_and(|t| t.value == ";") {
        t.pop();
    }
    if !t.first().is_some_and(|t| keyword(t, "SELECT")) {
        return None;
    }
    // Scope: single-level grammar only; add scoped parsing only for a proven need.
    if t.iter().skip(1).any(|t| {
        ["SELECT", "WITH", "UNION", "INTERSECT", "EXCEPT"]
            .iter()
            .any(|k| keyword(t, k))
            || t.value == ";"
    }) {
        return None;
    }
    let mut depth = 0i32;
    for token in &t {
        if token.value == "(" {
            depth += 1;
        }
        if token.value == ")" {
            depth -= 1;
        }
        if depth < 0 {
            return None;
        }
    }
    if depth != 0 {
        return None;
    }
    let mut tables = Vec::new();
    let mut predicates = Vec::new();
    let mut depth = 0i32;
    let mut table_scope = false;
    for i in 0..t.len() {
        if t[i].value == "(" {
            depth += 1;
            continue;
        }
        if t[i].value == ")" {
            depth -= 1;
            continue;
        }
        if depth != 0 {
            continue;
        }
        if ["WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "QUALIFY"]
            .iter()
            .any(|k| keyword(&t[i], k))
        {
            table_scope = false;
        }
        if table_scope
            && (t[i].value == "," || keyword(&t[i], "USING") || keyword(&t[i], "NATURAL"))
        {
            return None;
        }
        if keyword(&t[i], "FROM") || keyword(&t[i], "JOIN") {
            table_scope = true;
            let start = i + 1;
            let mut end = start;
            if !t.get(start).is_some_and(|t| t.identifier) {
                return None;
            }
            end += 1;
            while t.get(end).is_some_and(|t| t.value == ".") {
                if !t.get(end + 1).is_some_and(|t| t.identifier) {
                    return None;
                }
                end += 2;
            }
            tables.push((start, end));
        }
        if keyword(&t[i], "WHERE") || keyword(&t[i], "ON") {
            let kind = if keyword(&t[i], "WHERE") {
                Kind::Predicate
            } else {
                Kind::JoinCondition
            };
            let mut end = i + 1;
            let mut depth = 0i32;
            while end < t.len() {
                if depth == 0
                    && [
                        "WHERE", "JOIN", "LEFT", "RIGHT", "FULL", "INNER", "CROSS", "GROUP",
                        "ORDER", "HAVING", "LIMIT", "QUALIFY",
                    ]
                    .iter()
                    .any(|k| keyword(&t[end], k))
                {
                    break;
                }
                if t[end].value == "(" {
                    depth += 1;
                }
                if t[end].value == ")" {
                    depth -= 1;
                }
                end += 1;
            }
            if end == i + 1 {
                return None;
            }
            predicates.push((kind, i + 1, end));
        }
    }
    if tables.is_empty() {
        return None;
    }
    Some(Shape {
        tokens: t,
        tables,
        predicates,
    })
}

fn excerpt(sql: &str, start: usize, end: usize, kind: Kind, source_refs: Vec<String>) -> Location {
    let highlight = &sql[start..end];
    if highlight.chars().count() > 512 {
        return Location::Unsupported;
    }
    // Context remains inside the expression/reference: no unrelated literals are disclosed.
    Location::Located {
        kind,
        before: String::new(),
        highlight: highlight.to_owned(),
        after: String::new(),
        source_refs,
    }
}

fn locate(node: &GraphNode, sql: &str, parsed: &Shape, duplicate_node: bool) -> Location {
    if duplicate_node {
        return Location::Ambiguous;
    }
    let facts = node.sql_facts.as_deref().unwrap_or(&[]);
    // Even distinct aliases cannot distinguish two instances of the same base table here.
    for (index, &(s, e)) in parsed.tables.iter().enumerate() {
        if parsed.tables[..index]
            .iter()
            .any(|&(_, other)| name(&parsed.tokens[other - 1]) == name(&parsed.tokens[e - 1]))
        {
            return Location::Ambiguous;
        }
        let _ = s;
    }
    for kind in [Kind::Predicate, Kind::JoinCondition, Kind::Table] {
        let matching = facts
            .iter()
            .filter(|fact| fact.kind == kind)
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Location::Ambiguous;
        }
        let Some(fact) = matching.first() else {
            continue;
        };
        if fact.source_refs.is_empty() {
            continue;
        }
        let Some(ft) = lex(&fact.value) else { continue };
        let candidates: Vec<_> = if kind == Kind::Table {
            parsed
                .tables
                .iter()
                .copied()
                .filter(|&(s, e)| {
                    if ft.len() == 1 && ft[0].identifier {
                        name(&ft[0]) == name(&parsed.tokens[e - 1])
                    } else {
                        same(&ft, &parsed.tokens[s..e])
                    }
                })
                .collect()
        } else {
            parsed
                .predicates
                .iter()
                .filter(|(k, s, e)| *k == kind && same(&ft, &parsed.tokens[*s..*e]))
                .map(|(_, s, e)| (*s, *e))
                .collect()
        };
        if candidates.len() > 1 {
            return Location::Ambiguous;
        }
        if let Some(&(s, e)) = candidates.first() {
            return excerpt(
                sql,
                parsed.tokens[s].start,
                parsed.tokens[e - 1].end,
                kind,
                fact.source_refs.clone(),
            );
        }
    }
    Location::Unavailable
}

/// Only call with the SQL retained by this very execution request, never a UI draft.
pub fn attach_sql_locations(
    report: &mut AnalysisSnapshotReport,
    sql: &str,
    expected_query_id: &str,
) {
    let bound = report
        .query_summary
        .as_ref()
        .and_then(|q| q.query_id.as_deref())
        == Some(expected_query_id)
        && report.version.selected_pack.as_ref().is_some_and(|p| {
            p.upstream_release == "3.5.14"
                && p.upstream_commit == "23a56ec517db763ea9bc7ee50b19357acdee0303"
        });
    let parsed = bound.then(|| shape(sql)).flatten();
    let mut table_nodes =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for node in &report.graph_view.nodes {
        for fact in node
            .sql_facts
            .iter()
            .flatten()
            .filter(|f| f.kind == Kind::Table)
        {
            table_nodes
                .entry(fact.value.clone())
                .or_default()
                .insert(node.node_id.clone());
        }
    }
    let ids = report
        .graph_view
        .nodes
        .iter()
        .map(|n| n.node_id.clone())
        .collect::<Vec<_>>();
    for node in &mut report.graph_view.nodes {
        node.sql_location = Some(if !bound {
            Location::Unavailable
        } else if let Some(parsed) = &parsed {
            locate(
                node,
                sql,
                parsed,
                ids.iter().filter(|id| **id == node.node_id).count() != 1,
            )
        } else {
            Location::Unsupported
        });
        if matches!(
            node.sql_location,
            Some(Location::Located {
                kind: Kind::Table,
                ..
            })
        ) && node
            .sql_facts
            .iter()
            .flatten()
            .filter(|f| f.kind == Kind::Table)
            .any(|f| table_nodes.get(&f.value).is_some_and(|ids| ids.len() > 1))
        {
            node.sql_location = Some(Location::Ambiguous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::GraphSqlFact;
    fn report() -> AnalysisSnapshotReport {
        serde_json::from_str(include_str!(
            "../../../../fixtures/expected/3.5.14/sr3514_execution_graph.report.json"
        ))
        .unwrap()
    }
    fn node(kind: Kind, value: &str) -> GraphNode {
        let mut node = report().graph_view.nodes.remove(0);
        node.sql_facts = Some(vec![GraphSqlFact {
            kind,
            value: value.into(),
            source_refs: vec!["fixture:explicit".into()],
        }]);
        node
    }
    fn found(sql: &str, kind: Kind, value: &str) -> Location {
        match shape(sql) {
            Some(s) => locate(&node(kind, value), sql, &s, false),
            None => Location::Unsupported,
        }
    }
    #[test]
    fn sql_location_unique_table_unicode_crlf_comments_and_literals() {
        let sql = "SELECT '😀 FROM sales' AS note\r\nFROM `销售` -- JOIN sales\r\nWHERE id = 1";
        assert!(
            matches!(found(sql,Kind::Table,"销售"),Location::Located{highlight,..} if highlight=="`销售`")
        );
        assert!(matches!(
            found(
                "SELECT 'FROM fake' FROM sales /* JOIN fake */",
                Kind::Table,
                "fake"
            ),
            Location::Unavailable
        ));
        assert!(
            matches!(found("SELECT * FROM warehouse.sales s JOIN products p ON s.id = p.id",Kind::Table,"sales"),Location::Located{highlight,..} if highlight=="warehouse.sales")
        );
    }
    #[test]
    fn sql_location_complete_expression_only_no_slot_or_alias_guess() {
        let sql =
            "SELECT * FROM sales s JOIN products p ON s.id = p.id WHERE s.id > 1 AND p.id < 9";
        assert!(matches!(
            found(sql, Kind::JoinCondition, "s.id = p.id"),
            Location::Located {
                kind: Kind::JoinCondition,
                ..
            }
        ));
        assert!(matches!(
            found(sql, Kind::Predicate, "s.id > 1"),
            Location::Unavailable
        ));
        assert!(matches!(
            found(sql, Kind::JoinCondition, "1: id = 2: id"),
            Location::Unavailable
        ));
        assert!(matches!(
            found(sql, Kind::JoinCondition, "sales.id = products.id"),
            Location::Unavailable
        ));
    }

    #[test]
    fn sql_location_aggregates_keep_only_top_level_table_scope() {
        let sql = "SELECT SUM(p.id * w.factor), EXTRACT(YEAR FROM p.ts) FROM products p JOIN weights w ON p.id = w.id GROUP BY p.id, w.id ORDER BY p.id, w.id";
        assert!(
            matches!(found(sql, Kind::Table, "products"), Location::Located { highlight, .. } if highlight == "products")
        );
        assert!(
            matches!(found(sql, Kind::Table, "weights"), Location::Located { highlight, .. } if highlight == "weights")
        );
        assert!(matches!(
            found(sql, Kind::Table, "p.ts"),
            Location::Unavailable
        ));
    }
    #[test]
    fn sql_location_self_join_and_repeated_on_are_ambiguous() {
        assert_eq!(
            found(
                "SELECT * FROM sales a JOIN sales b ON a.id = b.id",
                Kind::Table,
                "sales"
            ),
            Location::Ambiguous
        );
        assert_eq!(
            found(
                "SELECT * FROM a JOIN b ON a.id = b.id JOIN c ON a.id = b.id",
                Kind::JoinCondition,
                "a.id = b.id"
            ),
            Location::Ambiguous
        );
    }
    #[test]
    fn sql_location_unsupported_and_truncated_sources() {
        for sql in [
            "WITH c AS (SELECT * FROM sales) SELECT * FROM c",
            "SELECT * FROM (SELECT * FROM sales) x",
            "SELECT * FROM sales UNION SELECT * FROM sales",
            "SELECT * FROM sales...",
            "SELECT * FROM sales…",
            "SELECT * FROM sales /*",
            "SELECT 'oops FROM sales",
            "SELECT * FROM \"sales\"",
            "SELECT 'a\\b' FROM sales",
            "SELECT * FROM sales\0",
            "SELECT * FROM sales; SELECT 1",
            "SELECT * FROM sales, products",
            "SELECT * FROM sales /*! hint */",
        ] {
            assert_eq!(
                found(sql, Kind::Table, "sales"),
                Location::Unsupported,
                "{sql}"
            );
        }
        assert!(shape(&format!("SELECT {} FROM sales", "x".repeat(65536))).is_none());
    }
    #[test]
    fn sql_location_bound_report_clears_stale_data() {
        let mut r = report();
        let query = r.query_summary.as_ref().unwrap().query_id.clone().unwrap();
        r.graph_view.nodes[0].sql_facts = node(Kind::Table, "sales").sql_facts;
        attach_sql_locations(&mut r, "SELECT * FROM sales", &query);
        assert!(matches!(
            r.graph_view.nodes[0].sql_location,
            Some(Location::Located { .. })
        ));
        attach_sql_locations(&mut r, "SELECT * FROM sales", "wrong-query-id");
        assert!(
            r.graph_view
                .nodes
                .iter()
                .all(|n| n.sql_location == Some(Location::Unavailable))
        );
        r.version.selected_pack = None;
        attach_sql_locations(&mut r, "SELECT * FROM sales", &query);
        assert!(
            r.graph_view
                .nodes
                .iter()
                .all(|n| n.sql_location == Some(Location::Unavailable))
        );
    }
    #[test]
    fn sql_location_duplicate_node_identity_never_locates() {
        let mut r = report();
        let query = r.query_summary.as_ref().unwrap().query_id.clone().unwrap();
        r.graph_view.nodes[0].sql_facts = node(Kind::Table, "sales").sql_facts;
        r.graph_view.nodes.push(r.graph_view.nodes[0].clone());
        attach_sql_locations(&mut r, "SELECT * FROM sales", &query);
        assert_eq!(
            r.graph_view.nodes[0].sql_location,
            Some(Location::Ambiguous)
        );
    }

    #[test]
    fn sql_location_distinct_plan_nodes_with_same_table_do_not_guess_view_lineage() {
        let mut r = report();
        let query = r.query_summary.as_ref().unwrap().query_id.clone().unwrap();
        r.graph_view.nodes[0].sql_facts = node(Kind::Table, "sales").sql_facts;
        let mut second = r.graph_view.nodes[0].clone();
        second.node_id = "another-plan-node".into();
        r.graph_view.nodes.push(second);
        attach_sql_locations(&mut r, "SELECT * FROM sales", &query);
        assert_eq!(
            r.graph_view.nodes[0].sql_location,
            Some(Location::Ambiguous)
        );
        assert_eq!(
            r.graph_view.nodes.last().unwrap().sql_location,
            Some(Location::Ambiguous)
        );
    }
}
