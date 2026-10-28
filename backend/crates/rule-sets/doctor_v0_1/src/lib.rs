//! Frozen deterministic findings. This is deliberately a fixed eleven-rule
//! contract, not a generic rule language.

use doctor_contracts::{
    ExpectedMetricChange, FindingConfidence, RecommendationKind, RecommendationResultEffect,
    RecommendationRiskLevel, canonical_hash,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

pub const RULESET_ID: &str = "doctor-v0.1";
pub const PRODUCT_RULESET_VERSION: &str = "0.3.0";
pub const SEMANTIC_MANIFEST_HASH: &str =
    "84667b9b849fbda0a82933cf908b9884429cb85cee2c69cb7cd0c4fd91374784";
pub const OUTER_MANIFEST_HASH: &str =
    "01047013d50d4eee3ef23358063c7974c2d22b19a958d36b37f0980e1f1668c7";
pub const SCAN_TIME_SHARE_RULE_ID: &str = "SR3514.SCAN_TIME_SHARE";
pub const JOIN_TIME_SHARE_RULE_ID: &str = "SR3514.JOIN_TIME_SHARE";
pub const AGGREGATE_TIME_SHARE_RULE_ID: &str = "SR3514.AGGREGATE_TIME_SHARE";
pub const EXCHANGE_NETWORK_TIME_SHARE_RULE_ID: &str = "SR3514.EXCHANGE_NETWORK_TIME_SHARE";
pub const SPILL_OBSERVED_RULE_ID: &str = "SR3514.SPILL_OBSERVED";
pub const ROW_FLOW_AMPLIFICATION_RULE_ID: &str = "SR3514.ROW_FLOW_AMPLIFICATION";
pub const NODE_TIME_SHARE_RULE_ID: &str = "SR3514.NODE_TIME_SHARE";
pub const OPERATOR_TIME_SKEW_RULE_ID: &str = "SR3514.OPERATOR_TIME_SKEW";
pub const JOIN_PROBE_FANOUT_RULE_ID: &str = "SR3514.JOIN_PROBE_FANOUT";
pub const BROADCAST_EXCHANGE_BYTES_RULE_ID: &str = "SR3514.BROADCAST_EXCHANGE_BYTES";
pub const STREAMING_PREAGG_INEFFECTIVE_RULE_ID: &str = "SR3514.STREAMING_PREAGG_INEFFECTIVE";
/// Product decision (ruleset 0.1.5, not an official StarRocks threshold): a
/// time-share finding whose subject operator time is entirely below this floor
/// is suppressed. A sub-100 ms operator is rarely worth a field investigation
/// even when it dominates a short query.
pub const TIME_SHARE_ABSOLUTE_FLOOR_NS: i128 = 100_000_000;
const TIME_RULE_VERSION: &str = "0.3.0";
const OTHER_RULE_VERSION: &str = "0.1.0";
const ROW_RULE_VERSION: &str = "0.2.0";

const RULESET_JSON: &str = include_str!("../ruleset.json");
const TIME_SHARE_RULE_IDS: [&str; 5] = [
    SCAN_TIME_SHARE_RULE_ID,
    JOIN_TIME_SHARE_RULE_ID,
    AGGREGATE_TIME_SHARE_RULE_ID,
    EXCHANGE_NETWORK_TIME_SHARE_RULE_ID,
    NODE_TIME_SHARE_RULE_ID,
];
const ACTIVE_RULE_IDS: [&str; 11] = [
    SCAN_TIME_SHARE_RULE_ID,
    JOIN_TIME_SHARE_RULE_ID,
    AGGREGATE_TIME_SHARE_RULE_ID,
    EXCHANGE_NETWORK_TIME_SHARE_RULE_ID,
    SPILL_OBSERVED_RULE_ID,
    ROW_FLOW_AMPLIFICATION_RULE_ID,
    NODE_TIME_SHARE_RULE_ID,
    OPERATOR_TIME_SKEW_RULE_ID,
    JOIN_PROBE_FANOUT_RULE_ID,
    BROADCAST_EXCHANGE_BYTES_RULE_ID,
    STREAMING_PREAGG_INEFFECTIVE_RULE_ID,
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RuleSetManifest {
    pub compatible_pack_ids: Vec<String>,
    pub ir_schema_range: String,
    pub manifest_hash: String,
    pub registry_requirements: Vec<String>,
    pub rules: Vec<RuleDefinition>,
    pub ruleset_id: String,
    pub semantic_manifest_hash: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RuleDefinition {
    pub applicability: String,
    pub category: String,
    pub confidence_policy: String,
    pub evaluator_id: String,
    pub evaluator_version: String,
    pub finding_policy: FindingPolicy,
    pub formula_source: String,
    pub golden_case_ids: Vec<String>,
    pub limitations: Vec<String>,
    pub recommendation_template: RecommendationTemplate,
    pub rule_id: String,
    pub severity_policy: String,
    pub subject_kinds: Vec<String>,
    pub threshold_source: String,
    pub thresholds: Vec<String>,
    pub typed_inputs: Vec<String>,
    pub version: String,
    #[serde(default)]
    pub absolute_floor: Option<AbsoluteFloor>,
}

/// Suppresses an otherwise triggered or suspected finding when the whole
/// subject time range is strictly below `suppress_below_ns`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct AbsoluteFloor {
    pub input: String,
    pub suppress_below_ns: i128,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct FindingPolicy {
    pub bucket_severity: Option<BucketSeverity>,
    pub confidence: ConfidencePolicy,
    pub severity: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct BucketSeverity {
    pub attention_15: String,
    pub attention_30: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ConfidencePolicy {
    pub bounded_definite: FindingConfidence,
    pub exact_definite: FindingConfidence,
    pub suspected: FindingConfidence,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RecommendationTemplate {
    pub expected_direction: String,
    pub risk: String,
    pub validation_steps: Vec<String>,
    /// Structured recommendation (ruleset 0.3.0): product-authored text, not an official StarRocks rule.
    pub recommendation_id: String,
    pub kind: RecommendationKind,
    pub title: String,
    pub next_step: String,
    pub risk_level: RecommendationRiskLevel,
    pub result_effect: RecommendationResultEffect,
    pub steps: Vec<RecommendationStepTemplate>,
    pub expected_changes: Vec<ExpectedMetricChange>,
    pub limitations: Vec<String>,
}
/// One investigation step; `evidence_metric_ids` name metrics of the finding's
/// own subject, bound to exact metric cells when the finding is built.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RecommendationStepTemplate {
    pub title: String,
    pub detail: String,
    pub evidence_metric_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorRuleSet {
    manifest: RuleSetManifest,
}
impl DoctorRuleSet {
    pub fn manifest(&self) -> &RuleSetManifest {
        &self.manifest
    }
    pub fn rule(&self, rule_id: &str) -> Option<&RuleDefinition> {
        self.manifest
            .rules
            .iter()
            .find(|rule| rule.rule_id == rule_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuleSetError {
    InvalidJson,
    InvalidRoot,
    InvalidHash(&'static str),
    HashMismatch(&'static str),
    IdentityMismatch(&'static str),
    RuleContractMismatch(&'static str),
}

/// Loads and validates the checked-in v0.1 RuleSet without filesystem or network access.
pub fn load() -> Result<DoctorRuleSet, RuleSetError> {
    let document = serde_json::from_str(RULESET_JSON).map_err(|_| RuleSetError::InvalidJson)?;
    validate_document(document)
}

fn validate_document(document: Value) -> Result<DoctorRuleSet, RuleSetError> {
    document.as_object().ok_or(RuleSetError::InvalidRoot)?;
    let manifest: RuleSetManifest =
        serde_json::from_value(document.clone()).map_err(|_| RuleSetError::InvalidJson)?;
    if manifest.ruleset_id != RULESET_ID {
        return Err(RuleSetError::IdentityMismatch("ruleset_id"));
    }
    if manifest.version != PRODUCT_RULESET_VERSION {
        return Err(RuleSetError::IdentityMismatch("version"));
    }
    if !is_sha256(&manifest.semantic_manifest_hash) || !is_sha256(&manifest.manifest_hash) {
        return Err(RuleSetError::InvalidHash("ruleset"));
    }
    if canonical_hash(&without(
        &document,
        &["manifest_hash", "semantic_manifest_hash"],
    )?) != manifest.semantic_manifest_hash
    {
        return Err(RuleSetError::HashMismatch("semantic_manifest_hash"));
    }
    if canonical_hash(&without(&document, &["manifest_hash"])?) != manifest.manifest_hash {
        return Err(RuleSetError::HashMismatch("manifest_hash"));
    }
    if manifest.compatible_pack_ids != ["starrocks-3.5.14"]
        || manifest.ir_schema_range != "1.x"
        || manifest.registry_requirements != ["sr3514-registry@0.3.2"]
        || manifest.rules.len() != ACTIVE_RULE_IDS.len()
    {
        return Err(RuleSetError::RuleContractMismatch("root"));
    }
    let ids = manifest
        .rules
        .iter()
        .map(|rule| rule.rule_id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != ACTIVE_RULE_IDS.len() || !ACTIVE_RULE_IDS.iter().all(|id| ids.contains(id)) {
        return Err(RuleSetError::RuleContractMismatch("active_rule_ids"));
    }
    let mut cases = BTreeSet::new();
    let mut recommendation_ids = BTreeSet::new();
    for rule in &manifest.rules {
        let expected_version = if TIME_SHARE_RULE_IDS.contains(&rule.rule_id.as_str()) {
            TIME_RULE_VERSION
        } else if rule.rule_id == ROW_FLOW_AMPLIFICATION_RULE_ID {
            ROW_RULE_VERSION
        } else {
            OTHER_RULE_VERSION
        };
        if rule.version != expected_version
            || rule.evaluator_version != "1"
            || rule.golden_case_ids.len() < 6
            || rule
                .golden_case_ids
                .iter()
                .any(|case| case.trim().is_empty() || !cases.insert(case.as_str()))
            || rule.applicability.is_empty()
            || rule.confidence_policy.is_empty()
            || rule.formula_source.is_empty()
            || rule.severity_policy != "product_decision_v0.1"
            || rule.subject_kinds.is_empty()
            || rule.threshold_source.is_empty()
            || rule.thresholds.is_empty()
            || rule.typed_inputs.is_empty()
            || rule.limitations.is_empty()
            || rule.recommendation_template.validation_steps.is_empty()
        {
            return Err(RuleSetError::RuleContractMismatch("rule_shape"));
        }
        validate_recommendation(rule, &mut recommendation_ids)?;
    }
    validate_time_rule(
        &manifest,
        SCAN_TIME_SHARE_RULE_ID,
        "SCAN",
        &[
            "OperatorTotalTime",
            "ScanTime",
            "QueryCumulativeOperatorTime",
        ],
    )?;
    validate_time_rule(
        &manifest,
        JOIN_TIME_SHARE_RULE_ID,
        "JOIN",
        &["OperatorTotalTime", "QueryCumulativeOperatorTime"],
    )?;
    validate_time_rule(
        &manifest,
        AGGREGATE_TIME_SHARE_RULE_ID,
        "AGGREGATE",
        &["OperatorTotalTime", "QueryCumulativeOperatorTime"],
    )?;
    validate_time_rule(
        &manifest,
        EXCHANGE_NETWORK_TIME_SHARE_RULE_ID,
        "EXCHANGE",
        &[
            "OperatorTotalTime",
            "NetworkTime",
            "QueryCumulativeOperatorTime",
        ],
    )?;
    validate_time_rule(
        &manifest,
        NODE_TIME_SHARE_RULE_ID,
        "other plan nodes",
        &["OperatorTotalTime", "QueryCumulativeOperatorTime"],
    )?;
    validate_spill_rule(&manifest)?;
    validate_row_rule(&manifest)?;
    for (id, evaluator, inputs, thresholds) in SIGNAL_RULES {
        validate_signal_rule(&manifest, id, evaluator, inputs, thresholds)?;
    }
    // Bind the fully validated semantic document to the checked-in receipt only
    // after contract validation. This keeps a re-hashed semantic drift
    // distinguishable from a merely stale receipt.
    if manifest.semantic_manifest_hash != SEMANTIC_MANIFEST_HASH {
        return Err(RuleSetError::IdentityMismatch("semantic_manifest_hash"));
    }
    if manifest.manifest_hash != OUTER_MANIFEST_HASH {
        return Err(RuleSetError::IdentityMismatch("manifest_hash"));
    }
    Ok(DoctorRuleSet { manifest })
}

/// Time-share rules point at where the cost is; every other rule is a concrete
/// investigation. Ids are `rec.<area>.<action>@v<N>` and unique in the set.
fn validate_recommendation<'a>(
    rule: &'a RuleDefinition,
    ids: &mut BTreeSet<&'a str>,
) -> Result<(), RuleSetError> {
    let template = &rule.recommendation_template;
    let expected_kind = if TIME_SHARE_RULE_IDS.contains(&rule.rule_id.as_str()) {
        RecommendationKind::OptimizationTarget
    } else {
        RecommendationKind::Investigation
    };
    let well_formed_id = template
        .recommendation_id
        .strip_prefix("rec.")
        .and_then(|rest| rest.split_once("@v"))
        .is_some_and(|(name, version)| {
            name.split('.').count() == 2
                && name.split('.').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                })
                && !version.is_empty()
                && version.bytes().all(|byte| byte.is_ascii_digit())
        });
    let blank = |value: &str| value.trim().is_empty();
    if !well_formed_id
        || !ids.insert(template.recommendation_id.as_str())
        || template.kind != expected_kind
        || blank(&template.title)
        || blank(&template.next_step)
        || template.steps.is_empty()
        || template.steps.iter().any(|step| {
            blank(&step.title)
                || blank(&step.detail)
                || step.evidence_metric_ids.iter().any(|id| blank(id))
                || step
                    .evidence_metric_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != step.evidence_metric_ids.len()
        })
        || template.expected_changes.is_empty()
        || template
            .expected_changes
            .iter()
            .any(|change| blank(&change.metric_id))
        || template.limitations.is_empty()
        || template
            .limitations
            .iter()
            .any(|limitation| blank(limitation))
    {
        return Err(RuleSetError::RuleContractMismatch("recommendation"));
    }
    Ok(())
}

fn rule<'a>(
    manifest: &'a RuleSetManifest,
    rule_id: &str,
) -> Result<&'a RuleDefinition, RuleSetError> {
    manifest
        .rules
        .iter()
        .find(|rule| rule.rule_id == rule_id)
        .ok_or(RuleSetError::RuleContractMismatch("missing_rule"))
}
fn validate_time_rule(
    manifest: &RuleSetManifest,
    id: &str,
    subject: &str,
    inputs: &[&str],
) -> Result<(), RuleSetError> {
    let rule = rule(manifest, id)?;
    let (applicability, risk, expected_direction, first_validation_step) = match id {
        SCAN_TIME_SHARE_RULE_ID => (
            "SCAN plan nodes (*_SCAN); detected 3.5.14 level_1 final merged profile; exact execution-summary node time from typed inputs and non-zero denominator",
            "scan time share needs manual investigation; it does not prove pruning or I/O root cause",
            "reduce selected scan time share after one controlled manual change",
            "inspect predicates, partition pruning, index, rows, bytes and I/O context",
        ),
        JOIN_TIME_SHARE_RULE_ID => (
            "JOIN allowlist plan nodes; detected 3.5.14 level_1 final merged profile; exact execution-summary node time from typed inputs and non-zero denominator",
            "join time share needs manual investigation; it does not prove a join root cause",
            "reduce selected join time share after one controlled manual change",
            "inspect join conditions, build/probe, distribution, rows, memory and spill context",
        ),
        AGGREGATE_TIME_SHARE_RULE_ID => (
            "AGGREGATE allowlist plan nodes; detected 3.5.14 level_1 final merged profile; exact execution-summary node time from typed inputs and non-zero denominator",
            "aggregate time share needs manual investigation; it does not prove an aggregation root cause",
            "reduce selected aggregate time share after one controlled manual change",
            "inspect grouping, stages, input/output rows, memory and spill context",
        ),
        NODE_TIME_SHARE_RULE_ID => (
            "other plan nodes (not SCAN/JOIN/AGGREGATE/EXCHANGE); detected 3.5.14 level_1 final merged profile; exact execution-summary node time from typed inputs and non-zero denominator",
            "plan node time share needs manual investigation; it does not prove a root cause",
            "reduce selected plan node time share after one controlled manual change",
            "inspect the operator, its input/output rows and upstream/downstream context",
        ),
        EXCHANGE_NETWORK_TIME_SHARE_RULE_ID => (
            "EXCHANGE plan nodes (*EXCHANGE*); detected 3.5.14 level_1 final merged profile; exact execution-summary node time from typed inputs and non-zero denominator",
            "exchange time share needs manual investigation; it does not prove a network root cause",
            "reduce selected exchange time share after one controlled manual change",
            "inspect distribution, transferred rows/bytes, upstream/downstream and network context",
        ),
        _ => return Err(RuleSetError::RuleContractMismatch("time_rule_id")),
    };
    let policy = FindingPolicy {
        bucket_severity: Some(BucketSeverity {
            attention_15: "moderate".into(),
            attention_30: "high".into(),
        }),
        confidence: ConfidencePolicy {
            bounded_definite: FindingConfidence::Medium,
            exact_definite: FindingConfidence::High,
            suspected: FindingConfidence::Low,
        },
        severity: None,
    };
    if rule.category != "operator_time_distribution"
        || rule.evaluator_id != "sr3514.operator_time_share"
        || rule.typed_inputs != inputs
        || rule.subject_kinds != ["operator_group"]
        || rule.finding_policy != policy
        || rule.formula_source
            != "StarRocks 3.5.14 ExplainAnalyzer NodeInfo time and computeTimeUsage, read from execution summary sr3514.explain_analyzer.node_info.v1"
        || rule.applicability != applicability
        || rule.confidence_policy
            != "exact complete; text precision interval, partial graph or verified-derived source stage is partial; threshold intersections are suspected"
        || rule.limitations
            != [
                "operator active time is not query wall time",
                "parallel and asynchronous operators may overlap",
            ]
        || rule.recommendation_template.risk != risk
        || rule.recommendation_template.expected_direction != expected_direction
        || rule.recommendation_template.validation_steps
            != [
                first_validation_step,
                "re-profile after one controlled manual change",
            ]
        || rule.threshold_source
            != "StarRocks 3.5.14 ExplainAnalyzer display thresholds; strict >15% and >30%"
        || rule.thresholds != [">15%", ">30%"]
        || rule.absolute_floor
            != Some(AbsoluteFloor {
                input: "subject_operator_time_ns".into(),
                suppress_below_ns: TIME_SHARE_ABSOLUTE_FLOOR_NS,
                source: "product_decision_ruleset_0.1.5".into(),
            })
        || !rule.applicability.contains(subject)
    {
        return Err(RuleSetError::RuleContractMismatch("time_rule"));
    }
    Ok(())
}
fn validate_spill_rule(manifest: &RuleSetManifest) -> Result<(), RuleSetError> {
    let rule = rule(manifest, SPILL_OBSERVED_RULE_ID)?;
    let policy = FindingPolicy {
        bucket_severity: None,
        confidence: ConfidencePolicy {
            bounded_definite: FindingConfidence::Medium,
            exact_definite: FindingConfidence::High,
            suspected: FindingConfidence::Low,
        },
        severity: Some("moderate".into()),
    };
    if rule.category != "spill_observation"
        || rule.evaluator_id != "sr3514.spill_observed"
        || rule.typed_inputs != ["QuerySpillBytes"]
        || rule.subject_kinds != ["query", "operator_group"]
        || rule.finding_policy != policy
        || rule.applicability != "registered QuerySpillBytes observation"
        || rule.confidence_policy != "interval containing zero is suspected"
        || rule.formula_source != "registered QuerySpillBytes observation"
        || rule.threshold_source != "product decision: exact >0 or conservative lower bound >0"
        || rule.thresholds != [">0 exact or bounds.lower >0"]
        || rule.absolute_floor.is_some()
        || rule.limitations != ["spill presence alone does not prove the root cause"]
        || rule.recommendation_template.risk != "spill presence alone does not prove the root cause"
        || rule.recommendation_template.expected_direction
            != "reduce spill observations after one controlled manual change"
        || rule.recommendation_template.validation_steps
            != [
                "inspect memory and operator context",
                "re-profile after one controlled manual change",
            ]
    {
        return Err(RuleSetError::RuleContractMismatch("spill_rule"));
    }
    Ok(())
}
fn validate_row_rule(manifest: &RuleSetManifest) -> Result<(), RuleSetError> {
    let rule = rule(manifest, ROW_FLOW_AMPLIFICATION_RULE_ID)?;
    let policy = FindingPolicy {
        bucket_severity: None,
        confidence: ConfidencePolicy {
            bounded_definite: FindingConfidence::Medium,
            exact_definite: FindingConfidence::Medium,
            suspected: FindingConfidence::Low,
        },
        severity: Some("moderate".into()),
    };
    if rule.category != "runtime_row_flow_amplification"
        || rule.evaluator_id != "sr3514.row_flow_amplification"
        || rule.typed_inputs != ["OutputRows"]
        || rule.subject_kinds != ["operator_group"]
        || rule.finding_policy != policy
        || rule.applicability
            != "JOIN allowlist plan nodes; complete graph with exact explicit official Topology PlanChild parent_to_child closure; exact execution-summary OutputRows of the node and every direct child"
        || rule.confidence_policy != "only exact complete typed evidence is eligible"
        || rule.formula_source
            != "registered StarRocks 3.5.14 OutputRows recipes, read per plan node from execution summary sr3514.explain_analyzer.node_info.v1"
        || rule.thresholds != [">10"]
        || rule.absolute_floor.is_some()
        || rule.threshold_source
            != "product_decision_v0.1: strict parent_output_rows / sum(direct_child_output_rows) > 10"
        || rule.limitations
            != ["row amplification is an investigation signal and does not prove a join root cause"]
        || rule.recommendation_template.risk
            != "observed row amplification does not prove bad SQL or statistics"
        || rule.recommendation_template.expected_direction
            != "reduce observed row amplification after one controlled manual change"
        || rule.recommendation_template.validation_steps
            != [
                "inspect join condition, cardinality and distribution",
                "re-profile after one controlled manual change",
            ]
    {
        return Err(RuleSetError::RuleContractMismatch("row_rule"));
    }
    Ok(())
}
/// (rule id, evaluator id, typed inputs, thresholds) of the ruleset 0.2.0
/// signal rules. Typed inputs are registered counters; their MIN/MAX roles and
/// categorical operator facts (PartType, DistributionMode) are stated in the
/// applicability text. Their thresholds and floors are product decisions.
type SignalRuleShape = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);
const SIGNAL_RULES: [SignalRuleShape; 4] = [
    (
        OPERATOR_TIME_SKEW_RULE_ID,
        "sr3514.operator_time_skew",
        &["OperatorTotalTime"],
        &[">2 max/mean", ">=100ms max operator time"],
    ),
    (
        JOIN_PROBE_FANOUT_RULE_ID,
        "sr3514.join_probe_fanout",
        &["PushRowNum", "PullRowNum"],
        &[">10 probe output/input", ">=100000 probe output rows"],
    ),
    (
        BROADCAST_EXCHANGE_BYTES_RULE_ID,
        "sr3514.broadcast_exchange_bytes",
        &["BytesSent"],
        &[">100MiB BytesSent"],
    ),
    (
        STREAMING_PREAGG_INEFFECTIVE_RULE_ID,
        "sr3514.streaming_preagg_ineffective",
        &["PushRowNum", "PullRowNum"],
        &[
            ">=90% streaming output/input",
            ">=1000000 streaming input rows",
        ],
    ),
];

/// Signal rules: a fixed contract shape; the product-decision thresholds must
/// be labelled as such and never borrow an official source.
fn validate_signal_rule(
    manifest: &RuleSetManifest,
    id: &str,
    evaluator: &str,
    inputs: &[&str],
    thresholds: &[&str],
) -> Result<(), RuleSetError> {
    let rule = rule(manifest, id)?;
    let policy = FindingPolicy {
        bucket_severity: None,
        confidence: ConfidencePolicy {
            bounded_definite: FindingConfidence::Medium,
            exact_definite: FindingConfidence::Medium,
            suspected: FindingConfidence::Low,
        },
        severity: Some("moderate".into()),
    };
    if rule.evaluator_id != evaluator
        || rule.typed_inputs != inputs
        || rule.thresholds != thresholds
        || rule.subject_kinds != ["operator_group"]
        || rule.finding_policy != policy
        || rule.absolute_floor.is_some()
        || !rule
            .threshold_source
            .starts_with("product_decision_ruleset_0.2.0:")
        || !rule.formula_source.contains("StarRocks 3.5.14")
        || rule
            .recommendation_template
            .validation_steps
            .last()
            .map(String::as_str)
            != Some("re-profile after one controlled manual change")
    {
        return Err(RuleSetError::RuleContractMismatch("signal_rule"));
    }
    Ok(())
}

fn without(value: &Value, fields: &[&str]) -> Result<Value, RuleSetError> {
    let mut object = value.as_object().ok_or(RuleSetError::InvalidRoot)?.clone();
    for field in fields {
        object.remove(*field);
    }
    Ok(Value::Object(object))
}
fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_in_ruleset_is_runtime_loadable_and_links_all_cases_once() {
        let ruleset = load().expect("checked-in ruleset must validate");
        assert_eq!(ruleset.manifest().rules.len(), 11);
        assert_eq!(
            ruleset
                .manifest()
                .rules
                .iter()
                .map(|rule| rule.golden_case_ids.len())
                .sum::<usize>(),
            66
        );
    }
    #[test]
    fn loader_rejects_semantic_drift_without_a_hash_change() {
        let mut document: Value = serde_json::from_str(RULESET_JSON).expect("checked-in JSON");
        document["rules"][0]["thresholds"][0] = Value::String(">16%".into());
        assert!(matches!(
            validate_document(document),
            Err(RuleSetError::HashMismatch("semantic_manifest_hash"))
        ));
    }
    #[test]
    fn loader_rejects_hash_valid_recommendation_drift() {
        let mut document: Value = serde_json::from_str(RULESET_JSON).expect("checked-in JSON");
        document["rules"][4]["recommendation_template"]["risk"] =
            Value::String("semantic drift".into());
        let semantic = canonical_hash(
            &without(&document, &["manifest_hash", "semantic_manifest_hash"])
                .expect("semantic projection"),
        );
        document["semantic_manifest_hash"] = Value::String(semantic);
        let outer =
            canonical_hash(&without(&document, &["manifest_hash"]).expect("outer projection"));
        document["manifest_hash"] = Value::String(outer);
        assert!(matches!(
            validate_document(document),
            Err(RuleSetError::RuleContractMismatch("spill_rule"))
        ));
    }
    #[test]
    fn every_rule_carries_a_unique_structured_recommendation() {
        let ruleset = load().expect("checked-in ruleset");
        let ids = ruleset
            .manifest()
            .rules
            .iter()
            .map(|rule| rule.recommendation_template.recommendation_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 11);
        for rule in &ruleset.manifest().rules {
            let template = &rule.recommendation_template;
            assert!(!template.steps.is_empty(), "{}", rule.rule_id);
            assert!(!template.expected_changes.is_empty(), "{}", rule.rule_id);
            assert_eq!(
                template.kind == RecommendationKind::OptimizationTarget,
                TIME_SHARE_RULE_IDS.contains(&rule.rule_id.as_str()),
                "{}",
                rule.rule_id
            );
        }
    }
    #[test]
    fn loader_rejects_hash_valid_recommendation_kind_drift() {
        let mut document: Value = serde_json::from_str(RULESET_JSON).expect("checked-in JSON");
        let index = document["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .position(|rule| rule["rule_id"] == SCAN_TIME_SHARE_RULE_ID)
            .expect("scan rule");
        document["rules"][index]["recommendation_template"]["kind"] =
            Value::String("investigation".into());
        let semantic = canonical_hash(
            &without(&document, &["manifest_hash", "semantic_manifest_hash"])
                .expect("semantic projection"),
        );
        document["semantic_manifest_hash"] = Value::String(semantic);
        let outer =
            canonical_hash(&without(&document, &["manifest_hash"]).expect("outer projection"));
        document["manifest_hash"] = Value::String(outer);
        assert!(matches!(
            validate_document(document),
            Err(RuleSetError::RuleContractMismatch("recommendation"))
        ));
    }
    #[test]
    fn pipeline_skew_is_not_an_active_rule() {
        let ruleset = load().expect("checked-in ruleset");
        assert!(ruleset.rule("SR3514.PIPELINE_DRIVER_TIME_SKEW").is_none());
    }
}
