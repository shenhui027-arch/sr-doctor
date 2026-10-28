//! A2 structure extraction: the table set, join shape and predicate column
//! set of one plain SELECT, read from the A1 normalized token stream. Product
//! decision (no official counterpart): a deliberately small grammar —
//! `SELECT … FROM t [alias] {[kind] JOIN t [alias] [ON … | USING (…)] | , t}
//! [WHERE …] [GROUP BY …] [HAVING …] [ORDER BY …] [LIMIT …]` — and anything
//! else (CTE, set operation, any subquery, derived table, table function) is
//! unavailable rather than guessed.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{SimilarSqlBasis, SimilarSqlUnavailableReason as Unavailable};

use crate::sql_fingerprint::{Out, normalized_tokens};

fn kw(token: Option<&Out>, expected: &str) -> bool {
    matches!(token, Some(Out::Kw(kw)) if kw == expected)
}

fn ident(token: Option<&Out>) -> Option<&str> {
    match token {
        Some(Out::Ident(name)) => Some(name),
        _ => None,
    }
}

/// Index of the first depth-0 token in `from..` satisfying `stop`, else the end.
fn clause_end(out: &[Out], from: usize, stop: impl Fn(&[Out], usize) -> bool) -> usize {
    let mut depth = 0usize;
    for index in from..out.len() {
        if depth == 0 && stop(out, index) {
            return index;
        }
        match &out[index] {
            Out::Op("(" | "[") => depth += 1,
            Out::Op(")" | "]") => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    out.len()
}

fn ends_from(out: &[Out], index: usize) -> bool {
    matches!(&out[index], Out::Kw(kw) if matches!(kw.as_str(), "WHERE" | "GROUP" | "HAVING" | "ORDER" | "LIMIT"))
}

fn ends_where(out: &[Out], index: usize) -> bool {
    matches!(&out[index], Out::Kw(kw) if matches!(kw.as_str(), "GROUP" | "HAVING" | "ORDER" | "LIMIT"))
}

struct TableRef {
    name: String,
    alias: Option<String>,
}

/// `name[.name[.name]] [[AS] alias]` at `index`; returns the ref and the next index.
fn table_ref(out: &[Out], mut index: usize, end: usize) -> Result<(TableRef, usize), Unavailable> {
    match out.get(index) {
        Some(Out::Op("(")) => return Err(Unavailable::DerivedTable),
        Some(Out::Ident(_)) => {}
        _ => return Err(Unavailable::UnsupportedFromClause),
    }
    let mut parts = vec![ident(out.get(index)).unwrap_or_default().to_owned()];
    index += 1;
    while index + 1 < end && out[index] == Out::Op(".") {
        let part = ident(out.get(index + 1)).ok_or(Unavailable::UnsupportedFromClause)?;
        parts.push(part.to_owned());
        index += 2;
    }
    // The small structure grammar reserves '#' for instance labels and '.'
    // for qualification. Exotic quoted table parts need a full name AST.
    if parts.iter().any(|part| {
        part.is_empty()
            || !part
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    }) {
        return Err(Unavailable::UnsupportedFromClause);
    }
    if index < end && kw(out.get(index), "AS") {
        index += 1;
    }
    let mut alias = None;
    if index < end
        && let Some(name) = ident(out.get(index))
    {
        alias = Some(name.to_owned());
        index += 1;
    }
    Ok((
        TableRef {
            name: parts.join("."),
            alias,
        },
        index,
    ))
}

/// Column references in `out[from..to]`: qualified or bare identifiers that
/// are not an alias, a type name or an INTERVAL unit.
fn column_refs(out: &[Out], from: usize, to: usize) -> Vec<Vec<String>> {
    let mut refs = Vec::new();
    let mut index = from;
    while index < to {
        let Some(first) = ident(out.get(index)) else {
            index += 1;
            continue;
        };
        let after_as = index > 0 && kw(out.get(index - 1), "AS");
        let after_dot = index > 0 && out[index - 1] == Out::Op(".");
        let interval_unit =
            index >= 2 && out[index - 1] == Out::Placeholder && kw(out.get(index - 2), "INTERVAL");
        let mut parts = vec![first.to_owned()];
        let mut next = index + 1;
        while next + 1 < to && out[next] == Out::Op(".") {
            match ident(out.get(next + 1)) {
                Some(part) => parts.push(part.to_owned()),
                None => break,
            }
            next += 2;
        }
        if !(after_as || after_dot || interval_unit) {
            refs.push(parts);
        }
        index = next;
    }
    refs
}

fn resolve(
    parts: &[String],
    aliases: &BTreeMap<String, String>,
    tables: &BTreeSet<String>,
) -> Result<String, Unavailable> {
    match parts {
        [column] if tables.len() == 1 => Ok(format!(
            "{}.{column}",
            tables.iter().next().unwrap_or(&String::new())
        )),
        [.., column] => {
            let qualifier = parts[..parts.len() - 1].join(".");
            let table = aliases
                .get(&qualifier)
                .ok_or(Unavailable::UnsupportedFromClause)?;
            Ok(format!("{table}.{column}"))
        }
        _ => Err(Unavailable::UnsupportedFromClause),
    }
}

pub(crate) fn sql_structure(
    text: &str,
    catalog: Option<&str>,
    database: Option<&str>,
) -> Result<SimilarSqlBasis, Unavailable> {
    let out =
        normalized_tokens(text, catalog, database).map_err(|_| Unavailable::TemplateUnavailable)?;
    match out.first() {
        Some(Out::Kw(kw)) if kw == "SELECT" => {}
        Some(Out::Kw(kw)) if kw == "WITH" => return Err(Unavailable::CommonTableExpression),
        _ => return Err(Unavailable::NotSelect),
    }
    // Each open parenthesis remembers whether it starts a FROM / JOIN item.
    let mut parens: Vec<bool> = Vec::new();
    for (index, token) in out.iter().enumerate() {
        match token {
            Out::Op("(") => parens.push(
                index > 0 && (kw(out.get(index - 1), "FROM") || kw(out.get(index - 1), "JOIN")),
            ),
            Out::Op(")") => {
                parens.pop();
            }
            Out::Kw(kw) if matches!(kw.as_str(), "UNION" | "EXCEPT" | "INTERSECT") => {
                return Err(Unavailable::SetOperation);
            }
            Out::Kw(kw) if kw == "SELECT" && !parens.is_empty() => {
                return Err(if parens.iter().any(|derived| *derived) {
                    Unavailable::DerivedTable
                } else {
                    Unavailable::Subquery
                });
            }
            _ => {}
        }
    }
    let from = clause_end(&out, 1, |out, index| kw(out.get(index), "FROM"));
    if from == out.len() {
        return Err(Unavailable::UnsupportedFromClause);
    }
    let from_end = clause_end(&out, from + 1, ends_from);

    // FROM: table refs and joins, in order.
    struct Join {
        kind: String,
        table: usize,
        on: Option<(usize, usize)>,
        using: Vec<String>,
    }
    let mut refs: Vec<TableRef> = Vec::new();
    let mut joins: Vec<Join> = Vec::new();
    let (first, mut index) = table_ref(&out, from + 1, from_end)?;
    refs.push(first);
    while index < from_end {
        let kind = if out[index] == Out::Op(",") {
            index += 1;
            "IMPLICIT".to_owned()
        } else {
            let mut words = Vec::new();
            while let Some(Out::Kw(word)) = out.get(index) {
                if word == "JOIN" {
                    break;
                }
                if !matches!(
                    word.as_str(),
                    "LEFT" | "RIGHT" | "FULL" | "CROSS" | "OUTER" | "SEMI" | "ANTI"
                ) {
                    return Err(Unavailable::UnsupportedFromClause);
                }
                if word != "OUTER" {
                    words.push(word.clone());
                }
                index += 1;
            }
            if !kw(out.get(index), "JOIN") {
                return Err(Unavailable::UnsupportedFromClause);
            }
            index += 1;
            if out.get(index) == Some(&Out::Op("[")) {
                let close = clause_end(&out, index + 1, |out, at| out[at] == Out::Op("]"));
                if close >= from_end || close != index + 2 || ident(out.get(index + 1)).is_none() {
                    return Err(Unavailable::UnsupportedFromClause);
                }
                index = close + 1;
            }
            if words.is_empty() {
                "INNER".to_owned()
            } else {
                words.join(" ")
            }
        };
        let (table, next) = table_ref(&out, index, from_end)?;
        refs.push(table);
        index = next;
        let mut join = Join {
            kind,
            table: refs.len() - 1,
            on: None,
            using: Vec::new(),
        };
        if kw(out.get(index), "ON") {
            let end = clause_end(&out, index + 1, |out, at| {
                out[at] == Out::Op(",") || kw(out.get(at), "JOIN") || matches!(&out[at], Out::Kw(word) if matches!(word.as_str(), "LEFT" | "RIGHT" | "FULL" | "CROSS" | "SEMI" | "ANTI"))
            })
            .min(from_end);
            join.on = Some((index + 1, end));
            index = end;
        } else if kw(out.get(index), "USING") && out.get(index + 1) == Some(&Out::Op("(")) {
            let close = clause_end(&out, index + 2, |out, at| out[at] == Out::Op(")"));
            if close >= from_end || close == index + 2 {
                return Err(Unavailable::UnsupportedFromClause);
            }
            for (position, token) in out[index + 2..close].iter().enumerate() {
                if position % 2 == 0 {
                    join.using.push(
                        ident(Some(token))
                            .ok_or(Unavailable::UnsupportedFromClause)?
                            .to_owned(),
                    );
                } else if *token != Out::Op(",") {
                    return Err(Unavailable::UnsupportedFromClause);
                }
            }
            if (close - index - 2) % 2 == 0 {
                return Err(Unavailable::UnsupportedFromClause);
            }
            index = close + 1;
        }
        joins.push(join);
    }
    if index != from_end {
        return Err(Unavailable::UnsupportedFromClause);
    }

    let mut counts = BTreeMap::<String, usize>::new();
    for table in &refs {
        *counts.entry(table.name.clone()).or_default() += 1;
    }
    let mut seen = BTreeMap::<String, usize>::new();
    let instances: Vec<String> = refs
        .iter()
        .map(|table| {
            let ordinal = seen.entry(table.name.clone()).or_default();
            *ordinal += 1;
            if counts[&table.name] > 1 {
                format!("{}#{ordinal}", table.name)
            } else {
                table.name.clone()
            }
        })
        .collect();
    let tables: BTreeSet<String> = instances.iter().cloned().collect();
    let mut aliases = BTreeMap::new();
    for (table, instance) in refs.iter().zip(&instances) {
        let exposed = table.alias.as_ref().unwrap_or(&table.name);
        if aliases
            .insert(exposed.clone(), instance.clone())
            .is_some_and(|old| old != *instance)
        {
            return Err(Unavailable::UnsupportedFromClause);
        }
    }
    let mut predicates = BTreeSet::new();
    let mut join_shapes = BTreeSet::new();
    for join in &joins {
        let right = &instances[join.table];
        let mut edges = Vec::new();
        if let Some((start, end)) = join.on {
            // Split ON by depth-0 AND; `col = col` is a join edge, anything else is a filter.
            let mut conjunct = start;
            while conjunct < end {
                let stop = clause_end(&out, conjunct, |out, at| kw(out.get(at), "AND")).min(end);
                let columns = column_refs(&out, conjunct, stop);
                let equality = stop - conjunct >= 3
                    && columns.len() == 2
                    && out[conjunct..stop]
                        .iter()
                        .filter(|token| **token == Out::Op("="))
                        .count()
                        == 1
                    && out[conjunct..stop]
                        .iter()
                        .all(|token| matches!(token, Out::Ident(_) | Out::Op("." | "=")));
                if equality {
                    let mut pair = [
                        resolve(&columns[0], &aliases, &tables)?,
                        resolve(&columns[1], &aliases, &tables)?,
                    ];
                    pair.sort();
                    edges.push(format!("{} = {}", pair[0], pair[1]));
                } else {
                    predicates.extend(
                        columns
                            .iter()
                            .map(|parts| resolve(parts, &aliases, &tables))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                conjunct = stop + 1;
            }
        }
        for column in &join.using {
            // Beyond two inputs, resolving a merged USING column needs
            // catalog/type semantics. Do not invent an all-left column.
            if join.table != 1 {
                return Err(Unavailable::UnsupportedFromClause);
            }
            let left = &instances[0];
            let mut pair = [format!("{left}.{column}"), format!("{right}.{column}")];
            pair.sort();
            edges.push(format!("{} = {}", pair[0], pair[1]));
        }
        if !matches!(join.kind.as_str(), "INNER" | "CROSS" | "IMPLICIT") {
            // Non-commutative joins retain the actual left relation and right
            // input even when an equality pair sorts to the same spelling.
            let left = instances[..join.table].join(", ");
            let condition = if edges.is_empty() {
                String::new()
            } else {
                format!(" ON {}", edges.join(" AND "))
            };
            join_shapes.insert(format!("{} JOIN ({left}) -> {right}{condition}", join.kind));
        } else if edges.is_empty() {
            join_shapes.insert(format!("{} JOIN {right}", join.kind));
        } else {
            for edge in edges {
                join_shapes.insert(format!("{} JOIN {edge}", join.kind));
            }
        }
    }
    if kw(out.get(from_end), "WHERE") {
        let where_end = clause_end(&out, from_end + 1, ends_where);
        predicates.extend(
            column_refs(&out, from_end + 1, where_end)
                .iter()
                .map(|parts| resolve(parts, &aliases, &tables))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(SimilarSqlBasis {
        tables: {
            let mut names: Vec<_> = refs.into_iter().map(|t| t.name).collect();
            names.sort();
            names
        },
        joins: join_shapes.into_iter().collect(),
        predicate_columns: predicates.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_v1_08_outer_join_sides_and_self_join_instances_are_preserved() {
        assert_ne!(
            basis("SELECT * FROM a LEFT JOIN b ON a.k=b.k").unwrap(),
            basis("SELECT * FROM b LEFT JOIN a ON a.k=b.k").unwrap()
        );
        let cross = basis("SELECT * FROM t a JOIN t b ON a.parent_id=b.id").unwrap();
        let same = basis("SELECT * FROM t a JOIN t b ON a.parent_id=a.id").unwrap();
        assert_ne!(cross, same);
        assert_eq!(cross.tables.len(), 2);
        assert_eq!(
            cross,
            basis("SELECT * FROM t x JOIN t y ON x.parent_id=y.id").unwrap()
        );
        // Retained A1/A2 E2E input: the unaliased first t remains visible,
        // while the second occurrence is exposed only as u.
        assert_eq!(
            basis("SELECT t.id FROM t INNER JOIN t AS u ON t.id=u.id").unwrap(),
            basis("SELECT a.id FROM t a JOIN t b ON a.id=b.id").unwrap()
        );
        assert_eq!(
            basis("SELECT * FROM a b JOIN b c ON b.k=c.k").unwrap(),
            basis("SELECT * FROM a JOIN b ON a.k=b.k").unwrap()
        );
        assert!(basis("SELECT * FROM t a JOIN t a ON a.parent_id=a.id").is_err());
        assert!(basis("SELECT * FROM a b JOIN b ON b.k=b.k").is_err());
        assert!(basis("SELECT * FROM t a JOIN t b ON parent_id=id").is_err());
    }

    #[test]
    fn audit_v1_05_using_stops_at_its_close_and_consumes_every_join() {
        for (using, on) in [
            (
                "SELECT * FROM a JOIN b USING(k) WHERE a.flag=1",
                "SELECT * FROM a JOIN b ON a.k=b.k WHERE a.flag=1",
            ),
            (
                "SELECT * FROM a JOIN b USING(k) JOIN c ON b.x=c.x",
                "SELECT * FROM a JOIN b ON a.k=b.k JOIN c ON b.x=c.x",
            ),
            (
                "SELECT * FROM a JOIN [broadcast] b USING(k) WHERE a.flag=1",
                "SELECT * FROM a JOIN b ON a.k=b.k WHERE a.flag=1",
            ),
        ] {
            assert_eq!(basis(using).unwrap(), basis(on).unwrap(), "{using}");
        }
        assert!(basis("SELECT * FROM a JOIN b USING(k + other)").is_err());
        assert!(basis("SELECT * FROM a JOIN b USING()").is_err());
    }

    fn basis(text: &str) -> Result<SimilarSqlBasis, Unavailable> {
        sql_structure(text, Some("default_catalog"), Some("demo"))
    }

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn aliases_resolve_and_projection_does_not_matter() {
        let wide = basis("SELECT o.*, c.region FROM orders o JOIN customers c ON o.cid = c.id WHERE c.region = 'x' AND o.dt >= '2024-01-01'").unwrap();
        let narrow = basis("select a.id, a.amount from demo.orders as a inner join customers b on b.id = a.cid where b.region = ? and a.dt >= ?").unwrap();
        assert_eq!(wide, narrow);
        assert_eq!(wide.tables, owned(&["customers", "orders"]));
        assert_eq!(wide.joins, owned(&["INNER JOIN customers.id = orders.cid"]));
        assert_eq!(
            wide.predicate_columns,
            owned(&["customers.region", "orders.dt"])
        );
    }

    #[test]
    fn single_table_columns_are_qualified_and_clauses_are_ignored() {
        let b = basis("SELECT customer_id, SUM(amount) FROM orders WHERE dt = ? AND date_sub(dt, INTERVAL 1 day) > ? GROUP BY customer_id ORDER BY 2 DESC LIMIT 10").unwrap();
        assert_eq!(b.tables, owned(&["orders"]));
        assert!(b.joins.is_empty());
        assert_eq!(b.predicate_columns, owned(&["orders.dt"]));
        assert_eq!(
            basis("SELECT CAST(id AS DATE) FROM t")
                .unwrap()
                .predicate_columns,
            Vec::<String>::new()
        );
    }

    #[test]
    fn join_kinds_using_implicit_and_filters_are_kept_apart() {
        assert_eq!(
            basis("SELECT 1 FROM a LEFT OUTER JOIN b ON a.k = b.k")
                .unwrap()
                .joins,
            owned(&["LEFT JOIN (a) -> b ON a.k = b.k"])
        );
        assert_eq!(
            basis("SELECT 1 FROM a LEFT JOIN b USING (k)")
                .unwrap()
                .joins,
            owned(&["LEFT JOIN (a) -> b ON a.k = b.k"])
        );
        assert_eq!(
            basis("SELECT 1 FROM a, b WHERE a.k = b.k").unwrap().joins,
            owned(&["IMPLICIT JOIN b"])
        );
        let filtered = basis("SELECT 1 FROM a JOIN b ON a.k = b.k AND b.flag = 1").unwrap();
        assert_eq!(filtered.joins, owned(&["INNER JOIN a.k = b.k"]));
        assert_eq!(filtered.predicate_columns, owned(&["b.flag"]));
        assert_ne!(
            basis("SELECT 1 FROM a JOIN b ON a.k = b.k").unwrap(),
            basis("SELECT 1 FROM a LEFT JOIN b ON a.k = b.k").unwrap()
        );
    }

    #[test]
    fn shapes_outside_the_grammar_are_unavailable() {
        for (text, reason) in [
            ("SELECT 'open", Unavailable::TemplateUnavailable),
            ("INSERT INTO t SELECT * FROM s", Unavailable::NotSelect),
            (
                "WITH x AS (SELECT 1) SELECT * FROM x",
                Unavailable::CommonTableExpression,
            ),
            (
                "SELECT id FROM a UNION ALL SELECT id FROM b",
                Unavailable::SetOperation,
            ),
            (
                "SELECT id FROM a WHERE id IN (SELECT id FROM b)",
                Unavailable::Subquery,
            ),
            (
                "SELECT (SELECT max(id) FROM b) FROM a",
                Unavailable::Subquery,
            ),
            ("SELECT * FROM (SELECT 1) t", Unavailable::DerivedTable),
            (
                "SELECT * FROM unnest(x)",
                Unavailable::UnsupportedFromClause,
            ),
            ("SELECT 1", Unavailable::UnsupportedFromClause),
        ] {
            assert_eq!(basis(text), Err(reason), "{text}");
        }
    }
}
