//! Frozen StarRocks 3.5.14 release semantics. This crate never reads an
//! external checkout or the network at runtime.

use doctor_contracts::{SelectedVersionPack, canonical_hash};

pub mod deltas;
use serde::Deserialize;
use serde_json::{Map, Value};

pub const PACK_ID: &str = "starrocks-3.5.14";
pub const UPSTREAM_RELEASE: &str = "3.5.14";
pub const UPSTREAM_COMMIT: &str = "23a56ec517db763ea9bc7ee50b19357acdee0303";
pub const PRODUCT_PACK_VERSION: &str = "0.1.10";
pub const METRIC_REGISTRY_VERSION: &str = "0.3.2";
pub const SEMANTIC_MANIFEST_HASH: &str =
    "bdbb05eeeded2e75898bac09a80c1d8ca6080440a2a5926e1c4442490d38adc2";
pub const OUTER_MANIFEST_HASH: &str =
    "6afe6fc2c2b4e5388ba0479382c2cd42d7fa6987f8c15f8ce2edfb79293b743c";
pub const RECIPE_MANIFEST_HASH: &str =
    "d40c05871af023dd2b6cf88fa3a1a9ad122ac2fd6a527eea523d87f05f5b3415";
pub const GOLDEN_MANIFEST_HASH: &str =
    "aa0dc153d931324354fb3cd57ea60b5050342f3a80924b820892814282eb73ab";
