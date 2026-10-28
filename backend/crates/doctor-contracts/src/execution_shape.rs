//! Fragment / instance counts of one stored Profile, as printed by the FE.
//!
//! StarRocks 3.5.14 `QueryRuntimeProfile.buildQueryProfile` writes one
//! `InstanceNum` counter per merged fragment (the number of instance profiles
//! it merged). Compare uses these counts to decide whether query counters that
//! the FE sums over instance reports (e.g. `QueryCumulativeCpuTime`) are
//! comparable at all. The browser never derives them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Recipe identity. A new StarRocks release registers its own recipe.
pub const EXECUTION_SHAPE_RECIPE_ID: &str = "sr3514.query_runtime_profile.instance_num.v1";

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExecutionShapeState {
    /// Fragment count and every fragment's `InstanceNum` are exact.
    Complete,
    /// Fragments are known but at least one `InstanceNum` is missing or invalid.
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct FragmentInstanceCount {
    pub fragment_id: String,
    /// FE `InstanceNum`; `None` when missing, ambiguous or invalid (never 0).
    pub instance_num: Option<u32>,
    pub reasons: Vec<String>,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct ExecutionShape {
    pub state: ExecutionShapeState,
    pub recipe_id: String,
    /// Distinct `Fragment N:` sections; `None` when they can not be counted.
    pub fragment_count: Option<u32>,
    /// Sum of every fragment's `InstanceNum`; `None` unless all are exact.
    pub instance_count: Option<u32>,
    pub fragments: Vec<FragmentInstanceCount>,
    pub reasons: Vec<String>,
}

impl ExecutionShape {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            state: ExecutionShapeState::Unavailable,
            recipe_id: EXECUTION_SHAPE_RECIPE_ID.to_owned(),
            fragment_count: None,
            instance_count: None,
            fragments: vec![],
            reasons: vec![reason.to_owned()],
        }
    }
}

impl Default for ExecutionShape {
    /// Retained reports assembled before the shape existed decode as
    /// explicitly unavailable, never as "zero instances".
    fn default() -> Self {
        Self::unavailable("retained_report_without_execution_shape")
    }
}
