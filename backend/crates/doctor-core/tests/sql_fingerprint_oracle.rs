//! RA-01a: product fingerprints against official StarRocks 3.5.14 digests
//! captured on the fixed-digest image (fixtures/oracle/sql-fingerprint; the
//! capture script is there too).
//! Two cases the official digest separates must never share a fingerprint.
//! Two cases it merges must share one, unless the pair is a documented
//! analysis-dependent over-split below; a case the product reports
//! unavailable takes part in neither and must be listed as such.
use std::collections::{BTreeMap, BTreeSet};

use doctor_contracts::{ProductSqlFingerprint, ProductSqlFingerprintUnavailableReason};
use doctor_core::{SqlFingerprintInput, product_sql_fingerprint};
use serde_json::Value;

const ORACLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/oracle/sql-fingerprint"
);

/// Cases this token-level normalizer deliberately keeps out of their official
/// class: each merge needs analysis the product does not reproduce.
const DOCUMENTED_OVER_SPLITS: &[(&str, &str)] = &[(
    "paren_redundant",
    "the analyser drops redundant parentheses around a predicate",
)];

const EXPECTED_UNAVAILABLE: &[(&str, ProductSqlFingerprintUnavailableReason)] = &[
    (
        "or_16",
        ProductSqlFingerprintUnavailableReason::MassiveCompoundPredicate,
    ),
    (
        "or_17",
        ProductSqlFingerprintUnavailableReason::MassiveCompoundPredicate,
    ),
    (
        "or_16_mixed_ops",
        ProductSqlFingerprintUnavailableReason::MassiveCompoundPredicate,
    ),
    (
        "or_16_two_cols",
        ProductSqlFingerprintUnavailableReason::MassiveCompoundPredicate,
    ),
    (
        "cm_hash_tail",
        ProductSqlFingerprintUnavailableReason::LexicalError,
    ),
];

fn read(name: &str) -> Value {
    let path = format!("{ORACLE}/{name}");
    serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}")),
    )
    .unwrap()
}

#[test]
fn product_fingerprints_never_merge_what_the_official_digest_separates() {
    let cases = read("cases.json");
    let official = read("official-digests.json");
    assert_eq!(
        official["fe_image"],
        "starrocks/fe-ubuntu:3.5.14@sha256:3d786dba52ea5e72a5815c438b8c45e041c647730f69c6728783f900e6798127"
    );
    let digests: BTreeMap<String, Option<String>> = official["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["id"].as_str().unwrap().to_owned(),
                case["digest"].as_str().map(str::to_owned),
            )
        })
        .collect();

    let mut fingerprints = BTreeMap::new();
    let mut unavailable = BTreeMap::new();
    for case in cases["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap().to_owned();
        match product_sql_fingerprint(SqlFingerprintInput {
            statement: case["sql"].as_str(),
            truncated: false,
            catalog: Some("default_catalog"),
            database: cases["database"].as_str(),
        }) {
            ProductSqlFingerprint::Exact { fingerprint, .. } => {
                fingerprints.insert(id, fingerprint);
            }
            ProductSqlFingerprint::Unavailable { reason } => {
                unavailable.insert(id, reason);
            }
            ProductSqlFingerprint::Partial { .. } => panic!("{id}: not truncated"),
        }
    }
    assert_eq!(digests.len(), fingerprints.len() + unavailable.len());
    assert_eq!(
        unavailable,
        EXPECTED_UNAVAILABLE
            .iter()
            .map(|(id, reason)| ((*id).to_owned(), *reason))
            .collect(),
    );

    let documented: BTreeSet<&str> = DOCUMENTED_OVER_SPLITS.iter().map(|(id, _)| *id).collect();
    let (mut over_merged, mut over_split, mut compared) = (Vec::new(), Vec::new(), 0);
    let mut documented_seen = BTreeSet::new();
    let ids: Vec<&String> = fingerprints.keys().collect();
    for (position, left) in ids.iter().enumerate() {
        for right in &ids[position + 1..] {
            let (Some(Some(left_digest)), Some(Some(right_digest))) =
                (digests.get(*left), digests.get(*right))
            else {
                continue;
            };
            compared += 1;
            let same_official = left_digest == right_digest;
            let same_product = fingerprints[*left] == fingerprints[*right];
            if same_product && !same_official {
                over_merged.push(format!("{left} ~ {right}"));
            }
            if same_official && !same_product {
                match [left.as_str(), right.as_str()]
                    .into_iter()
                    .find(|id| documented.contains(id))
                {
                    Some(id) => {
                        documented_seen.insert(id);
                    }
                    None => over_split.push(format!("{left} / {right}")),
                }
            }
        }
    }
    assert!(
        over_merged.is_empty(),
        "merged across official digests: {over_merged:?}"
    );
    assert!(
        over_split.is_empty(),
        "undocumented over-splits: {over_split:?}"
    );
    // Every documented over-split still happens, so the list stays truthful.
    assert_eq!(documented_seen, documented);
    assert!(compared > 5000, "compared {compared} pairs");
}
