//! Real StarRocks 3.5.14 Profiles in states the product must fail closed on,
//! captured from the isolated 1 FE + 3 BE fixture by
//! `scripts/e2e/capture-profile-states.ps1`
//! (`fixtures/runtime/3.5.14/profile-states/manifest.json`):
//!
//! * `pipeline_profile_level=2`: the FE skips the isomorphic merge
//!   (`QueryRuntimeProfile.buildQueryProfile(needMerge=false)`), so every
//!   instance and driver is printed on its own;
//! * a running query's runtime report (`runtime_profile_report_interval`),
//!   read while `SHOW PROFILELIST` still says `Running`;
//! * the final Profile of that same query, as the positive control.
//!
//! For the first two the product must never print a precise-looking node
//! time, share, row count or finding: the execution summary is unavailable
//! with a reason, no rule is evaluated and no finding is emitted, and the
//! version check names the profile state.

use std::{fs, path::PathBuf};

use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    AdapterContext, AdapterLimits, InputEnvelope, adapt_input, assemble_public_report,
    evaluate_rules, resolve_version_and_graph, v01_analysis_receipt,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn states_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/runtime/3.5.14/profile-states")
        .canonicalize()
        .expect("profile-states directory")
}

fn manifest() -> Value {
    serde_json::from_str(&fs::read_to_string(states_dir().join("manifest.json")).expect("manifest"))
        .expect("manifest json")
}

