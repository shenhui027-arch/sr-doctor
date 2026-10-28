use std::collections::BTreeSet;

use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    AdapterContext, AdapterLimits, AdapterOutput, InputEnvelope, PhysicalOperatorMembership,
    adapt_input, v01_analysis_receipt,
};

const PIPELINE_DRIVER_OCCURRENCES_PROFILE: &str = include_str!(
    "../../../../fixtures/runtime-profiles/starrocks-3.5.14/pipeline-driver-occurrences.profile.txt"
);
const LIVE_RUNTIME_PROFILE_TEXT: &str = include_str!(
    "../../../../fixtures/runtime/3.5.14/docker_information_schema_columns.profile.txt"
);

const TWO_PIPELINES_SAME_PLAN_NODE: &str = r#"Query:
  Summary:
    - Query ID: 00000000-0000-0000-0000-000000000071
    - Query Type: Query
    - Query State: Finished
    - StarRocks Version: 3.5.14-23a56ec
  Execution:
    Fragment 0:
      Pipeline (id=0):
        PROJECT (plan_node_id=7):
          CommonMetrics:
            - OperatorTotalTime: 1us
      Pipeline (id=1):
        PROJECT (plan_node_id=7):
          CommonMetrics:
            - OperatorTotalTime: 2us
"#;

const SAME_PIPELINE_ID_ACROSS_FRAGMENTS: &str = r#"Query:
  Summary:
    - Query ID: 00000000-0000-0000-0000-000000000072
    - Query Type: Query
    - Query State: Finished
    - StarRocks Version: 3.5.14-23a56ec
  Execution:
    Fragment 0:
      Pipeline (id=0):
        OLAP_SCAN (plan_node_id=9):
          CommonMetrics:
            - OperatorTotalTime: 1us
    Fragment 1:
      Pipeline (id=0):
        OLAP_SCAN (plan_node_id=9):
          CommonMetrics:
            - OperatorTotalTime: 2us
"#;

const EMPTY_CONTAINER_IDS: &str = r#"Query:
  Summary:
    - Query ID: 00000000-0000-0000-0000-000000000073
    - Query Type: Query
    - Query State: Finished
    - StarRocks Version: 3.5.14-23a56ec
  Execution:
    Fragment 0:
      Pipeline (id=):
        PROJECT (plan_node_id=1):
          CommonMetrics:
            - OperatorTotalTime: 1us
    Fragment :
      Pipeline (id=0):
        PROJECT (plan_node_id=2):
          CommonMetrics:
            - OperatorTotalTime: 2us
"#;

const JSON_SAME_PIPELINE_ID_ACROSS_FRAGMENTS: &str = r#"{
  "Query": {
    "Summary": {
      "Query ID": "00000000-0000-0000-0000-000000000074",
      "Query Type": "Query",
      "Query State": "Finished",
      "StarRocks Version": "3.5.14-23a56ec"
    },
    "Execution": {
      "Fragment 0": {
        "Pipeline (id=0)": {
          "PROJECT (plan_node_id=7)": {
            "CommonMetrics": { "OperatorTotalTime": "1us" }
          }
        }
      },
      "Fragment 1": {
        "Pipeline (id=0)": {
          "PROJECT (plan_node_id=7)": {
            "CommonMetrics": { "OperatorTotalTime": "2us" }
          }
        }
      }
    }
  }
}"#;

const JSON_EMPTY_CONTAINER_IDS_AFTER_VALID_CONTAINER: &str = r#"{
  "Query": {
    "Summary": {
      "Query ID": "00000000-0000-0000-0000-000000000075",
      "Query Type": "Query",
      "Query State": "Finished",
      "StarRocks Version": "3.5.14-23a56ec"
    },
    "Execution": {
      "Fragment 0": {
        "Pipeline (id=1)": {
          "PROJECT (plan_node_id=1)": {
            "CommonMetrics": { "OperatorTotalTime": "1us" }
          }
        },
        "Pipeline (id=)": {
          "PROJECT (plan_node_id=2)": {
            "CommonMetrics": { "OperatorTotalTime": "2us" }
          }
        }
      },
      "Fragment ": {
        "Pipeline (id=0)": {
          "PROJECT (plan_node_id=3)": {
            "CommonMetrics": { "OperatorTotalTime": "3us" }
          }
        }
      }
    }
  }
}"#;

fn adapt_text(content: &str) -> AdapterOutput {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, content.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap()
}

fn adapt_json(content: &str) -> AdapterOutput {
    let context = AdapterContext::new(v01_analysis_receipt()).unwrap();
    adapt_input(
        InputEnvelope::new(RequestedSourceKind::JsonProfile, content.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .unwrap()
}

fn members_for_plan_node<'a>(
    output: &'a AdapterOutput,
    plan_node_id: &str,
) -> Vec<&'a PhysicalOperatorMembership> {
    output
        .ir
        .physical_operator_memberships
        .iter()
        .filter(|membership| membership.plan_node_id.as_deref() == Some(plan_node_id))
        .collect()
}

