//! Text Profiles of 3.1.0–3.1.2 (tasks 32, 34): an operator with a negative
//! plan node id is printed without `(plan_node_id=…)`. The spelling is accepted
//! only for the audited releases whose delta says so. 2.5 and 3.0 are not
//! supported, so none of their layout (`Execution Profile <query id>:`, the
//! MySQL state type in `Query State`) is read.
use doctor_contracts::{QueryState, RequestedSourceKind, RuntimeInventoryFormatterShape};
use doctor_core::{
    AdapterContext, AdapterLimits, AdapterOutput, InputEnvelope, adapt_input, v01_analysis_receipt,
};

fn adapt(text: &str) -> AdapterOutput {
    adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, text.as_bytes()),
        &AdapterLimits::default(),
        &AdapterContext::new(v01_analysis_receipt()).unwrap(),
    )
    .unwrap()
}

fn codes(output: &AdapterOutput) -> Vec<&str> {
    output
        .ir
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect()
}

/// A minimal profile in the given release's layout.
fn synthetic(version: &str, execution: &str, state: &str, operator: &str) -> String {
    format!(
        "Query:\n  Summary:\n     - Query ID: 00000000-0000-0000-0000-000000000032\n     - Query Type: Query\n     - Query State: {state}\n     - StarRocks Version: {version}-0000000\n  {execution}\n    Fragment 0:\n       - BackendAddresses: 127.0.0.1:9060\n      Pipeline (id=0):\n        {operator}:\n          CommonMetrics:\n             - OperatorTotalTime: 1ms\n          UniqueMetrics:\n        OLAP_SCAN (plan_node_id=0):\n          CommonMetrics:\n             - OperatorTotalTime: 2ms\n          UniqueMetrics:\n"
    )
}

const LEGACY_EXECUTION: &str = "Execution Profile 00000000-0000-0000-0000-000000000032:";

#[test]
fn legacy_spellings_are_rejected_from_3_1_3_on() {
    for version in ["3.1.17", "3.5.14"] {
        let bare = adapt(&synthetic(version, "Execution:", "Finished", "RESULT_SINK"));
        assert!(
            codes(&bare).contains(&"runtime_operator_header_invalid"),
            "{version}"
        );

        let state = adapt(&synthetic(
            version,
            "Execution:",
            "EOF",
            "RESULT_SINK (plan_node_id=-1)",
        ));
        assert!(
            codes(&state).contains(&"profile_context_invalid"),
            "{version}"
        );
        assert_eq!(state.ir.profile_context.query_state, QueryState::Unknown);

        // Merged fragments list InstanceIds; with them the baseline layout
        // verifies, and a pre-3.1 execution header alone breaks it.
        let with_instance_ids = |execution: &str| {
            adapt(
                &synthetic(
                    version,
                    execution,
                    "Finished",
                    "RESULT_SINK (plan_node_id=-1)",
                )
                .replace(
                    "- BackendAddresses: 127.0.0.1:9060\n",
                    "- BackendAddresses: 127.0.0.1:9060\n       - InstanceIds: 00000000-0000-0000-0000-000000000033\n",
                ),
            )
            .ir
            .runtime_formatter_shape
        };
        assert_eq!(
            with_instance_ids("Execution:"),
            RuntimeInventoryFormatterShape::VerifiedPrettyPrintV3514,
            "{version}"
        );
        assert_eq!(
            with_instance_ids(LEGACY_EXECUTION),
            RuntimeInventoryFormatterShape::Unknown,
            "{version}"
        );
    }
}

#[test]
fn the_bare_operator_header_ends_at_3_1_3() {
    let accepted = adapt(&synthetic("3.1.2", "Execution:", "Finished", "RESULT_SINK"));
    assert!(!codes(&accepted).contains(&"runtime_operator_header_invalid"));
    assert_eq!(accepted.ir.physical_operator_memberships.len(), 2);
    // The bare sink keeps no plan node id: the printed name says nothing about it.
    assert!(
        accepted
            .ir
            .physical_operator_memberships
            .iter()
            .any(|member| member.canonical_type == "RESULT_SINK" && member.plan_node_id.is_none())
    );

    let rejected = adapt(&synthetic("3.1.3", "Execution:", "Finished", "RESULT_SINK"));
    assert!(codes(&rejected).contains(&"runtime_operator_header_invalid"));
}

#[test]
fn pseudo_plan_node_ids_name_no_plan_node() {
    let output = adapt(&synthetic(
        "3.1.2",
        "Execution:",
        "Finished",
        "LOCAL_EXCHANGE_SOURCE (pseudo_plan_node_id=-101)",
    ));
    assert!(!codes(&output).contains(&"runtime_operator_header_invalid"));
    let member = output
        .ir
        .physical_operator_memberships
        .iter()
        .find(|member| member.canonical_type == "LOCAL_EXCHANGE_SOURCE")
        .expect("pseudo operator is read");
    assert_eq!(member.plan_node_id, None);

    // A pseudo id above the official bound (-100) is not a pseudo id.
    for header in [
        "LOCAL_EXCHANGE_SOURCE (pseudo_plan_node_id=-99)",
        "LOCAL_EXCHANGE_SOURCE (pseudo_plan_node_id=101)",
    ] {
        let output = adapt(&synthetic("3.1.2", "Execution:", "Finished", header));
        assert!(
            codes(&output).contains(&"runtime_operator_header_invalid"),
            "{header}"
        );
    }
}

#[test]
fn a_bare_header_is_only_a_direct_pipeline_child() {
    // Nested one level too deep: not an operator header, so it is quarantined.
    let text = synthetic("3.1.2", "Execution:", "Finished", "RESULT_SINK")
        .replace("        RESULT_SINK:\n", "          RESULT_SINK:\n");
    let output = adapt(&text);
    assert!(
        output
            .ir
            .physical_operator_memberships
            .iter()
            .all(|member| member.canonical_type != "RESULT_SINK")
    );
}

#[test]
fn no_2_5_or_3_0_layout_is_read() {
    for version in ["2.5.21", "3.0.9"] {
        let output = adapt(&synthetic(version, LEGACY_EXECUTION, "EOF", "RESULT_SINK"));
        let codes = codes(&output);
        assert!(codes.contains(&"profile_context_invalid"), "{version}");
        assert_eq!(
            output.ir.profile_context.query_state,
            QueryState::Unknown,
            "{version}"
        );
        assert!(
            codes.contains(&"runtime_operator_header_invalid"),
            "{version}"
        );
        assert!(
            output
                .ir
                .physical_operator_memberships
                .iter()
                .all(|member| member.canonical_type != "RESULT_SINK"),
            "{version}"
        );
        assert_eq!(
            output.ir.runtime_formatter_shape,
            RuntimeInventoryFormatterShape::Unknown,
            "{version}"
        );
    }
}