fn read_verified(state: &Value, file_key: &str, sha_key: &str) -> String {
    let name = state[file_key].as_str().expect("file name");
    let bytes = fs::read(states_dir().join(name)).unwrap_or_else(|_| panic!("read {name}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        state[sha_key].as_str().expect("sha256"),
        "{name}: SHA-256 drift from manifest"
    );
    String::from_utf8(bytes).unwrap_or_else(|_| panic!("{name} is UTF-8"))
}

fn state(id: &str) -> Value {
    manifest()["states"]
        .as_array()
        .expect("states")
        .iter()
        .find(|state| state["state_id"] == id)
        .unwrap_or_else(|| panic!("state {id}"))
        .clone()
}

fn report_for(id: &str) -> Value {
    let state = state(id);
    read_verified(&state, "sql_file", "sql_sha256");
    read_verified(&state, "analyze_file", "analyze_sha256");
    report_of(&read_verified(&state, "profile_file", "profile_sha256"))
}

fn report_of(profile: &str) -> Value {
    let context = AdapterContext::new(v01_analysis_receipt()).expect("receipt context");
    let mut output = adapt_input(
        InputEnvelope::new(RequestedSourceKind::TextProfile, profile.as_bytes()),
        &AdapterLimits::default(),
        &context,
    )
    .expect("captured Profile adapts");
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("captured Profile resolves");
    evaluate_rules(&mut output).expect("rules evaluate");
    serde_json::to_value(assemble_public_report(output).expect("report assembles"))
        .expect("report serializes")
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The fail-closed contract shared by every non-final / unmerged state.
fn assert_fail_closed(id: &str, report: &Value, summary_reason: &str, version_issue: &str) {
    let summary = &report["execution_summary"];
    assert_eq!(summary["state"], "unavailable", "{id}: execution summary");
    assert_eq!(
        strings(&summary["reasons"]),
        vec![summary_reason],
        "{id}: summary reason"
    );
    assert!(
        summary["nodes"].as_array().is_some_and(Vec::is_empty),
        "{id}: no per-node time, share or rows"
    );
    assert_eq!(
        report["version"]["compatibility"], "partial",
        "{id}: version"
    );
    assert!(
        strings(&report["version"]["issues"]).contains(&version_issue),
        "{id}: version issue {version_issue}"
    );
    assert!(
        strings(&report["profile_context"]["limitations"]).contains(&version_issue),
        "{id}: profile limitation {version_issue}"
    );
    assert_ne!(report["analysis_state"], "complete", "{id}: analysis state");
    assert!(
        report["findings"].as_array().is_some_and(Vec::is_empty),
        "{id}: no finding"
    );
    for evaluation in report["rule_evaluations"]
        .as_array()
        .expect("rule evaluations")
    {
        assert_eq!(
            evaluation["status"], "not_applicable",
            "{id}: {} must not be evaluated",
            evaluation["rule_id"]
        );
    }
}

#[test]
fn manifest_records_the_multi_be_fixture_and_every_state() {
    let manifest = manifest();
    assert!(
        manifest["server_version"]
            .as_str()
            .is_some_and(|version| version.starts_with("3.5.14-23a56ec"))
    );
    assert_eq!(manifest["topology"]["backends"], 3);
    let ids: Vec<&str> = manifest["states"]
        .as_array()
        .expect("states")
        .iter()
        .filter_map(|state| state["state_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "s01_level2_shuffle_join",
            "s02_level2_multi_instance_agg",
            "s03_running_snapshot",
            "s04_running_query_final",
        ]
    );
    assert_eq!(
        state("s04_running_query_final")["query_id"],
        state("s03_running_snapshot")["query_id"],
        "the final control is the same query as the running snapshot"
    );
}

#[test]
fn level_2_profiles_fail_closed() {
    for id in ["s01_level2_shuffle_join", "s02_level2_multi_instance_agg"] {
        let report = report_for(id);
        let context = &report["profile_context"];
        assert_eq!(context["profile_level"], "level_2", "{id}");
        assert_eq!(context["merge_state"], "unmerged", "{id}");
        assert_fail_closed(id, &report, "profile_not_merged", "profile_level_2");
    }
}

#[test]
fn running_snapshot_fails_closed() {
    let id = "s03_running_snapshot";
    let report = report_for(id);
    assert_eq!(report["profile_context"]["query_state"], "running");
    assert_fail_closed(
        id,
        &report,
        "profile_running_snapshot",
        "profile_running_snapshot",
    );
}

/// `QueryCumulativeCpuTime` duplication (v0.5-05 limitation) on real data:
/// the unmerged Level 2 Profile keeps every instance's own report. Instances
/// on one BE report that BE's shared `QueryContext::cpu_cost()` snapshot, so
/// two of them can print the very same value, and the FE's per-instance sum
/// (`QueryRuntimeProfile.java:449-453`) exceeds the per-BE totals.
#[test]
fn level_2_instances_on_one_be_report_the_shared_query_cpu() {
    let state = state("s02_level2_multi_instance_agg");
    let profile = read_verified(&state, "profile_file", "profile_sha256");
    let mut address = None::<String>;
    let mut reports = Vec::<(String, u64)>::new();
    for line in profile.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("- Address: ") {
            address = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("- QueryCumulativeCpuTime: ") {
            // Level 2 prints these as `N.NNNms`.
            let micros = value
                .strip_suffix("ms")
                .and_then(|ms| ms.split_once('.'))
                .map(|(whole, frac)| {
                    whole.parse::<u64>().unwrap() * 1000 + frac.parse::<u64>().unwrap()
                })
                .unwrap_or_else(|| panic!("unexpected CPU text {value}"));
            reports.push((address.clone().expect("instance address"), micros));
        }
    }
    let backends: std::collections::BTreeSet<&str> = reports
        .iter()
        .map(|(address, _)| address.as_str())
        .collect();
    assert_eq!(backends.len(), 3, "instances on three BEs");
    assert!(
        reports.len() > backends.len(),
        "some BE runs several instances"
    );
    let per_be_max: u64 = backends
        .iter()
        .map(|be| {
            reports
                .iter()
                .filter(|(address, _)| address == be)
                .map(|(_, cpu)| *cpu)
                .max()
                .unwrap()
        })
        .sum();
    let fe_sum: u64 = reports.iter().map(|(_, cpu)| cpu).sum();
    assert!(
        fe_sum > per_be_max,
        "FE sum {fe_sum}us over-counts the per-BE {per_be_max}us"
    );
    let shared = backends.iter().any(|be| {
        let values: Vec<u64> = reports
            .iter()
            .filter(|(address, _)| address == be)
            .map(|(_, cpu)| *cpu)
            .collect();
        values
            .iter()
            .enumerate()
            .any(|(i, value)| values[i + 1..].contains(value))
    });
    assert!(
        shared,
        "two instances of one BE print the same shared snapshot"
    );
}

/// The official incomplete markers on an otherwise final, merged multi-BE
/// Profile (`NotIdentical`, `MissingInstanceIds`, operator `IsRunning`): the
/// summary is withdrawn instead of summing the partial operators.
#[test]
fn incomplete_markers_withdraw_the_execution_summary() {
    let path = states_dir().join("../oracle-corpus-multi-be/c03_shuffle_join_agg.profile.txt");
    let profile = fs::read_to_string(path).expect("multi-BE c03 Profile");
    assert_eq!(
        report_of(&profile)["execution_summary"]["state"],
        "available"
    );
    for (marked, reason) in [
        (
            profile.replacen("  Execution:", "  Execution:\n     - NotIdentical", 1),
            "runtime_profile_not_identical",
        ),
        (
            profile.replacen(
                "  Execution:",
                "  Execution:\n     - MissingInstanceIds: redacted-instance",
                1,
            ),
            "runtime_profile_missing_instances",
        ),
        (
            profile.replacen(
                "CommonMetrics:",
                "CommonMetrics:\n                 - IsRunning",
                1,
            ),
            "runtime_operator_running",
        ),
    ] {
        assert_ne!(marked, profile, "{reason}: marker injected");
        let report = report_of(&marked);
        assert_eq!(
            report["execution_summary"]["state"], "unavailable",
            "{reason}"
        );
        assert_eq!(
            strings(&report["execution_summary"]["reasons"]),
            vec![reason],
            "{reason}"
        );
    }
}

#[test]
fn final_profile_of_the_same_query_is_fully_analyzed() {
    let id = "s04_running_query_final";
    let report = report_for(id);
    let context = &report["profile_context"];
    assert_eq!(context["profile_level"], "level_1");
    assert_eq!(context["query_state"], "finished");
    assert_eq!(context["snapshot_kind"], "final");
    assert_eq!(context["merge_state"], "merged");
    assert_eq!(report["version"]["compatibility"], "compatible");
    assert_eq!(report["execution_summary"]["state"], "available");
    assert!(
        report["execution_summary"]["nodes"]
            .as_array()
            .is_some_and(|nodes| !nodes.is_empty())
    );
    // Multi-BE Level 1: the summed CPU keeps its backend-owned caveat.
    let cpu = report["derived_metric_cells"]
        .as_array()
        .expect("derived cells")
        .iter()
        .find(|cell| cell["metric_id"] == "QueryCumulativeCpuTime")
        .expect("query CPU cell");
    assert!(
        strings(&cpu["reasons"])
            .contains(&"query_cpu_summed_over_instance_reports_not_wall_or_operator_time")
    );
}