fn grouped_physical_refs(output: &AdapterOutput) -> BTreeSet<&str> {
    output
        .ir
        .operator_groups
        .iter()
        .flat_map(|group| {
            group
                .native_operator_refs
                .iter()
                .chain(group.subordinate_operator_refs.iter())
        })
        .map(|(physical_operator_ref, _)| physical_operator_ref.as_str())
        .collect()
}

#[test]
fn docker_runtime_fixture_closes_every_physical_member_to_group_and_source() {
    let output = adapt_text(LIVE_RUNTIME_PROFILE_TEXT);
    let role_known_refs = output
        .ir
        .physical_operator_memberships
        .iter()
        // Ordered group refs carry the explicit subordinate marker only; natives
        // are membership-role facts (official ExplainAnalyzer).
        .filter(|membership| {
            membership.source_role == doctor_contracts::RuntimeSourceRole::Subordinate
        })
        .map(|membership| membership.physical_operator_ref.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(role_known_refs, grouped_physical_refs(&output));

    for membership in &output.ir.physical_operator_memberships {
        assert!(!membership.source_refs.is_empty());
        assert!(membership.source_refs.iter().all(|source_ref| {
            output
                .source_refs
                .iter()
                .any(|available| available.ref_id == *source_ref)
        }));
        assert!(output.ir.operator_groups.iter().any(|group| {
            group.plan_node_id == membership.plan_node_id
                && group.fragment_ref == membership.fragment_ref
                && membership
                    .source_refs
                    .iter()
                    .any(|source_ref| group.source_refs.contains(source_ref))
        }));
        if membership.source_role == doctor_contracts::RuntimeSourceRole::Unknown {
            assert!(
                !grouped_physical_refs(&output).contains(membership.physical_operator_ref.as_str())
            );
        }
    }
}

#[test]
fn real_driver_fixture_preserves_physical_members_for_one_pipeline_container() {
    let output = adapt_text(PIPELINE_DRIVER_OCCURRENCES_PROFILE);

    assert_eq!(output.ir.pipeline_containers.len(), 2);
    for container in &output.ir.pipeline_containers {
        assert!(!container.container_ref.is_empty());
        assert!(!container.fragment_ref.is_empty());
        assert!(!container.pipeline_ref.is_empty());
        assert!(!container.source_refs.is_empty());
        assert!(output.ir.operator_groups.iter().any(|group| {
            group.operator_group_id == container.container_ref && group.canonical_type == "PIPELINE"
        }));
    }

    let pipeline_zero = output
        .ir
        .pipeline_containers
        .iter()
        .find(|container| container.fragment_ref == "0" && container.pipeline_ref == "0")
        .unwrap();
    let scan_members = members_for_plan_node(&output, "0");
    assert_eq!(
        scan_members.len(),
        2,
        "two PipelineDriver physical rows survive"
    );
    assert!(scan_members.iter().all(|member| {
        member.container_ref.as_deref() == Some(pipeline_zero.container_ref.as_str())
            && member.reasons.is_empty()
            && !member.source_refs.is_empty()
    }));
    assert_eq!(
        scan_members
            .iter()
            .map(|member| member.physical_operator_ref.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        2,
        "driver rows must not merge into one physical member"
    );
    assert_eq!(output.ir.runtime_driver_occurrences.len(), 3);
    assert_eq!(
        scan_members
            .iter()
            .map(|member| member.driver_occurrence_ref.as_deref().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    assert!(scan_members.iter().all(|member| {
        member.emission_ordinal == Some(0)
            && member.source_role == doctor_contracts::RuntimeSourceRole::Native
            && member.plan_node_id.as_deref() == Some("0")
            && member.canonical_type == "OLAP_SCAN"
    }));
}

#[test]
fn same_plan_node_in_two_pipelines_keeps_distinct_container_memberships() {
    let output = adapt_text(TWO_PIPELINES_SAME_PLAN_NODE);
    let members = members_for_plan_node(&output, "7");

    assert_eq!(members.len(), 2);
    let container_refs = members
        .iter()
        .map(|member| member.container_ref.as_deref().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(container_refs.len(), 2);
    assert_eq!(
        output
            .ir
            .pipeline_containers
            .iter()
            .map(|container| (
                container.fragment_ref.as_str(),
                container.pipeline_ref.as_str(),
                container.occurrence_ordinal,
            ))
            .collect::<BTreeSet<_>>(),
        [("0", "0", 0), ("0", "1", 0)].into_iter().collect()
    );
}

#[test]
fn same_pipeline_id_in_different_fragments_is_not_merged() {
    let output = adapt_text(SAME_PIPELINE_ID_ACROSS_FRAGMENTS);
    let members = members_for_plan_node(&output, "9");

    assert_eq!(members.len(), 2);
    assert_eq!(
        output
            .ir
            .pipeline_containers
            .iter()
            .map(|container| (
                container.fragment_ref.as_str(),
                container.pipeline_ref.as_str(),
                container.occurrence_ordinal,
            ))
            .collect::<BTreeSet<_>>(),
        [("0", "0", 0), ("1", "0", 0)].into_iter().collect()
    );
    assert_eq!(
        members
            .iter()
            .map(|member| (
                member.fragment_ref.as_deref().unwrap(),
                member.container_ref.as_deref().unwrap(),
            ))
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
}

#[test]
fn json_same_pipeline_id_in_different_fragments_keeps_typed_memberships_distinct() {
    let output = adapt_json(JSON_SAME_PIPELINE_ID_ACROSS_FRAGMENTS);
    let members = members_for_plan_node(&output, "7");

    assert_eq!(members.len(), 2);
    assert_eq!(
        output
            .ir
            .pipeline_containers
            .iter()
            .map(|container| (
                container.fragment_ref.as_str(),
                container.pipeline_ref.as_str(),
                container.occurrence_ordinal,
            ))
            .collect::<BTreeSet<_>>(),
        [("0", "0", 0), ("1", "0", 0)].into_iter().collect()
    );
    assert_eq!(
        members
            .iter()
            .map(|member| member.container_ref.as_deref().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        2,
        "JSON physical members must bind their own Fragment/Pipeline container"
    );
}

#[test]
fn json_empty_container_ids_keep_known_members_without_inheriting_prior_scope() {
    let output = adapt_json(JSON_EMPTY_CONTAINER_IDS_AFTER_VALID_CONTAINER);
    assert_eq!(output.ir.pipeline_containers.len(), 1);
    let valid = &output.ir.pipeline_containers[0];
    assert_eq!(
        (
            valid.fragment_ref.as_str(),
            valid.pipeline_ref.as_str(),
            valid.occurrence_ordinal,
        ),
        ("0", "1", 0)
    );

    let valid_member = members_for_plan_node(&output, "1");
    assert_eq!(valid_member.len(), 1);
    assert_eq!(
        valid_member[0].container_ref.as_deref(),
        Some(valid.container_ref.as_str())
    );
    assert!(valid_member[0].reasons.is_empty());

    let empty_pipeline_member = members_for_plan_node(&output, "2");
    assert_eq!(empty_pipeline_member.len(), 1);
    assert_eq!(empty_pipeline_member[0].fragment_ref.as_deref(), Some("0"));
    assert!(empty_pipeline_member[0].container_ref.is_none());
    assert_eq!(
        empty_pipeline_member[0].reasons,
        vec![
            "runtime_pipeline_ref_missing".to_owned(),
            "runtime_pipeline_container_missing".to_owned(),
        ]
    );

    let empty_fragment_member = members_for_plan_node(&output, "3");
    assert_eq!(empty_fragment_member.len(), 1);
    assert!(empty_fragment_member[0].fragment_ref.is_none());
    assert!(empty_fragment_member[0].container_ref.is_none());
    assert_eq!(
        empty_fragment_member[0].reasons,
        vec![
            "runtime_fragment_ref_missing".to_owned(),
            "runtime_pipeline_container_missing".to_owned(),
        ]
    );
    for code in [
        "runtime_pipeline_ref_missing",
        "runtime_fragment_ref_missing",
    ] {
        assert!(
            output.ir.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == code && !diagnostic.source_refs.is_empty()
            })
        );
    }
    assert!(
        output
            .ir
            .physical_operator_memberships
            .iter()
            .all(|membership| {
                !membership.source_refs.is_empty()
                    && membership.source_refs.iter().all(|source_ref| {
                        output
                            .source_refs
                            .iter()
                            .any(|available| available.ref_id == *source_ref)
                    })
            })
    );
}

#[test]
fn empty_fragment_or_pipeline_id_never_materializes_a_typed_container() {
    let output = adapt_text(EMPTY_CONTAINER_IDS);

    assert!(output.ir.pipeline_containers.is_empty());
    let empty_pipeline_member = members_for_plan_node(&output, "1");
    assert_eq!(empty_pipeline_member.len(), 1);
    assert_eq!(empty_pipeline_member[0].fragment_ref.as_deref(), Some("0"));
    assert!(empty_pipeline_member[0].container_ref.is_none());
    assert_eq!(
        empty_pipeline_member[0].reasons,
        vec![
            "runtime_pipeline_ref_missing".to_owned(),
            "runtime_pipeline_container_missing".to_owned(),
        ]
    );
    let empty_fragment_member = members_for_plan_node(&output, "2");
    assert_eq!(empty_fragment_member.len(), 1);
    assert!(empty_fragment_member[0].fragment_ref.is_none());
    assert!(empty_fragment_member[0].container_ref.is_none());
    assert_eq!(
        empty_fragment_member[0].reasons,
        vec![
            "runtime_fragment_ref_missing".to_owned(),
            "runtime_pipeline_ref_missing".to_owned(),
            "runtime_pipeline_container_missing".to_owned(),
        ]
    );
    for code in [
        "runtime_pipeline_ref_missing",
        "runtime_fragment_ref_missing",
    ] {
        assert!(
            output.ir.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == code && !diagnostic.source_refs.is_empty()
            })
        );
    }
}
