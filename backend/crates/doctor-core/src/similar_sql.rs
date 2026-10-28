//! A2 similar-SQL clustering of exact templates. A cluster is two or more
//! members sharing one catalog, database and the enabled parts of their
//! structural basis; its totals are member sums. Detaching and predicate
//! refinement are per-request inputs, never stored.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{
    AuditFamilySort, ProductSqlFingerprint, SIMILAR_SQL_STRUCTURE_VERSION, SimilarSqlBasis,
    SimilarSqlDimensions, SimilarSqlMemberSource, SimilarSqlStructure, SimilarSqlUnavailableReason,
};
use sha2::{Digest, Sha256};

use crate::sql_structure::sql_structure;
use crate::{FamilyCells, SqlFingerprintInput, compare_family_cells, product_sql_fingerprint};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimilarSqlInput {
    pub member_key: String,
    pub source: SimilarSqlMemberSource,
    pub catalog: String,
    pub database: String,
    /// A member statement (official family) or the template (fingerprint group).
    pub statement: Option<String>,
    pub statement_truncated: bool,
    pub cells: FamilyCells,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimilarSqlMemberOut {
    pub input: SimilarSqlInput,
    pub template: Option<String>,
    pub structure: SimilarSqlStructure,
    pub detached: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimilarSqlClusterOut {
    pub cluster_key: String,
    pub catalog: String,
    pub database: String,
    pub basis: SimilarSqlBasis,
    pub refined: bool,
    pub members: Vec<SimilarSqlMemberOut>,
    pub cells: FamilyCells,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimilarSqlOut {
    pub clusters: Vec<SimilarSqlClusterOut>,
    pub independent: Vec<SimilarSqlMemberOut>,
    pub unavailable: Vec<SimilarSqlMemberOut>,
}

fn keyed_basis(
    basis: &SimilarSqlBasis,
    dimensions: &SimilarSqlDimensions,
    predicates: bool,
) -> SimilarSqlBasis {
    SimilarSqlBasis {
        tables: if dimensions.tables {
            basis.tables.clone()
        } else {
            vec![]
        },
        joins: if dimensions.join {
            basis.joins.clone()
        } else {
            vec![]
        },
        predicate_columns: if predicates {
            basis.predicate_columns.clone()
        } else {
            vec![]
        },
    }
}

fn cluster_key(
    catalog: &str,
    database: &str,
    dimensions: &SimilarSqlDimensions,
    refined: bool,
    basis: &SimilarSqlBasis,
) -> String {
    let mut hasher = Sha256::new();
    let flags = format!(
        "{}{}{}{}",
        u8::from(dimensions.tables),
        u8::from(dimensions.join),
        u8::from(dimensions.predicates),
        u8::from(refined)
    );
    let parts = [SIMILAR_SQL_STRUCTURE_VERSION, catalog, database, &flags]
        .into_iter()
        .map(str::to_owned)
        .chain(basis.tables.iter().map(|value| format!("t:{value}")))
        .chain(basis.joins.iter().map(|value| format!("j:{value}")))
        .chain(
            basis
                .predicate_columns
                .iter()
                .map(|value| format!("p:{value}")),
        );
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("similar-v1:{:x}", hasher.finalize())
}

fn by_cost(
    sort: &AuditFamilySort,
) -> impl Fn(&SimilarSqlMemberOut, &SimilarSqlMemberOut) -> std::cmp::Ordering + '_ {
    move |left, right| {
        compare_family_cells(sort, &left.input.cells, &right.input.cells)
            .then_with(|| left.input.member_key.cmp(&right.input.member_key))
    }
}

pub fn cluster_similar_sql(
    inputs: Vec<SimilarSqlInput>,
    dimensions: &SimilarSqlDimensions,
    detached: &BTreeSet<String>,
    refined: &BTreeSet<String>,
    sort: &AuditFamilySort,
) -> SimilarSqlOut {
    let mut out = SimilarSqlOut::default();
    let mut groups: BTreeMap<String, (SimilarSqlBasis, Vec<SimilarSqlMemberOut>)> = BTreeMap::new();
    for input in inputs {
        let fingerprint = product_sql_fingerprint(SqlFingerprintInput {
            statement: input.statement.as_deref(),
            truncated: input.statement_truncated,
            catalog: Some(&input.catalog),
            database: Some(&input.database),
        });
        let template = match fingerprint {
            ProductSqlFingerprint::Exact { template, .. } => Some(template),
            _ => None,
        };
        let structure = match (&template, &input.statement) {
            (Some(_), Some(statement)) => {
                sql_structure(statement, Some(&input.catalog), Some(&input.database))
            }
            _ => Err(SimilarSqlUnavailableReason::TemplateUnavailable),
        };
        let basis = match structure {
            Ok(basis) => basis,
            Err(reason) => {
                out.unavailable.push(SimilarSqlMemberOut {
                    input,
                    template,
                    structure: SimilarSqlStructure::Unavailable { reason },
                    detached: false,
                });
                continue;
            }
        };
        let member = SimilarSqlMemberOut {
            detached: detached.contains(&input.member_key),
            input,
            template,
            structure: SimilarSqlStructure::Available {
                basis: basis.clone(),
            },
        };
        if member.detached {
            out.independent.push(member);
            continue;
        }
        let key_basis = keyed_basis(&basis, dimensions, dimensions.predicates);
        let key = cluster_key(
            &member.input.catalog,
            &member.input.database,
            dimensions,
            false,
            &key_basis,
        );
        groups
            .entry(key)
            .or_insert_with(|| (key_basis, Vec::new()))
            .1
            .push(member);
    }
    // Refinement: a requested cluster is regrouped by predicate columns too.
    let mut finals: Vec<(String, SimilarSqlBasis, bool, Vec<SimilarSqlMemberOut>)> = Vec::new();
    for (key, (key_basis, members)) in groups {
        if refined.contains(&key) && !dimensions.predicates {
            let mut parts: BTreeMap<String, (SimilarSqlBasis, Vec<SimilarSqlMemberOut>)> =
                BTreeMap::new();
            for member in members {
                let SimilarSqlStructure::Available { basis } = &member.structure else {
                    continue;
                };
                let sub_basis = keyed_basis(basis, dimensions, true);
                let sub_key = cluster_key(
                    &member.input.catalog,
                    &member.input.database,
                    dimensions,
                    true,
                    &sub_basis,
                );
                parts
                    .entry(sub_key)
                    .or_insert_with(|| (sub_basis, Vec::new()))
                    .1
                    .push(member);
            }
            finals.extend(
                parts
                    .into_iter()
                    .map(|(sub_key, (basis, members))| (sub_key, basis, true, members)),
            );
        } else {
            finals.push((key, key_basis, false, members));
        }
    }
    for (cluster_key, basis, refined, mut members) in finals {
        if members.len() < 2 {
            out.independent.append(&mut members);
            continue;
        }
        members.sort_by(by_cost(sort));
        let mut cells = FamilyCells::default();
        for member in &members {
            cells.add(&member.input.cells);
        }
        let first = &members[0].input;
        out.clusters.push(SimilarSqlClusterOut {
            catalog: first.catalog.clone(),
            database: first.database.clone(),
            cluster_key,
            basis,
            refined,
            members,
            cells,
        });
    }
    out.clusters.sort_by(|left, right| {
        compare_family_cells(sort, &left.cells, &right.cells)
            .then_with(|| left.cluster_key.cmp(&right.cluster_key))
    });
    out.independent.sort_by(by_cost(sort));
    out.unavailable.sort_by(by_cost(sort));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetricCells;

    fn input(key: &str, sql: &str, executions: u64, query_time: u128) -> SimilarSqlInput {
        SimilarSqlInput {
            member_key: key.into(),
            source: SimilarSqlMemberSource::OfficialDigest {
                native_digest: key.into(),
            },
            catalog: "default_catalog".into(),
            database: "demo".into(),
            statement: Some(sql.into()),
            statement_truncated: false,
            cells: FamilyCells {
                execution_count: executions,
                query_time: MetricCells {
                    valid_count: executions,
                    sum: Some(query_time),
                    max: Some(query_time),
                },
                ..FamilyCells::default()
            },
        }
    }

    fn all() -> SimilarSqlDimensions {
        SimilarSqlDimensions {
            tables: true,
            join: true,
            predicates: true,
        }
    }

    fn members() -> Vec<SimilarSqlInput> {
        vec![
            input(
                "a",
                "SELECT o.*, c.region FROM orders o JOIN customers c ON o.cid = c.id WHERE c.region = 'x' AND o.dt >= ?",
                188,
                2472,
            ),
            input(
                "b",
                "SELECT o.id FROM orders o JOIN customers c ON o.cid = c.id WHERE c.region = ? AND o.dt >= ?",
                143,
                2162,
            ),
            input(
                "c",
                "SELECT o.id FROM orders o JOIN customers c ON o.cid = c.id WHERE o.dt >= ?",
                10,
                50,
            ),
            input("d", "SELECT id FROM t WHERE id IN (SELECT id FROM s)", 5, 5),
            input("e", "SELECT customer_id FROM orders WHERE dt = ?", 1, 1),
        ]
    }

    #[test]
    fn clusters_need_two_members_and_keep_unavailable_apart() {
        let out = cluster_similar_sql(
            members(),
            &all(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &AuditFamilySort::CumulativeQueryTimeDesc,
        );
        assert_eq!(out.clusters.len(), 1);
        let cluster = &out.clusters[0];
        assert_eq!(
            cluster
                .members
                .iter()
                .map(|m| m.input.member_key.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(cluster.cells.execution_count, 331);
        assert_eq!(cluster.cells.query_time.sum, Some(4634));
        assert_eq!(
            cluster.basis.predicate_columns,
            ["customers.region", "orders.dt"]
        );
        assert_eq!(
            out.independent
                .iter()
                .map(|m| m.input.member_key.as_str())
                .collect::<Vec<_>>(),
            ["c", "e"]
        );
        assert_eq!(out.unavailable.len(), 1);
        assert!(matches!(
            out.unavailable[0].structure,
            SimilarSqlStructure::Unavailable {
                reason: SimilarSqlUnavailableReason::Subquery
            }
        ));
    }

    #[test]
    fn disabling_predicates_merges_and_refinement_splits_again() {
        let loose = SimilarSqlDimensions {
            predicates: false,
            ..all()
        };
        let merged = cluster_similar_sql(
            members(),
            &loose,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &AuditFamilySort::ExecutionCountDesc,
        );
        assert_eq!(merged.clusters[0].members.len(), 3);
        assert!(merged.clusters[0].basis.predicate_columns.is_empty());
        let key = merged.clusters[0].cluster_key.clone();
        let refined = cluster_similar_sql(
            members(),
            &loose,
            &BTreeSet::new(),
            &BTreeSet::from([key]),
            &AuditFamilySort::ExecutionCountDesc,
        );
        assert_eq!(refined.clusters.len(), 1);
        assert!(refined.clusters[0].refined);
        assert_eq!(refined.clusters[0].members.len(), 2);
    }

    #[test]
    fn a_detached_member_leaves_its_cluster_for_this_request() {
        let out = cluster_similar_sql(
            members(),
            &all(),
            &BTreeSet::from(["b".to_owned()]),
            &BTreeSet::new(),
            &AuditFamilySort::CumulativeQueryTimeDesc,
        );
        assert!(out.clusters.is_empty());
        assert!(
            out.independent
                .iter()
                .any(|m| m.input.member_key == "b" && m.detached)
        );
    }
}
