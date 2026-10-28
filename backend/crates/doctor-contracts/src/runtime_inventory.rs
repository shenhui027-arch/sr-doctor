//! Runtime operator facts shared by the adapter IR. The public report carries
//! them only through `ExecutionSummary`; no separate runtime inventory wire.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RuntimeInventoryFormatterShape {
    VerifiedPrettyPrintV3514,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RuntimeInventoryAcquisitionShape {
    GetQueryProfile,
    OfflineImport,
    Unknown,
}

/// Official ExplainAnalyzer role: `IsSubordinate` marks subordinate; every
/// other operator profile is native.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RuntimeSourceRole {
    Native,
    Subordinate,
    Unknown,
}