const PACK_JSON: &str = include_str!("../version-pack.json");
const GOLDEN_MANIFEST_JSON: &str =
    include_str!("../../../../../fixtures/manifests/sr3514-contract-fixtures.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct VersionPackManifest {
    pub pack_id: String,
    pub product_pack_version: String,
    pub upstream_release: String,
    pub upstream_commit: String,
    pub semantic_manifest_hash: String,
    pub manifest_hash: String,
    pub golden_manifest_hash: String,
    pub source_evidence: Vec<SourceEvidence>,
    pub detection_rules: Vec<DetectionRule>,
    pub metric_registry: MetricRegistryManifest,
    pub metric_recipe_manifest: MetricRecipeManifest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SourceEvidence {
    pub path: String,
    pub symbol: String,
    pub source_blob_sha256: String,
    pub observation: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct DetectionRule {
    pub id: String,
    pub evidence: String,
    pub result: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct MetricRegistryManifest {
    pub registry_id: String,
    pub version: String,
    pub metrics: Vec<MetricManifest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct MetricManifest {
    pub analysis_aggregation: String,
    pub canonical_unit: String,
    pub dimension: String,
    pub fidelity_policy: String,
    pub metric_id: String,
    pub ordered_source_contract: String,
    pub valid_scopes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct MetricRecipeManifest {
    pub manifest_hash: String,
    pub manifest_version: String,
    pub recipes: Vec<MetricRecipe>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct MetricRecipe {
    pub id: String,
    pub version: String,
    pub ordered_input_contract: String,
    pub typed_inputs: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StarRocks3514Pack {
    manifest: VersionPackManifest,
}

impl StarRocks3514Pack {
    pub fn manifest(&self) -> &VersionPackManifest {
        &self.manifest
    }

    /// Receipts use the semantic manifest hash, never the outer bookkeeping hash.
    pub fn selected_pack(&self) -> SelectedVersionPack {
        SelectedVersionPack {
            pack_id: self.manifest.pack_id.clone(),
            product_pack_version: self.manifest.product_pack_version.clone(),
            upstream_release: self.manifest.upstream_release.clone(),
            upstream_commit: self.manifest.upstream_commit.clone(),
            manifest_hash: self.manifest.semantic_manifest_hash.clone(),
            release_deltas: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VersionPackError {
    InvalidJson,
    InvalidRoot,
    MissingField(&'static str),
    InvalidHash(&'static str),
    HashMismatch(&'static str),
    IdentityMismatch(&'static str),
    MissingEvidence(&'static str),
    ForbiddenField(&'static str),
}

/// Loads and validates the checked-in release manifest without filesystem or network access.
pub fn load() -> Result<StarRocks3514Pack, VersionPackError> {
    let document = serde_json::from_str(PACK_JSON).map_err(|_| VersionPackError::InvalidJson)?;
    validate_document(document)
}

fn validate_document(document: Value) -> Result<StarRocks3514Pack, VersionPackError> {
    let object = document.as_object().ok_or(VersionPackError::InvalidRoot)?;
    let manifest: VersionPackManifest =
        serde_json::from_value(document.clone()).map_err(|_| VersionPackError::InvalidJson)?;
    if manifest.pack_id != PACK_ID {
        return Err(VersionPackError::IdentityMismatch("pack_id"));
    }
    if manifest.product_pack_version != PRODUCT_PACK_VERSION {
        return Err(VersionPackError::IdentityMismatch("product_pack_version"));
    }
    if manifest.upstream_release != UPSTREAM_RELEASE {
        return Err(VersionPackError::IdentityMismatch("upstream_release"));
    }
    if manifest.upstream_commit != UPSTREAM_COMMIT {
        return Err(VersionPackError::IdentityMismatch("upstream_commit"));
    }
    if manifest.semantic_manifest_hash != SEMANTIC_MANIFEST_HASH {
        return Err(VersionPackError::IdentityMismatch("semantic_manifest_hash"));
    }
    if manifest.manifest_hash != OUTER_MANIFEST_HASH {
        return Err(VersionPackError::IdentityMismatch("manifest_hash"));
    }
    if manifest.golden_manifest_hash != GOLDEN_MANIFEST_HASH {
        return Err(VersionPackError::IdentityMismatch("golden_manifest_hash"));
    }
    if manifest.metric_recipe_manifest.manifest_hash != RECIPE_MANIFEST_HASH {
        return Err(VersionPackError::IdentityMismatch(
            "metric_recipe_manifest.manifest_hash",
        ));
    }
    if ["rules", "severity", "recommendation"]
        .into_iter()
        .any(|field| object.contains_key(field))
    {
        return Err(VersionPackError::ForbiddenField("product_rule_material"));
    }
    for (name, value) in [
        (
            "semantic_manifest_hash",
            manifest.semantic_manifest_hash.as_str(),
        ),
        ("manifest_hash", manifest.manifest_hash.as_str()),
        (
            "golden_manifest_hash",
            manifest.golden_manifest_hash.as_str(),
        ),
        (
            "metric_recipe_manifest.manifest_hash",
            manifest.metric_recipe_manifest.manifest_hash.as_str(),
        ),
    ] {
        if !is_sha256(value) {
            return Err(VersionPackError::InvalidHash(name));
        }
    }

    let recipe = object
        .get("metric_recipe_manifest")
        .ok_or(VersionPackError::MissingField("metric_recipe_manifest"))?;
    if canonical_hash(&without(recipe, &["manifest_hash"])?)
        != manifest.metric_recipe_manifest.manifest_hash
    {
        return Err(VersionPackError::HashMismatch("metric_recipe_manifest"));
    }
    if canonical_hash(&without(
        &document,
        &[
            "manifest_hash",
            "semantic_manifest_hash",
            "golden_manifest_path",
            "golden_manifest_hash",
            "golden_manifest_hash_scope",
        ],
    )?) != manifest.semantic_manifest_hash
    {
        return Err(VersionPackError::HashMismatch("semantic_manifest_hash"));
    }
    if canonical_hash(&without(&document, &["manifest_hash"])?) != manifest.manifest_hash {
        return Err(VersionPackError::HashMismatch("manifest_hash"));
    }
    let golden: Value =
        serde_json::from_str(GOLDEN_MANIFEST_JSON).map_err(|_| VersionPackError::InvalidJson)?;
    if canonical_hash(&without(&golden, &["manifest_hash"])?) != manifest.golden_manifest_hash {
        return Err(VersionPackError::HashMismatch("golden_manifest_hash"));
    }

    if manifest.source_evidence.len() != REQUIRED_EVIDENCE.len()
        || manifest.source_evidence.iter().any(|entry| {
            entry.path.is_empty()
                || entry.symbol.is_empty()
                || entry.observation.is_empty()
                || !is_sha256(&entry.source_blob_sha256)
        })
    {
        return Err(VersionPackError::MissingEvidence("source_evidence"));
    }
    for (path, symbol, sha) in REQUIRED_EVIDENCE {
        if !manifest.source_evidence.iter().any(|entry| {
            entry.path == path && entry.symbol == symbol && entry.source_blob_sha256 == sha
        }) {
            return Err(VersionPackError::MissingEvidence(symbol));
        }
    }
    if !manifest
        .detection_rules
        .iter()
        .any(|rule| rule.id == "reported-version-3.5.14" && rule.result == "exact_pack")
    {
        return Err(VersionPackError::MissingEvidence("reported-version-3.5.14"));
    }
    if !manifest
        .detection_rules
        .iter()
        .any(|rule| rule.id == "user-declared-3.5.14" && rule.result == "candidate_pack_parse_only")
    {
        return Err(VersionPackError::MissingEvidence("user-declared-3.5.14"));
    }
    if manifest.metric_recipe_manifest.manifest_version != "1.0"
        || manifest.metric_recipe_manifest.recipes.len() != REQUIRED_RECIPES.len()
        || REQUIRED_RECIPES.iter().any(|(id, inputs, conditions)| {
            !manifest
                .metric_recipe_manifest
                .recipes
                .iter()
                .any(|recipe| {
                    recipe.id == *id
                        && recipe.version == "1"
                        && recipe.ordered_input_contract == "preserve_source_ordinal"
                        && recipe
                            .typed_inputs
                            .iter()
                            .map(String::as_str)
                            .eq(inputs.iter().copied())
                        && recipe
                            .conditions
                            .iter()
                            .map(String::as_str)
                            .eq(conditions.iter().copied())
                })
        })
    {
        return Err(VersionPackError::MissingEvidence("output_row_recipes"));
    }
    if manifest.metric_registry.registry_id != "sr3514-registry"
        || manifest.metric_registry.version != METRIC_REGISTRY_VERSION
        || manifest.metric_registry.metrics.len() != REQUIRED_METRICS.len()
        || REQUIRED_METRICS.iter().any(|id| {
            !manifest
                .metric_registry
                .metrics
                .iter()
                .any(|metric| metric.metric_id == *id)
        })
    {
        return Err(VersionPackError::MissingEvidence("metric_registry"));
    }
    let output_rows = manifest
        .metric_registry
        .metrics
        .iter()
        .find(|metric| metric.metric_id == "OutputRows");
    if !output_rows.is_some_and(|metric| {
        metric.analysis_aggregation == "non_additive"
            && metric.canonical_unit == "item"
            && metric.dimension == "count"
            && metric.fidelity_policy
                == "exact only when a registered output-row recipe selects unambiguous exact inputs"
            && metric.ordered_source_contract == "registered 3.5.14 output-row recipe only"
            && metric
                .valid_scopes
                .iter()
                .map(String::as_str)
                .eq(["operator"])
    }) {
        return Err(VersionPackError::MissingEvidence("output_rows_registry"));
    }
    Ok(StarRocks3514Pack { manifest })
}

fn without(value: &Value, fields: &[&str]) -> Result<Value, VersionPackError> {
    let mut object: Map<String, Value> = value
        .as_object()
        .ok_or(VersionPackError::InvalidRoot)?
        .clone();
    for field in fields {
        object.remove(*field);
    }
    Ok(Value::Object(object))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

const REQUIRED_EVIDENCE: [(&str, &str, &str); 12] = [
    (
        "gensrc/thrift/RuntimeProfile.thrift",
        "TCounterStrategy",
        "e44fbd2efa23bb223bb6db874bf2b695f4e3c536224d88773d0e4b0d41060312",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/common/util/RuntimeProfile.java",
        "printCounter",
        "3beeb55528db37abbb5d0c553a74ba4bb32cdc3ff9dbf9a91f7f5f6a1858e605",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/common/util/RuntimeProfile.java",
        "getMaxCounter/removeRedundantMinMaxMetrics",
        "3beeb55528db37abbb5d0c553a74ba4bb32cdc3ff9dbf9a91f7f5f6a1858e605",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/common/util/DebugUtil.java",
        "getByteUint",
        "d90793dbeee9dcc420105637b044a77203984fd61968e9eabe0504e0d733b219",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/sql/ExplainAnalyzer.java",
        "computeTimeUsage",
        "669990b393d5579ef0d09c2e7ed82e3763593b7fce6e9ec772d56a383b9a9928",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/sql/ExplainAnalyzer.java",
        "searchOutputRows/sumUpMetric",
        "669990b393d5579ef0d09c2e7ed82e3763593b7fce6e9ec772d56a383b9a9928",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/common/util/ProfilingExecPlan.java",
        "ProfilingExecPlan",
        "180eef9bbe602addda427c4dbf9a48531786a834d3c37fd86daf4f34fcace35d",
    ),
    (
        "fe/fe-core/src/main/java/com/starrocks/qe/scheduler/QueryRuntimeProfile.java",
        "buildQueryProfile/mergePipelineProfile",
        "0f936d010335bc0b37d0791182830906f2c21158a3027d7b8d9d97ab02583b72",
    ),
    (
        "be/src/exec/pipeline/pipeline_driver.cpp",
        "PipelineDriver::prepare/process",
        "90ad997eabd42def1784ef91c2622c56ffaef127585e75ee0b778be16b205a32",
    ),
    (
        "be/src/exec/pipeline/pipeline_driver_poller.cpp",
        "PipelineDriverPoller::_poll",
        "19183817640da312602858f1d25bd11f04c3317c7a06e067be9488291411d396",
    ),
    (
        "be/src/util/runtime_profile.h",
        "RuntimeProfile::Counter::create_strategy",
        "473241ab4b2eb19f2c5e6e7f3f5c0e220a0b84825aaeb9cdae8e4c1509456074",
    ),
    (
        "be/src/util/runtime_profile.cpp",
        "RuntimeProfile::merge",
        "8012a5dd358db2cdbd78cc0bf5a5846b28f5d908116a647feb8a475535e65e82",
    ),
];

const REQUIRED_RECIPES: [(&str, &[&str], &[&str]); 4] = [
    ("sr3514.output_rows.sink", &["PushRowNum"], &[]),
    ("sr3514.output_rows.union", &["PullRowNum"], &[]),
    ("sr3514.output_rows.general", &["PullRowNum"], &[]),
    (
        "sr3514.output_rows.broadcast_exchange",
        &["PullRowNum", "InstanceNum"],
        &[
            "native UniqueMetrics.PartType is exactly UNPARTITIONED",
            "fragment InstanceNum is exact and > 0",
        ],
    ),
];

const REQUIRED_METRICS: [&str; 35] = [
    "QueryExecutionWallTime",
    "QueryPlanningTime",
    "QueryTotalTime",
    "QueryCumulativeCpuTime",
    "QueryCumulativeOperatorTime",
    "QueryCumulativeScanTime",
    "QueryCumulativeNetworkTime",
    "QueryPeakMemoryUsagePerNode",
    "QueryAllocatedMemoryUsage",
    "QueryDeallocatedMemoryUsage",
    "QuerySumMemoryUsage",
    "QueryPeakScheduleTime",
    "ResultDeliverTime",
    "FrontendProfileMergeTime",
    "QuerySpillBytes",
    "Active",
    "ActiveTime",
    "DriverTotalTime",
    "DriverPrepareTime",
    "PendingTime",
    "ScheduleTime",
    "ScheduleCount",
    "OperatorTotalTime",
    "NetworkTime",
    "ScanTime",
    "PushRowNum",
    "PullRowNum",
    "PushChunkNum",
    "PullChunkNum",
    "PushTotalTime",
    "PullTotalTime",
    "OutputChunkBytes",
    "BytesSent",
    "InstanceNum",
    "OutputRows",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_pack_is_runtime_loadable_and_uses_semantic_receipt_identity() {
        let pack = load().expect("checked-in manifest must validate");
        assert_eq!(pack.manifest().upstream_commit, UPSTREAM_COMMIT);
        assert_eq!(
            pack.selected_pack().manifest_hash,
            pack.manifest().semantic_manifest_hash
        );
        assert_eq!(
            pack.manifest().semantic_manifest_hash,
            SEMANTIC_MANIFEST_HASH
        );
    }

    #[test]
    fn loader_rejects_semantic_manifest_drift_even_when_identity_stays_the_same() {
        let mut document: Value = serde_json::from_str(PACK_JSON).expect("checked-in JSON");
        document["known_unknowns"][0] = Value::String("tampered".into());
        assert!(matches!(
            validate_document(document),
            Err(VersionPackError::HashMismatch("semantic_manifest_hash"))
        ));
    }
}
